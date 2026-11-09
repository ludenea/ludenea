//! The client's side: the calls of the control plane and the thread that
//! reads the data plane. Throwaway, never product code.
//!
//! The client numbers its commands with the session's sequence and keeps
//! nothing else. Its reader runs on its own thread, as a client's would in
//! another process: it reads the ring through its own view of the shared
//! memory object the server handed over, or the second stream through its own
//! connection, copies every frame into a buffer it owns, and can be told to
//! stop reading for a while.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use reference_workload::fnv1a;

use crate::commands::{Body, Digests, Handle, Transaction};
use crate::data_plane::{FRAME_STREAM, Policy, STATE_STREAM, read_frame_stamp};
use crate::ring::{FrameHeader, HEADER_BYTES, RingReader};
use crate::surface::{CommandReply, PLANE_RING, PLANE_SECOND_STREAM, PROTOCOL, Profile, Statistics, op};
use crate::transport::{Address, Framed, Link, read_full};
use crate::wire::{Reader, put_bytes, put_u8, put_u32, put_u64};

#[derive(Debug)]
pub enum CallError {
    Io(io::Error),
    /// A structured error of the surface.
    Surface { code: String, message: String, context: u64 },
    Malformed(&'static str),
}

impl CallError {
    /// The code of a structured error, empty for another failure.
    pub fn code(&self) -> &str {
        if let CallError::Surface { code, .. } = self { code } else { "" }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Io(error) => write!(f, "transport: {error}"),
            CallError::Surface { code, message, context } => write!(f, "{code}: {message} ({context})"),
            CallError::Malformed(what) => write!(f, "malformed reply to {what}"),
        }
    }
}

impl From<io::Error> for CallError {
    fn from(error: io::Error) -> CallError {
        CallError::Io(error)
    }
}

pub type Fallible<T> = Result<T, CallError>;

/// What a step or a poll returns.
#[derive(Clone, Debug)]
pub struct Progress {
    pub tick: u32,
    /// One bit per stream identifier that has ended.
    pub ended_streams: u32,
    pub hashes: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct StreamView {
    pub id: u32,
    pub cursor: u32,
    pub ended: String,
    pub frames_written: u64,
    pub ticks_skipped: u64,
    pub frames_evicted: u64,
    pub frames_refused: u64,
    pub pauses: u64,
    pub copy_nanos: u64,
}

pub struct Client {
    link: Framed,
    address: Address,
    pub token: u64,
    pub entity_count: usize,
    /// The sequence the next new command takes.
    pub next_sequence: u64,
}

impl Client {
    pub fn connect(address: &Address, profile: Profile) -> Fallible<Client> {
        let mut client = Client { link: address.connect()?, address: address.clone(), token: 0, entity_count: 0, next_sequence: 1 };
        let reply = client.request(op::OPEN, |body| put_u8(body, u8::from(profile == Profile::Paced)))?;
        let mut reader = Reader::new(&reply);
        client.token = reader.u64().ok_or(CallError::Malformed("open"))?;
        client.entity_count = reader.u32().ok_or(CallError::Malformed("open"))? as usize;
        Ok(client)
    }

    fn exchange(link: &mut Framed, operation: u8, body: impl FnOnce(&mut Vec<u8>)) -> Fallible<Vec<u8>> {
        let mut request = PROTOCOL.to_le_bytes().to_vec();
        request.push(operation);
        body(&mut request);
        let mut reply = link.call(&request)?;
        if reply.first() == Some(&0) {
            reply.remove(0);
            return Ok(reply);
        }
        let mut reader = Reader::new(reply.get(1..).unwrap_or(&[]));
        let failure = (|| Some(CallError::Surface { code: reader.text()?, message: reader.text()?, context: reader.u64()? }))();
        Err(failure.unwrap_or(CallError::Malformed("an error")))
    }

    fn request(&mut self, operation: u8, body: impl FnOnce(&mut Vec<u8>)) -> Fallible<Vec<u8>> {
        Client::exchange(&mut self.link, operation, body)
    }

