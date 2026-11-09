//! The ring of frames inside a shared memory mapping. Throwaway, never
//! product code.
//!
//! One writer (the endpoint), one reader (the client), each through its own
//! view of the mapping. Three byte positions that only grow live in the
//! mapping as atomics: `write` (the writer's), `read` (the reader's
//! acknowledgement) and `floor` (the writer's, the oldest byte still valid).
//! The distance between them is the flow control: a lossless frame is written
//! only when it fits after the oldest unread byte; a latest-wins frame may
//! raise the floor over the oldest frames, which the reader then never sees.
//!
//! A reader copies a frame out and only then checks that the floor has not
//! passed the frame's start: a frame overwritten while it was being copied is
//! discarded, never delivered torn.
//!
//! A frame is a 32-byte header (world, stream, tick, length, sequence) and the
//! payload, padded to eight bytes; frames wrap around the end of the data.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::shared_memory::Mapping;

const MAGIC: u64 = 0x4c44_4e52_494e_4731;
const WRITE_AT: usize = 64;
const READ_AT: usize = 128;
const FLOOR_AT: usize = 192;
/// The data starts on the second page; the positions each have their own cache line.
pub const DATA_AT: usize = 4096;
pub const HEADER_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub world: u32,
    pub stream: u32,
    pub tick: u32,
    pub length: u32,
    pub sequence: u64,
}

impl FrameHeader {
    pub fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0_u8; HEADER_BYTES];
        bytes[0..4].copy_from_slice(&self.world.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.stream.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.tick.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.length.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.sequence.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; HEADER_BYTES]) -> FrameHeader {
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        FrameHeader { world: u32_at(0), stream: u32_at(4), tick: u32_at(8), length: u32_at(12), sequence: u64::from_le_bytes(bytes[16..24].try_into().expect("eight bytes")) }
    }

    /// The bytes the frame takes in the ring or on a stream.
    pub fn frame_bytes(&self) -> u64 {
        HEADER_BYTES as u64 + u64::from(self.length).next_multiple_of(8)
    }
}

/// The mapping seen as a ring: shared by the writer's and the reader's halves.
struct Ring {
    mapping: Mapping,
    capacity: u64,
}

impl Ring {
    fn atomic(&self, at: usize) -> &AtomicU64 {
        // SAFETY: inside the first page of the mapping, which is page-aligned,
        // and only ever accessed as an atomic.
        unsafe { &*self.mapping.pointer().add(at).cast::<AtomicU64>() }
    }

    fn data(&self) -> *mut u8 {
        // SAFETY: the mapping is `DATA_AT + capacity` bytes long.
        unsafe { self.mapping.pointer().add(DATA_AT) }
    }

    fn copy_in(&self, position: u64, bytes: &[u8]) {
        let offset = (position % self.capacity) as usize;
        let first = bytes.len().min(self.capacity as usize - offset);
        // SAFETY: both parts lie inside the data; the reader discards what it
        // copied from a region the writer was allowed to write (see the module).
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.data().add(offset), first);
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(first), self.data(), bytes.len() - first);
        }
    }

    fn copy_out(&self, position: u64, bytes: &mut [u8]) {
        let offset = (position % self.capacity) as usize;
        let first = bytes.len().min(self.capacity as usize - offset);
        // SAFETY: as for `copy_in`.
        unsafe {
            std::ptr::copy_nonoverlapping(self.data().add(offset), bytes.as_mut_ptr(), first);
            std::ptr::copy_nonoverlapping(self.data(), bytes.as_mut_ptr().add(first), bytes.len() - first);
        }
    }

    fn header_at(&self, position: u64) -> FrameHeader {
        let mut bytes = [0_u8; HEADER_BYTES];
        self.copy_out(position, &mut bytes);
        FrameHeader::decode(&bytes)
    }

    fn oldest(&self) -> u64 {
        self.atomic(READ_AT).load(Ordering::SeqCst).max(self.atomic(FLOOR_AT).load(Ordering::SeqCst))
    }
}

fn capacity_of(mapping: &Mapping) -> io::Result<u64> {
    let capacity = mapping.length().checked_sub(DATA_AT).unwrap_or(0) as u64;
    if capacity < 4096 || capacity % 8 != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a ring needs a header page and a data size that is a multiple of eight"));
    }
    Ok(capacity)
}

pub struct RingWriter {
    ring: Ring,
    write: u64,
}

