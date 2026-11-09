//! The two transports of the paced editor session. Throwaway, never product
//! code.
//!
//! A paced session is a duplex stream, not a request and its response: the
//! server delivers messages at its period and the client submits commands
//! when it wants. Both transports give the same three operations. In process,
//! a delivered message is handed to the client through a channel and a
//! submitted command is handed to the session on the client's thread. Over
//! the loopback TCP socket both directions carry frames (a 4-byte
//! little-endian length, then the message), as the toy spike's sockets do,
//! and one thread per connection reads the client's frames on the server
//! side: with the thread that delivers at the period, it stands for the I/O
//! executor.

use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

/// The largest frame a peer accepts: a guard against a malformed length.
const MAX_FRAME: u32 = 64 << 20;

/// What the session does with a message a client submitted.
pub type Accept = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Server to client.
pub trait Deliver: Send {
    fn deliver(&mut self, message: &[u8]) -> io::Result<()>;
}

/// The client's side of the deliveries; `Ok(None)` when the server is gone.
pub trait Receive: Send {
    fn receive(&mut self) -> io::Result<Option<Vec<u8>>>;
}

/// Client to server, several messages in one write.
pub trait Submit: Send {
    fn submit(&mut self, messages: &[Vec<u8>]) -> io::Result<()>;
}

/// Bytes the client sent and received, frames included over a socket.
#[derive(Default)]
pub struct Traffic {
    pub sent: AtomicU64,
    pub received: AtomicU64,
}

pub struct ServerSide {
    pub deliver: Box<dyn Deliver>,
    pub transport: &'static str,
    connection: Option<TcpStream>,
}

