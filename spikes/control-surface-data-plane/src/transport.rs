//! The local transports. Throwaway, never product code.
//!
//! A trimmed copy of the control surface toy spike's transports: a loopback
//! TCP socket (which stands in for the named pipe on Windows) and, where the
//! platform has one, a Unix domain socket, both carrying length-prefixed
//! frames, one thread per connection on the server side. Two things are
//! added: after the reply that opens a ring, the server sends the ring's
//! descriptor over the Unix domain socket; and a connection that attaches
//! itself as a session's second stream stops being served and is handed to
//! the thread that copies frames.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::surface::{Connection, Handoff, Server};

/// The largest frame a peer accepts: a guard against a malformed length.
const MAX_FRAME: u32 = 64 << 20;

/// A connection of either transport.
pub trait Link: Read + Write + Send {
    fn passes_descriptors(&self) -> bool;
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn close(&self);
    fn into_writer(self: Box<Self>) -> Box<dyn Write + Send>;
    #[cfg(unix)]
    fn unix(&self) -> Option<&std::os::unix::net::UnixStream>;
}

impl Link for TcpStream {
    fn passes_descriptors(&self) -> bool {
        false
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        TcpStream::set_nonblocking(self, nonblocking)
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }

    fn close(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }

    fn into_writer(self: Box<Self>) -> Box<dyn Write + Send> {
        self
    }

    #[cfg(unix)]
    fn unix(&self) -> Option<&std::os::unix::net::UnixStream> {
        None
    }
}

#[cfg(unix)]
impl Link for std::os::unix::net::UnixStream {
    fn passes_descriptors(&self) -> bool {
        true
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_nonblocking(self, nonblocking)
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }

    fn close(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }

    fn into_writer(self: Box<Self>) -> Box<dyn Write + Send> {
        self
    }

    fn unix(&self) -> Option<&std::os::unix::net::UnixStream> {
        Some(self)
    }
}

fn write_frame(stream: &mut dyn Link, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

/// Reads one frame; `Ok(None)` at a clean end of stream before a frame.
fn read_frame(stream: &mut dyn Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0_u8; 4];
    match stream.read_exact(&mut len) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes refused")));
    }
    let mut bytes = vec![0_u8; len as usize];
    stream.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

/// The client's end of a connection: one request, one reply.
pub struct Framed {
    stream: Box<dyn Link>,
}

impl Framed {
    pub fn call(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        write_frame(&mut *self.stream, request)?;
        read_frame(&mut *self.stream)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection"))
    }

    #[cfg(unix)]
    pub fn link(&self) -> &dyn Link {
        &*self.stream
    }

    pub fn into_link(self) -> Box<dyn Link> {
        self.stream
    }
}

/// Serves one connection until the client closes it or it becomes a second stream.
fn serve_connection(mut stream: Box<dyn Link>, server: &Mutex<Server>) -> io::Result<()> {
    let connection = Connection { passes_descriptors: stream.passes_descriptors() };
    while let Some(request) = read_frame(&mut *stream)? {
        let reply = server.lock().unwrap().handle(&request, connection);
        write_frame(&mut *stream, &reply.bytes)?;
        match reply.handoff {
            Handoff::None => {}
            #[cfg(unix)]
            Handoff::Descriptor(descriptor) => {
                let socket = stream.unix().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "a descriptor needs a Unix domain socket"))?;
                crate::shared_memory::send_descriptor(socket, &descriptor)?;
            }
            Handoff::SecondStream(session) => {
                stream.set_nonblocking(true)?;
                server.lock().unwrap().attach_second_stream(session, stream.into_writer());
                return Ok(());
            }
        }
    }
    stream.close();
    Ok(())
}

#[derive(Clone, Debug)]
pub enum Address {
    Tcp(String),
    #[cfg(unix)]
    Unix(String),
}

impl Address {
    pub fn transport(&self) -> &'static str {
        match self {
            Address::Tcp(_) => "tcp-loopback",
            #[cfg(unix)]
            Address::Unix(_) => "unix-domain-socket",
        }
    }

    pub fn connect(&self) -> io::Result<Framed> {
        match self {
            Address::Tcp(address) => {
                let stream = TcpStream::connect(address.as_str())?;
                stream.set_nodelay(true)?;
                Ok(Framed { stream: Box::new(stream) })
            }
            #[cfg(unix)]
            Address::Unix(path) => Ok(Framed { stream: Box::new(std::os::unix::net::UnixStream::connect(path)?) }),
        }
    }
}