    fn progress(&mut self, operation: u8, argument: u32) -> Fallible<Progress> {
        let token = self.token;
        let reply = self.request(operation, |body| {
            put_u64(body, token);
            put_u32(body, argument);
        })?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            let (tick, ended_streams, count) = (reader.u32()?, reader.u32()?, reader.u32()?);
            let hashes: Option<Vec<u64>> = (0..count).map(|_| reader.u64()).collect();
            Some(Progress { tick, ended_streams, hashes: hashes? })
        })();
        decoded.ok_or(CallError::Malformed("a step or a poll"))
    }

    pub fn step(&mut self, ticks: u32) -> Fallible<Progress> {
        self.progress(op::STEP, ticks)
    }

    /// The tick, and the hashes of the ticks after `from`.
    pub fn poll(&mut self, from: u32) -> Fallible<Progress> {
        self.progress(op::POLL, from)
    }

    /// Sends command bytes under a sequence the caller chooses.
    pub fn command(&mut self, sequence: u64, bytes: &[u8]) -> Fallible<CommandReply> {
        let token = self.token;
        let reply = self.request(op::COMMAND, |body| {
            put_u64(body, token);
            put_u64(body, sequence);
            put_bytes(body, bytes);
        })?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            let (retry, applied, tick, outcome) = (reader.u8()? != 0, reader.u8()? != 0, reader.u32()?, reader.bytes()?);
            Some(CommandReply { retry, tick: applied.then_some(tick), outcome: applied.then(|| outcome.to_vec()) })
        })();
        decoded.ok_or(CallError::Malformed("a command"))
    }

    /// Sends a transaction as the session's next command. Returns its
    /// sequence, its canonical bytes and the reply.
    pub fn send(&mut self, transaction: &Transaction) -> Fallible<(u64, Vec<u8>, CommandReply)> {
        let bytes = transaction.encode();
        let sequence = self.next_sequence;
        let reply = self.command(sequence, &bytes)?;
        self.next_sequence += 1;
        Ok((sequence, bytes, reply))
    }

    pub fn entity_state(&mut self, handles: &[Handle]) -> Fallible<Vec<Option<Body>>> {
        let reply = self.request(op::ENTITY_STATE, |body| {
            put_u32(body, handles.len() as u32);
            for handle in handles {
                put_u32(body, handle.index);
                put_u32(body, handle.generation);
            }
        })?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            let count = reader.u32()?;
            (0..count)
                .map(|_| {
                    let present = reader.u8()? != 0;
                    let body = Body { x: reader.f32()?, y: reader.f32()?, vx: reader.f32()?, vy: reader.f32()?, mass: reader.f32()? };
                    Some(present.then_some(body))
                })
                .collect::<Option<Vec<_>>>()
        })();
        decoded.ok_or(CallError::Malformed("an entity state query"))
    }

    /// The full state through the socket: the tick and the canonical bytes.
    pub fn full_state(&mut self) -> Fallible<(u32, Vec<u8>)> {
        let reply = self.request(op::FULL_STATE, |_| {})?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| Some((reader.u32()?, reader.bytes()?.to_vec())))();
        decoded.ok_or(CallError::Malformed("a full state read"))
    }

    pub fn digests(&mut self) -> Fallible<Digests> {
        let reply = self.request(op::DIGESTS, |_| {})?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            reader.u32()?;
            Some(Digests { full: reader.u64()?, components: reader.u64()?, labels: reader.u64()? })
        })();
        decoded.ok_or(CallError::Malformed("the digests"))
    }

    pub fn statistics(&mut self) -> Fallible<Statistics> {
        let reply = self.request(op::STATISTICS, |_| {})?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            Some(Statistics {
                tick: reader.u32()?,
                extraction_micros: reader.f32()?,
                stepping_nanos: reader.u64()?,
                extracted_ticks: reader.u64()?,
                log_commands: reader.u64()?,
                log_bytes: reader.u64()?,
                retries_answered: reader.u64()?,
                gaps_refused: reader.u64()?,
                altered_retries_refused: reader.u64()?,
                retries_outside_retention: reader.u64()?,
            })
        })();
        decoded.ok_or(CallError::Malformed("the statistics"))
    }

    pub fn stream_status(&mut self) -> Fallible<Vec<StreamView>> {
        let token = self.token;
        let reply = self.request(op::STREAM_STATUS, |body| put_u64(body, token))?;
        let mut reader = Reader::new(&reply);
        let decoded = (|| {
            let count = reader.u32()?;
            (0..count)
                .map(|_| {
                    let id = reader.u32()?;
                    Policy::from_tag(reader.u8()?)?;
                    Some(StreamView {
                        id,
                        cursor: reader.u32()?,
                        ended: reader.text()?,
                        frames_written: reader.u64()?,
                        ticks_skipped: reader.u64()?,
                        frames_evicted: reader.u64()?,
                        frames_refused: reader.u64()?,
                        pauses: reader.u64()?,
                        copy_nanos: reader.u64()?,
                    })
                })
                .collect::<Option<Vec<_>>>()
        })();
        decoded.ok_or(CallError::Malformed("the stream status"))
    }

    pub fn resume_stream(&mut self, stream: u32, cursor: u32) -> Fallible<()> {
        let token = self.token;
        self.request(op::RESUME_STREAM, |body| {
            put_u64(body, token);
            put_u32(body, stream);
            put_u32(body, cursor);
        })
        .map(drop)
    }

    fn open_data_plane(&mut self, mode: u8, state: Policy, frame: Policy) -> Fallible<Vec<u8>> {
        let token = self.token;
        self.request(op::OPEN_DATA_PLANE, |body| {
            put_u64(body, token);
            put_u8(body, mode);
            put_u8(body, state.tag());
            put_u8(body, frame.tag());
        })
    }

    /// Opens the session's ring and attaches to it through the client's own
    /// view of the shared memory object the server hands over.
    pub fn open_ring(&mut self, state: Policy, frame: Policy) -> Fallible<Source> {
        let reply = self.open_data_plane(PLANE_RING, state, frame)?;
        let mut reader = Reader::new(&reply);
        let (length, name) = (|| Some((reader.u64()? as usize, reader.text()?)))().ok_or(CallError::Malformed("the data plane's opening"))?;
        #[cfg(windows)]
        let mapping = crate::shared_memory::open_named(&name, length)?;
        #[cfg(unix)]
        let mapping = {
            let _ = name;
            let socket = self.link.link().unix().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "a descriptor needs a Unix domain socket"))?;
            crate::shared_memory::from_descriptor(crate::shared_memory::receive_descriptor(socket)?, length)?
        };
        Ok(Source::Ring(RingReader::attach(mapping)?))
    }

    /// Opens the session's data plane as a second stream: another connection
    /// of the same transport, which the server then only writes frames to.
    pub fn open_second_stream(&mut self, state: Policy, frame: Policy) -> Fallible<Source> {
        self.open_data_plane(PLANE_SECOND_STREAM, state, frame)?;
        let mut second = self.address.connect()?;
        let token = self.token;
        Client::exchange(&mut second, op::ATTACH_SECOND_STREAM, |body| put_u64(body, token))?;
        let link = second.into_link();
        link.set_read_timeout(Some(Duration::from_millis(5)))?;
        Ok(Source::SecondStream(link))
    }
}