impl RingWriter {
    /// Lays a ring out in a fresh, zero-filled mapping.
    pub fn create(mapping: Mapping) -> io::Result<RingWriter> {
        let capacity = capacity_of(&mapping)?;
        let ring = Ring { mapping, capacity };
        // SAFETY: the first sixteen bytes of the mapping, written before any peer reads them.
        unsafe {
            ring.mapping.pointer().cast::<u64>().write(MAGIC);
            ring.mapping.pointer().add(8).cast::<u64>().write(capacity);
        }
        Ok(RingWriter { ring, write: 0 })
    }

    #[cfg(test)]
    pub fn mapping(&self) -> &Mapping {
        &self.ring.mapping
    }

    /// Writes one frame if it fits after the oldest unread byte. With
    /// `overwrite`, makes room by raising the floor over the oldest frames
    /// while `evictable` allows it for their stream. Returns whether the
    /// frame was written, and the number of frames evicted for it.
    pub fn write(&mut self, header: &FrameHeader, payload: &[u8], overwrite: bool, evictable: impl Fn(u32) -> bool) -> (bool, u32) {
        debug_assert_eq!(header.length as usize, payload.len());
        let needed = header.frame_bytes();
        if needed > self.ring.capacity {
            return (false, 0);
        }
        let mut evicted = 0;
        loop {
            let oldest = self.ring.oldest();
            if self.ring.capacity - (self.write - oldest) >= needed {
                break;
            }
            if !overwrite {
                return (false, evicted);
            }
            // The frame at `oldest` is ours and intact: the floor is below it.
            let victim = self.ring.header_at(oldest);
            if !evictable(victim.stream) {
                return (false, evicted);
            }
            self.ring.atomic(FLOOR_AT).fetch_max(oldest + victim.frame_bytes(), Ordering::SeqCst);
            fence(Ordering::SeqCst);
            evicted += 1;
        }
        self.ring.copy_in(self.write, &header.encode());
        self.ring.copy_in(self.write + HEADER_BYTES as u64, payload);
        self.write += needed;
        self.ring.atomic(WRITE_AT).store(self.write, Ordering::SeqCst);
        (true, evicted)
    }
}

pub struct RingReader {
    ring: Ring,
    /// Frames discarded because they were overwritten while being copied.
    pub torn_reads: u64,
}

impl RingReader {
    /// Attaches to a ring the peer laid out in the object this mapping views.
    pub fn attach(mapping: Mapping) -> io::Result<RingReader> {
        let capacity = capacity_of(&mapping)?;
        // SAFETY: the first sixteen bytes of the mapping.
        let (magic, declared) = unsafe { (mapping.pointer().cast::<u64>().read(), mapping.pointer().add(8).cast::<u64>().read()) };
        if magic != MAGIC || declared != capacity {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the mapping holds no ring of this size"));
        }
        Ok(RingReader { ring: Ring { mapping, capacity }, torn_reads: 0 })
    }

