// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Frames and the primitive values inside them.
//!
//! A frame is a 32-bit little-endian length followed by that many bytes. Inside a
//! frame, integers are little-endian and fixed-size; text and byte strings are a
//! 32-bit length and their bytes; an optional value is a byte 0 or 1 and the value;
//! a list is a 32-bit count and its items. Decoding checks every length against the
//! bytes that are left, so a malformed frame is an error, never a panic or a large
//! allocation.

use std::io::{self, Read, Write};

/// The largest frame either side accepts, in bytes.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The frame ended in the middle of a value.
    #[error("the message ends early")]
    Truncated,
    /// Bytes were left after the message.
    #[error("{0} bytes left after the message")]
    TrailingBytes(usize),
    /// A tag byte has a value this version does not know.
    #[error("unknown {what} {value}")]
    UnknownTag {
        /// What the byte selects.
        what: &'static str,
        /// The value found.
        value: u8,
    },
    /// Text that is not UTF-8.
    #[error("text that is not UTF-8")]
    NotUtf8,
    /// A value outside its range, such as a zero rate.
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// Writes one frame holding `payload`.
pub fn write_frame(out: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {} bytes is too large", payload.len()),
        ));
    }
    let length = u32::try_from(payload.len()).map_err(io::Error::other)?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

/// Reads one frame. `Ok(None)` at a clean end of the stream, before a frame starts.
pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0_u8; 4];
    let mut filled = 0;
    while filled < length.len() {
        match input.read(&mut length[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let length = usize::try_from(u32::from_le_bytes(length)).map_err(io::Error::other)?;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {length} bytes is too large"),
        ));
    }
    let mut payload = vec![0_u8; length];
    input.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// Builds a frame's payload.
#[derive(Debug, Default)]
pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn i16(&mut self, value: i16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn f64(&mut self, value: f64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// A count or length. Lengths above `u32::MAX` cannot occur in a frame, which is
    /// at most [`MAX_FRAME_BYTES`]; they are clamped and then fail to decode.
    pub(crate) fn len(&mut self, len: usize) {
        self.u32(u32::try_from(len).unwrap_or(u32::MAX));
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn str(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    pub(crate) fn opt<T>(&mut self, value: Option<T>, mut item: impl FnMut(&mut Self, T)) {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1);
                item(self, value);
            }
        }
    }

    pub(crate) fn opt_str(&mut self, value: Option<&str>) {
        self.opt(value, Self::str);
    }

    pub(crate) fn opt_u32(&mut self, value: Option<u32>) {
        self.opt(value, Self::u32);
    }

    pub(crate) fn list<T>(&mut self, items: &[T], mut item: impl FnMut(&mut Self, &T)) {
        self.len(items.len());
        for value in items {
            item(self, value);
        }
    }

    pub(crate) fn strs(&mut self, items: &[String]) {
        self.list(items, |e, s| e.str(s));
    }
}

/// Reads a frame's payload.
#[derive(Debug)]
pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Fails unless every byte was read.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes(self.bytes.len()))
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0_u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(DecodeError::UnknownTag {
                what: "boolean",
                value,
            }),
        }
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        self.array().map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        self.array().map(u32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        self.array().map(u64::from_le_bytes)
    }

    pub(crate) fn i16(&mut self) -> Result<i16, DecodeError> {
        self.array().map(i16::from_le_bytes)
    }

    pub(crate) fn f64(&mut self) -> Result<f64, DecodeError> {
        self.array().map(f64::from_le_bytes)
    }

    fn len(&mut self) -> Result<usize, DecodeError> {
        usize::try_from(self.u32()?).map_err(|_| DecodeError::Truncated)
    }

    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.len()?;
        self.take(n)
    }

    pub(crate) fn string(&mut self) -> Result<String, DecodeError> {
        std::str::from_utf8(self.bytes()?)
            .map(str::to_owned)
            .map_err(|_| DecodeError::NotUtf8)
    }

    pub(crate) fn opt<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => item(self).map(Some),
            value => Err(DecodeError::UnknownTag {
                what: "option",
                value,
            }),
        }
    }

    pub(crate) fn opt_string(&mut self) -> Result<Option<String>, DecodeError> {
        self.opt(Self::string)
    }

    pub(crate) fn opt_u32(&mut self) -> Result<Option<u32>, DecodeError> {
        self.opt(Self::u32)
    }

    pub(crate) fn list<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = self.len()?;
        // Every item takes at least one byte, so a count above the bytes left is a lie;
        // checking it first keeps the allocation bounded by the frame.
        if count > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(item(self)?);
        }
        Ok(items)
    }

    pub(crate) fn strings(&mut self) -> Result<Vec<String>, DecodeError> {
        self.list(Self::string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let mut stream = Vec::new();
        write_frame(&mut stream, b"one").unwrap();
        write_frame(&mut stream, b"").unwrap();
        let mut input = stream.as_slice();
        assert_eq!(
            read_frame(&mut input).unwrap().as_deref(),
            Some(&b"one"[..])
        );
        assert_eq!(read_frame(&mut input).unwrap().as_deref(), Some(&b""[..]));
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn a_cut_frame_is_an_error() {
        let mut stream = Vec::new();
        write_frame(&mut stream, b"hello").unwrap();
        for cut in 1..stream.len() {
            assert!(read_frame(&mut &stream[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn an_oversized_length_is_refused_before_allocating() {
        let stream = u32::MAX.to_le_bytes();
        let error = read_frame(&mut &stream[..]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn values_round_trip() {
        let mut e = Encoder::default();
        e.u8(7);
        e.bool(true);
        e.u16(0xbeef);
        e.u32(123_456);
        e.u64(u64::MAX);
        e.i16(-3);
        e.f64(-1.5);
        e.bytes(b"\xff\x00");
        e.str("héllo");
        e.opt_str(None);
        e.opt_u32(Some(9));
        e.strs(&["a".to_owned(), "b".to_owned()]);
        let bytes = e.finish();
        let mut d = Decoder::new(&bytes);
        assert_eq!(d.u8().unwrap(), 7);
        assert!(d.bool().unwrap());
        assert_eq!(d.u16().unwrap(), 0xbeef);
        assert_eq!(d.u32().unwrap(), 123_456);
        assert_eq!(d.u64().unwrap(), u64::MAX);
        assert_eq!(d.i16().unwrap(), -3);
        assert_eq!(d.f64().unwrap(), -1.5);
        assert_eq!(d.bytes().unwrap(), b"\xff\x00");
        assert_eq!(d.string().unwrap(), "héllo");
        assert_eq!(d.opt_string().unwrap(), None);
        assert_eq!(d.opt_u32().unwrap(), Some(9));
        assert_eq!(d.strings().unwrap(), ["a", "b"]);
        d.finish().unwrap();
    }

    #[test]
    fn lying_counts_are_refused() {
        let mut e = Encoder::default();
        e.u32(1_000_000);
        let bytes = e.finish();
        assert_eq!(
            Decoder::new(&bytes).list(Decoder::u8),
            Err(DecodeError::Truncated)
        );
        assert_eq!(Decoder::new(&bytes).bytes(), Err(DecodeError::Truncated));
    }
}