impl Drop for ServerSide {
    /// Closes the connection. The reader thread is not waited for, as in the
    /// toy spike: it ends when the client closes its side, which a blocked
    /// read does not learn from a local shutdown on every platform.
    fn drop(&mut self) {
        if let Some(stream) = self.connection.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

pub struct ClientSide {
    pub receive: Box<dyn Receive>,
    pub submit: Box<dyn Submit>,
    pub traffic: Arc<Traffic>,
}

// ---------------------------------------------------------------------------
// In process
// ---------------------------------------------------------------------------

struct ChannelDeliver(mpsc::Sender<Vec<u8>>);

impl Deliver for ChannelDeliver {
    fn deliver(&mut self, message: &[u8]) -> io::Result<()> {
        self.0.send(message.to_vec()).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the client is gone"))
    }
}

struct ChannelReceive(mpsc::Receiver<Vec<u8>>, Arc<Traffic>);

impl Receive for ChannelReceive {
    fn receive(&mut self) -> io::Result<Option<Vec<u8>>> {
        let message = self.0.recv().ok();
        if let Some(message) = &message {
            self.1.received.fetch_add(message.len() as u64, Ordering::Relaxed);
        }
        Ok(message)
    }
}

struct DirectSubmit(Accept, Arc<Traffic>);

impl Submit for DirectSubmit {
    fn submit(&mut self, messages: &[Vec<u8>]) -> io::Result<()> {
        for message in messages {
            self.1.sent.fetch_add(message.len() as u64, Ordering::Relaxed);
            (self.0)(message);
        }
        Ok(())
    }
}

pub fn in_process(accept: Accept) -> (ServerSide, ClientSide) {
    let (sender, receiver) = mpsc::channel();
    let traffic = Arc::new(Traffic::default());
    let server = ServerSide { deliver: Box::new(ChannelDeliver(sender)), transport: "in-process", connection: None };
    let client = ClientSide { receive: Box::new(ChannelReceive(receiver, traffic.clone())), submit: Box::new(DirectSubmit(accept, traffic.clone())), traffic };
    (server, client)
}

// ---------------------------------------------------------------------------
// The loopback socket
// ---------------------------------------------------------------------------

fn append_frame(out: &mut Vec<u8>, message: &[u8]) {
    out.extend_from_slice(&(message.len() as u32).to_le_bytes());
    out.extend_from_slice(message);
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

struct SocketDeliver {
    stream: TcpStream,
    frame: Vec<u8>,
}

impl Deliver for SocketDeliver {
    fn deliver(&mut self, message: &[u8]) -> io::Result<()> {
        self.frame.clear();
        append_frame(&mut self.frame, message);
        self.stream.write_all(&self.frame)
    }
}

struct SocketReceive(BufReader<TcpStream>, Arc<Traffic>);

impl Receive for SocketReceive {
    fn receive(&mut self) -> io::Result<Option<Vec<u8>>> {
        let message = read_frame(&mut self.0)?;
        if let Some(message) = &message {
            self.1.received.fetch_add(4 + message.len() as u64, Ordering::Relaxed);
        }
        Ok(message)
    }
}

struct SocketSubmit {
    stream: TcpStream,
    frames: Vec<u8>,
    traffic: Arc<Traffic>,
}

impl Submit for SocketSubmit {
    fn submit(&mut self, messages: &[Vec<u8>]) -> io::Result<()> {
        self.frames.clear();
        for message in messages {
            append_frame(&mut self.frames, message);
        }
        self.traffic.sent.fetch_add(self.frames.len() as u64, Ordering::Relaxed);
        self.stream.write_all(&self.frames)
    }
}

/// One connection over a loopback TCP port chosen by the system, with the
/// thread that reads the client's frames on the server side.
pub fn loopback(accept: Accept) -> io::Result<(ServerSide, ClientSide)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;
    let mut reading = BufReader::new(server.try_clone()?);
    thread::Builder::new().name("connection-reader".to_string()).spawn(move || {
        while let Ok(Some(message)) = read_frame(&mut reading) {
            accept(&message);
        }
    })?;
    let traffic = Arc::new(Traffic::default());
    let server_side = ServerSide {
        deliver: Box::new(SocketDeliver { stream: server.try_clone()?, frame: Vec::new() }),
        transport: "tcp-loopback",
        connection: Some(server),
    };
    let client_side = ClientSide {
        receive: Box::new(SocketReceive(BufReader::with_capacity(1 << 16, client.try_clone()?), traffic.clone())),
        submit: Box::new(SocketSubmit { stream: client, frames: Vec::new(), traffic: traffic.clone() }),
        traffic,
    };
    Ok((server_side, client_side))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn exchange(link: fn(Accept) -> io::Result<(ServerSide, ClientSide)>) {
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let sink = accepted.clone();
        let (mut server, mut client) = link(Arc::new(move |message: &[u8]| sink.lock().unwrap().push(message.to_vec()))).unwrap();
        let large = vec![7_u8; 300_000];
        thread::scope(|scope| {
            scope.spawn(|| {
                server.deliver.deliver(&large).unwrap();
                server.deliver.deliver(b"second").unwrap();
            });
            client.submit.submit(&[b"one".to_vec(), b"two".to_vec()]).unwrap();
            assert_eq!(client.receive.receive().unwrap().unwrap(), large);
            assert_eq!(client.receive.receive().unwrap().unwrap(), b"second");
        });
        let started = std::time::Instant::now();
        while accepted.lock().unwrap().len() < 2 {
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "the submitted messages never arrived");
            thread::yield_now();
        }
        assert_eq!(*accepted.lock().unwrap(), [b"one".to_vec(), b"two".to_vec()]);
        assert!(client.traffic.received.load(Ordering::Relaxed) >= 300_006);
        drop(server);
        assert!(client.receive.receive().unwrap_or(None).is_none(), "the stream ends when the server is gone");
    }

    #[test]
    fn the_in_process_link_delivers_in_order_and_hands_submissions_to_the_session() {
        exchange(|accept| Ok(in_process(accept)));
    }

    #[test]
    fn the_loopback_link_delivers_in_order_and_hands_submissions_to_the_session() {
        exchange(loopback);
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        let mut bytes = (MAX_FRAME + 1).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        assert_eq!(read_frame(&mut &bytes[..]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_frame(&mut &[][..]).unwrap().is_none(), "a clean end of stream");
    }
}