/// Where the reader thread reads the data plane from.
pub enum Source {
    Ring(RingReader),
    SecondStream(Box<dyn Link>),
}

/// What the client's main thread and its reader thread share.
#[derive(Default)]
pub struct ReaderControl {
    /// While set, the reader reads nothing: a stalled client.
    pub stall: AtomicBool,
    pub stop: AtomicBool,
    /// The last sequence received per stream identifier.
    pub last_sequence: [AtomicU64; 3],
    /// Times the reader looked and found nothing to read.
    pub empty_polls: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Receipt {
    pub frames: u64,
    pub bytes: u64,
    pub first: u64,
    pub last: u64,
    /// Sequences jumped over: lost under a lossless policy, skipped under latest-wins.
    pub missing: u64,
    /// Frames whose sequence was not above the one before.
    pub duplicates: u64,
    /// The time spent obtaining the frames' bytes, waiting excluded.
    pub read_nanos: u64,
}

impl Receipt {
    pub fn micros_per_frame(&self) -> f64 {
        if self.frames == 0 { 0.0 } else { self.read_nanos as f64 / self.frames as f64 / 1_000.0 }
    }

    /// Every sequence from `first` to `last` once, in order.
    pub fn complete(&self, first: u64, last: u64) -> bool {
        self.frames == last + 1 - first && self.first == first && self.last == last && self.missing == 0 && self.duplicates == 0
    }
}

#[derive(Debug, Default)]
pub struct ReaderReport {
    pub streams: [Receipt; 3],
    /// The tick and the FNV-1a hash of every state frame's payload, when verifying.
    pub state_hashes: Vec<(u32, u64)>,
    /// The tick and the hash stamped in every frame stream payload.
    pub frame_stamps: Vec<(u32, u64)>,
    pub malformed_frames: u64,
    pub torn_reads: u64,
    pub error: Option<String>,
}

impl ReaderReport {
    fn record(&mut self, header: &FrameHeader, payload: &[u8], nanos: u64, verify: bool, control: &ReaderControl) {
        let Some(receipt) = self.streams.get_mut(header.stream as usize) else {
            self.malformed_frames += 1;
            return;
        };
        receipt.frames += 1;
        receipt.bytes += payload.len() as u64;
        receipt.read_nanos += nanos;
        if receipt.frames == 1 {
            receipt.first = header.sequence;
        } else if header.sequence <= receipt.last {
            receipt.duplicates += 1;
        } else {
            receipt.missing += header.sequence - receipt.last - 1;
        }
        receipt.last = receipt.last.max(header.sequence);
        if header.stream == STATE_STREAM && verify {
            self.state_hashes.push((header.tick, fnv1a(payload)));
        }
        if header.stream == FRAME_STREAM {
            match read_frame_stamp(payload, header.tick) {
                Some(hash) => self.frame_stamps.push((header.tick, hash)),
                None => self.malformed_frames += 1,
            }
        }
        control.last_sequence[header.stream as usize].store(receipt.last, Ordering::SeqCst);
    }
}

fn read_ring(mut ring: RingReader, control: &ReaderControl, verify: bool, report: &mut ReaderReport) {
    let mut payload = Vec::new();
    while !control.stop.load(Ordering::SeqCst) {
        if control.stall.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_micros(200));
            continue;
        }
        let started = Instant::now();
        match ring.read(&mut payload) {
            Some(header) => report.record(&header, &payload, started.elapsed().as_nanos() as u64, verify, control),
            None => {
                control.empty_polls.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_micros(200));
            }
        }
    }
    report.torn_reads = ring.torn_reads;
}

