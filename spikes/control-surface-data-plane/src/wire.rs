//! The byte encodings every message of the spike shares. Throwaway, never
//! product code.
//!
//! Little-endian integers and floats, a 32-bit count before a sequence or a
//! byte string. A reader returns `None` past the end instead of panicking, so
//! that a malformed message becomes a structured error at the surface.

pub fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

pub fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

pub fn put_text(out: &mut Vec<u8>, text: &str) {
    put_bytes(out, text.as_bytes());
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.remaining() < count {
            return None;
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Some(slice)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|bytes| u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.take(4).map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let count = self.u32()? as usize;
        self.take(count)
    }

    pub fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_a_short_message_reads_as_none() {
        let mut out = Vec::new();
        put_u8(&mut out, 7);
        put_u32(&mut out, 0xdead_beef);
        put_u64(&mut out, u64::MAX - 1);
        put_f32(&mut out, -1.5);
        put_text(&mut out, "sequence_gap");
        let mut reader = Reader::new(&out);
        assert_eq!(reader.u8(), Some(7));
        assert_eq!(reader.u32(), Some(0xdead_beef));
        assert_eq!(reader.u64(), Some(u64::MAX - 1));
        assert_eq!(reader.f32(), Some(-1.5));
        assert_eq!(reader.text().as_deref(), Some("sequence_gap"));
        assert_eq!(reader.remaining(), 0);
        assert_eq!(reader.u8(), None);
        let mut short = Reader::new(&out[..3]);
        assert_eq!(short.u8(), Some(7));
        assert_eq!(short.u32(), None);
    }
}