    /// Copies the oldest unread frame's payload into `payload` and
    /// acknowledges it; `None` when the ring holds no unread frame.
    pub fn read(&mut self, payload: &mut Vec<u8>) -> Option<FrameHeader> {
        loop {
            let start = self.ring.oldest();
            let write = self.ring.atomic(WRITE_AT).load(Ordering::SeqCst);
            if start >= write {
                return None;
            }
            let header = self.ring.header_at(start);
            let plausible = header.frame_bytes() <= write - start;
            if plausible {
                payload.resize(header.length as usize, 0);
                self.ring.copy_out(start + HEADER_BYTES as u64, payload);
            }
            fence(Ordering::SeqCst);
            if self.ring.atomic(FLOOR_AT).load(Ordering::SeqCst) > start {
                self.torn_reads += 1;
                continue;
            }
            assert!(plausible, "a frame below the write position and above the floor is whole");
            self.ring.atomic(READ_AT).store(start + header.frame_bytes(), Ordering::SeqCst);
            return Some(header);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(capacity: usize) -> (RingWriter, RingReader) {
        let writer = RingWriter::create(Mapping::create(DATA_AT + capacity).unwrap()).unwrap();
        let reader = RingReader::attach(writer.mapping().second_view().unwrap()).unwrap();
        (writer, reader)
    }

    fn frame(stream: u32, sequence: u64, length: usize) -> (FrameHeader, Vec<u8>) {
        let payload: Vec<u8> = (0..length).map(|i| (i as u64 * 31 + sequence * 7) as u8).collect();
        (FrameHeader { world: 1, stream, tick: sequence as u32, length: length as u32, sequence }, payload)
    }

    #[test]
    fn frames_wrap_around_the_end_and_read_back_in_order() {
        let (mut writer, mut reader) = pair(4096);
        let mut buffer = Vec::new();
        assert_eq!(reader.read(&mut buffer), None);
        for sequence in 1..=200_u64 {
            let (header, payload) = frame(1, sequence, 100 + (sequence as usize * 37) % 900);
            assert_eq!(writer.write(&header, &payload, false, |_| false), (true, 0), "an emptied ring has room");
            assert_eq!(reader.read(&mut buffer), Some(header));
            assert_eq!(buffer, payload);
        }
        assert_eq!(reader.read(&mut buffer), None);
        assert_eq!(reader.torn_reads, 0);
    }

    #[test]
    fn a_lossless_frame_waits_for_room_and_loses_nothing() {
        let (mut writer, mut reader) = pair(4096);
        let mut written = 0_u64;
        loop {
            let (header, payload) = frame(1, written + 1, 500);
            if !writer.write(&header, &payload, false, |_| false).0 {
                break;
            }
            written += 1;
        }
        assert_eq!(written, 7, "seven frames of 536 bytes fit in 4096");
        let mut buffer = Vec::new();
        assert_eq!(reader.read(&mut buffer).unwrap().sequence, 1);
        let (header, payload) = frame(1, 8, 500);
        assert!(writer.write(&header, &payload, false, |_| false).0, "one frame read makes room for one");
        let sequences: Vec<u64> = std::iter::from_fn(|| reader.read(&mut buffer).map(|header| header.sequence)).collect();
        assert_eq!(sequences, [2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(buffer, payload);
    }

    #[test]
    fn a_latest_wins_frame_overwrites_the_oldest_and_spares_another_policy() {
        let (mut writer, mut reader) = pair(4096);
        for sequence in 1..=20_u64 {
            let (header, payload) = frame(2, sequence, 500);
            assert!(writer.write(&header, &payload, true, |stream| stream == 2).0);
        }
        let mut buffer = Vec::new();
        let sequences: Vec<u64> = std::iter::from_fn(|| reader.read(&mut buffer).map(|header| header.sequence)).collect();
        assert_eq!(sequences, [14, 15, 16, 17, 18, 19, 20], "the newest seven remain");
        assert_eq!(buffer, frame(2, 20, 500).1);

        let (lossless, payload) = frame(1, 1, 500);
        assert!(writer.write(&lossless, &payload, false, |_| false).0);
        let mut refused = 0;
        for sequence in 21..=40_u64 {
            let (header, payload) = frame(2, sequence, 500);
            if !writer.write(&header, &payload, true, |stream| stream == 2).0 {
                refused += 1;
            }
        }
        assert_eq!(refused, 14, "six fit beside the lossless frame, which is never evicted");
        assert_eq!(reader.read(&mut buffer), Some(lossless));
    }

    #[test]
    fn a_frame_larger_than_the_ring_is_refused_and_a_foreign_mapping_is_not_a_ring() {
        let (mut writer, _reader) = pair(4096);
        let (header, payload) = frame(1, 1, 5000);
        assert_eq!(writer.write(&header, &payload, true, |_| true), (false, 0));
        assert!(RingReader::attach(Mapping::create(DATA_AT + 4096).unwrap()).is_err());
        assert!(RingWriter::create(Mapping::create(DATA_AT).unwrap()).is_err());
    }

    #[test]
    fn a_reader_racing_a_writer_that_overwrites_never_delivers_a_torn_frame() {
        let (mut writer, mut reader) = pair(8192);
        let frames = 200_000_u64;
        let producer = std::thread::spawn(move || {
            for sequence in 1..=frames {
                let length = 64 + (sequence as usize % 13) * 96;
                let payload = vec![sequence as u8; length];
                let header = FrameHeader { world: 1, stream: 2, tick: sequence as u32, length: length as u32, sequence };
                assert!(writer.write(&header, &payload, true, |_| true).0);
            }
        });
        let mut buffer = Vec::new();
        let mut last = 0;
        let mut received = 0_u64;
        while last < frames {
            let Some(header) = reader.read(&mut buffer) else {
                std::hint::spin_loop();
                continue;
            };
            assert!(header.sequence > last, "sequences only grow");
            assert_eq!(header.length as usize, 64 + (header.sequence as usize % 13) * 96);
            assert!(buffer.iter().all(|byte| *byte == header.sequence as u8), "frame {} is whole", header.sequence);
            last = header.sequence;
            received += 1;
        }
        producer.join().unwrap();
        assert!(received <= frames);
    }
}