fn read_second_stream(mut link: Box<dyn Link>, control: &ReaderControl, verify: bool, report: &mut ReaderReport) -> io::Result<()> {
    let mut payload = Vec::new();
    while !control.stop.load(Ordering::SeqCst) {
        if control.stall.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_micros(200));
            continue;
        }
        let mut header = [0_u8; HEADER_BYTES];
        // Between two frames the reader may be told to stall or to stop.
        let between_frames = |filled: usize| {
            if filled == 0 {
                control.empty_polls.fetch_add(1, Ordering::SeqCst);
            }
            filled == 0 && (control.stall.load(Ordering::SeqCst) || control.stop.load(Ordering::SeqCst))
        };
        if !read_full(&mut *link, &mut header, between_frames)? {
            continue;
        }
        let header = FrameHeader::decode(&header);
        payload.resize(header.length as usize, 0);
        let started = Instant::now();
        // A frame begun is read to its end, stalled or not: the stream has no other framing.
        if !read_full(&mut *link, &mut payload, |_| control.stop.load(Ordering::SeqCst))? {
            break;
        }
        report.record(&header, &payload, started.elapsed().as_nanos() as u64, verify, control);
    }
    link.close();
    Ok(())
}

/// Starts the client's reader thread. With `verify`, every state frame's
/// payload is hashed, outside the time charged to reading.
pub fn spawn_reader(source: Source, control: Arc<ReaderControl>, verify: bool) -> JoinHandle<ReaderReport> {
    thread::Builder::new()
        .name("client-reader".to_string())
        .spawn(move || {
            let mut report = ReaderReport::default();
            match source {
                Source::Ring(ring) => read_ring(ring, &control, verify, &mut report),
                Source::SecondStream(link) => {
                    if let Err(error) = read_second_stream(link, &control, verify, &mut report) {
                        report.error = Some(error.to_string());
                    }
                }
            }
            report
        })
        .expect("spawn the reader thread")
}