/// A listening server: an accept loop on a thread, stopped when dropped. The
/// connections it accepted end when their clients close them.
pub struct Listening {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub address: Address,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn accept_loop(server: Arc<Mutex<Server>>, address: Address, mut accept: impl FnMut() -> io::Result<Box<dyn Link>> + Send + 'static) -> io::Result<Listening> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    let thread = thread::Builder::new().name("accept".to_string()).spawn(move || {
        while !stop_flag.load(Ordering::SeqCst) {
            match accept() {
                Ok(stream) => {
                    let server = server.clone();
                    thread::spawn(move || {
                        let _ = serve_connection(stream, &server);
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_micros(200)),
                Err(_) => break,
            }
        }
    })?;
    Ok(Listening { stop, thread: Some(thread), address })
}

/// Serves `server` on a loopback TCP port chosen by the system.
pub fn serve_tcp(server: Arc<Mutex<Server>>) -> io::Result<Listening> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = Address::Tcp(listener.local_addr()?.to_string());
    accept_loop(server, address, move || {
        let (stream, _) = listener.accept()?;
        let _ = stream.set_nodelay(true);
        stream.set_nonblocking(false)?;
        Ok(Box::new(stream))
    })
}

/// Serves `server` on a Unix domain socket in the temporary directory.
#[cfg(unix)]
pub fn serve_unix(server: Arc<Mutex<Server>>) -> io::Result<Listening> {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!("ludenea-data-plane-{}-{}.sock", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // The clients connect by path while the socket listens; the path is
    // removed once the listener is dropped with the accept loop.
    struct Bound(UnixListener, std::path::PathBuf);
    impl Drop for Bound {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.1);
        }
    }
    listener.set_nonblocking(true)?;
    let address = Address::Unix(path.to_string_lossy().into_owned());
    let bound = Bound(listener, path);
    accept_loop(server, address, move || {
        let (stream, _) = bound.0.accept()?;
        stream.set_nonblocking(false)?;
        Ok(Box::new(stream))
    })
}

/// The transports this platform serves, the one that can hand a ring over first.
pub fn serve_all(server: &Arc<Mutex<Server>>) -> io::Result<Vec<Listening>> {
    let mut listening = Vec::new();
    #[cfg(unix)]
    listening.push(serve_unix(server.clone())?);
    listening.push(serve_tcp(server.clone())?);
    Ok(listening)
}

/// Reads exactly `buffer.len()` bytes from a stream with a read timeout,
/// going on across timeouts until `give_up`, told how many bytes are already
/// read, says to stop. Returns whether the buffer was filled.
pub fn read_full(stream: &mut dyn Read, buffer: &mut [u8], mut give_up: impl FnMut(usize) -> bool) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => filled += count,
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                if give_up(filled) {
                    return Ok(false);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_plane::PlaneConfig;
    use crate::surface::{PROTOCOL, ServerConfig, op};
    use crate::world::{Size, WorldConfig};

    fn server() -> Arc<Mutex<Server>> {
        Arc::new(Mutex::new(Server::new(ServerConfig { world: WorldConfig::new(Size::Small, 1), plane: PlaneConfig::default(), log_retention: 100 })))
    }

    fn open_request() -> Vec<u8> {
        let mut bytes = PROTOCOL.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[op::OPEN, 0]);
        bytes
    }

    #[test]
    fn every_transport_answers_alike_and_survives_a_reconnection() {
        let server = server();
        let expected = server.lock().unwrap().handle(&open_request(), Connection { passes_descriptors: false }).bytes;
        for listening in serve_all(&server).unwrap() {
            let mut first = listening.address.connect().unwrap();
            let reply = first.call(&open_request()).unwrap();
            assert_eq!(reply.len(), expected.len(), "{}", listening.address.transport());
            assert_eq!(reply[0], 0);
            drop(first);
            let mut again = listening.address.connect().unwrap();
            assert_eq!(again.call(&open_request()).unwrap()[0], 0);
        }
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        let mut bytes = (MAX_FRAME + 1).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        let error = read_frame(&mut &bytes[..]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(read_frame(&mut &[][..]).unwrap().is_none(), "a clean end of stream");
    }
}
