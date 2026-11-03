//! The transports of the message surface. Throwaway, never product code.
//!
//! One endpoint trait, three endpoints: in process (the request bytes handed
//! to the server under its lock), a loopback TCP socket and, where the
//! platform has them, a Unix domain socket. The two sockets carry the same
//! frames: a 4-byte little-endian length, then the message. The server side
//! accepts connections on a thread and serves each on its own thread; a
//! connection ends when the client closes it (its thread ends with it, and is
//! not waited for by the accept loop), and the session it served survives for
//! a later connection to resume.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::surface::Server;

/// The largest frame a peer accepts: a guard against a malformed length.
const MAX_FRAME: u32 = 64 << 20;

pub trait Endpoint: Send {
    fn call(&mut self, request: &[u8]) -> io::Result<Vec<u8>>;
    /// Bytes sent and received so far, frames included.
    fn traffic(&self) -> (u64, u64);
}

pub struct InProcess {
    server: Arc<Mutex<Server>>,
    sent: u64,
    received: u64,
}

impl InProcess {
    pub fn new(server: Arc<Mutex<Server>>) -> InProcess {
        InProcess { server, sent: 0, received: 0 }
    }
}

impl Endpoint for InProcess {
    fn call(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        let response = self.server.lock().unwrap().handle(request);
        self.sent += request.len() as u64;
        self.received += response.len() as u64;
        Ok(response)
    }

    fn traffic(&self) -> (u64, u64) {
        (self.sent, self.received)
    }
}

fn write_frame(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

/// Reads one frame; `Ok(None)` at a clean end of stream before a frame.
fn read_frame(stream: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
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

/// A framed stream as an endpoint.
pub struct Framed<S: Read + Write + Send> {
    stream: S,
    sent: u64,
    received: u64,
}

impl<S: Read + Write + Send> Framed<S> {
    pub fn new(stream: S) -> Framed<S> {
        Framed { stream, sent: 0, received: 0 }
    }
}

impl<S: Read + Write + Send> Endpoint for Framed<S> {
    fn call(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        write_frame(&mut self.stream, request)?;
        self.sent += 4 + request.len() as u64;
        let response = read_frame(&mut self.stream)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection"))?;
        self.received += 4 + response.len() as u64;
        Ok(response)
    }

    fn traffic(&self) -> (u64, u64) {
        (self.sent, self.received)
    }
}

/// Serves one connection until the client closes it.
fn serve_connection(mut stream: impl Read + Write, server: &Mutex<Server>) -> io::Result<()> {
    while let Some(request) = read_frame(&mut stream)? {
        let response = server.lock().unwrap().handle(&request);
        write_frame(&mut stream, &response)?;
    }
    Ok(())
}

/// A listening server: an accept loop on a thread, stopped by `stop`. The
/// connections it accepted end when their clients close them.
pub struct Listening {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub address: String,
    pub transport: &'static str,
}

impl Listening {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the accept loop never panics");
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Serves `server` on a loopback TCP port chosen by the system.
pub fn serve_tcp(server: Arc<Mutex<Server>>) -> io::Result<Listening> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    let thread = thread::Builder::new().name("tcp-accept".to_string()).spawn(move || {
        while !stop_flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_nonblocking(false);
                    let server = server.clone();
                    thread::spawn(move || {
                        let _ = serve_connection(&stream, &server);
                        let _ = stream.shutdown(Shutdown::Both);
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_micros(200)),
                Err(_) => break,
            }
        }
    })?;
    Ok(Listening { stop, thread: Some(thread), address, transport: "tcp-loopback" })
}

pub fn connect_tcp(address: &str) -> io::Result<Framed<TcpStream>> {
    let address: SocketAddr = address.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?;
    let stream = TcpStream::connect(address)?;
    stream.set_nodelay(true)?;
    Ok(Framed::new(stream))
}

#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    /// Serves `server` on a Unix domain socket in the temporary directory.
    pub fn serve_unix(server: Arc<Mutex<Server>>, name: &str) -> io::Result<Listening> {
        let path: PathBuf = std::env::temp_dir().join(format!("ludenea-toy-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let address = path.to_string_lossy().into_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let cleanup = path.clone();
        let thread = thread::Builder::new().name("unix-accept".to_string()).spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let server = server.clone();
                        thread::spawn(move || {
                            let _ = serve_connection(&stream, &server);
                            let _ = stream.shutdown(Shutdown::Both);
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_micros(200)),
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(&cleanup);
        })?;
        Ok(Listening { stop, thread: Some(thread), address, transport: "unix-domain-socket" })
    }

    pub fn connect_unix(path: &str) -> io::Result<Framed<UnixStream>> {
        Ok(Framed::new(UnixStream::connect(path)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::PROTOCOL;
    use crate::world::{Size, WorldConfig};

    fn describe_request() -> Vec<u8> {
        let mut bytes = PROTOCOL.to_le_bytes().to_vec();
        bytes.push(0);
        bytes
    }

    #[test]
    fn the_tcp_endpoint_answers_like_the_in_process_one_and_survives_reconnection() {
        let server = Arc::new(Mutex::new(Server::new(WorldConfig::new(Size::Small, 1), 1)));
        let listening = serve_tcp(server.clone()).unwrap();
        let mut in_process = InProcess::new(server.clone());
        let expected = in_process.call(&describe_request()).unwrap();
        let mut tcp = connect_tcp(&listening.address).unwrap();
        assert_eq!(tcp.call(&describe_request()).unwrap(), expected);
        assert_eq!(tcp.traffic(), (4 + 5, 4 + expected.len() as u64));
        drop(tcp);
        let mut again = connect_tcp(&listening.address).unwrap();
        assert_eq!(again.call(&describe_request()).unwrap(), expected);
        listening.stop();
    }

    #[cfg(unix)]
    #[test]
    fn the_unix_endpoint_answers_like_the_in_process_one() {
        let server = Arc::new(Mutex::new(Server::new(WorldConfig::new(Size::Small, 1), 1)));
        let listening = unix::serve_unix(server.clone(), "test").unwrap();
        let expected = InProcess::new(server.clone()).call(&describe_request()).unwrap();
        let mut socket = unix::connect_unix(&listening.address).unwrap();
        assert_eq!(socket.call(&describe_request()).unwrap(), expected);
        let path = listening.address.clone();
        drop(socket);
        listening.stop();
        assert!(!std::path::Path::new(&path).exists(), "the socket file is removed");
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
