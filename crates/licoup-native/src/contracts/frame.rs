//! Shared streaming frame Layer for length-delimited stdio protocol frames.
//!
//! This is the L2/L4-common layer: raw bytes in, complete newline-terminated
//! frames out. Payload typing (method commands, delta envelopes) is the
//! responsibility of the codegen Payload Layer in [`crate::contracts::conversation_protocol`].
//!
//! `FrameDecoder` applies bounded streaming decode with natural backpressure:
//! it only pulls from the source through [`BufRead::fill_buf`] / [`BufRead::consume`],
//! holding at most one in-progress frame and discarding the payload of any frame
//! that exceeds `max_frame_bytes` (signalled once per oversized frame, without
//! losing the following frames).

use std::io::{self, BufRead};

/// The wire terminator between frames. Frames are one bounded JSON line.
pub const FRAME_TERMINATOR: u8 = b'\n';

/// A completed frame read from the source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// The source reached EOF before any further frame content.
    Eof,
    /// One complete frame, with the trailing terminator removed.
    Data(Vec<u8>),
    /// A frame exceeded `max_frame_bytes` and was discarded wholesale.
    TooLarge,
}

/// Streaming newline frame decoder shared by L2 (codegen instances) and L4
/// (per-agent manual parsers).
#[derive(Clone, Debug)]
pub struct FrameDecoder {
    max_frame_bytes: usize,
    pending: Vec<u8>,
    discarding: bool,
}

impl FrameDecoder {
    /// Create a decoder that rejects any single frame larger than
    /// `max_frame_bytes` (including the terminator).
    pub const fn new(max_frame_bytes: usize) -> Self {
        Self {
            max_frame_bytes,
            pending: Vec::new(),
            discarding: false,
        }
    }

    /// The bound applied to a single frame.
    pub const fn max_frame_bytes(&self) -> usize {
        self.max_frame_bytes
    }

    /// Decode the next frame from `reader`.
    ///
    /// Backpressure contract: this call consumes from `reader` only what is
    /// necessary to decide the next frame. An oversized frame is fully consumed
    /// but its payload is never retained, and the following frame remains
    /// decodeable.
    pub fn next_frame(&mut self, reader: &mut impl BufRead) -> io::Result<Frame> {
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if self.pending.is_empty() && !self.discarding {
                    return Ok(Frame::Eof);
                }
                break;
            }
            let newline = available.iter().position(|byte| *byte == FRAME_TERMINATOR);
            let consumed = newline.map_or(available.len(), |index| index + 1);
            if !self.discarding {
                if self.pending.len().saturating_add(consumed) > self.max_frame_bytes {
                    self.discarding = true;
                    self.pending.clear();
                } else {
                    self.pending.extend_from_slice(&available[..consumed]);
                }
            }
            reader.consume(consumed);
            if newline.is_some() {
                break;
            }
        }
        if self.discarding {
            self.discarding = false;
            return Ok(Frame::TooLarge);
        }
        while self
            .pending
            .last()
            .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
        {
            self.pending.pop();
        }
        let data = std::mem::take(&mut self.pending);
        Ok(Frame::Data(data))
    }
}

/// Bounded frame encoder with atomic write semantics: the frame is fully
/// serialized by the caller and flushed as one bounded unit or not at all.
pub fn write_frame(
    writer: &mut impl io::Write,
    frame: &[u8],
    max_frame_bytes: usize,
) -> io::Result<bool> {
    if frame.len().saturating_add(1) > max_frame_bytes {
        return Ok(false);
    }
    writer.write_all(frame)?;
    writer.write_all(&[FRAME_TERMINATOR])?;
    writer.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_ordered_frames_and_preserves_terminators() {
        let mut decoder = FrameDecoder::new(16);
        let mut reader = Cursor::new(b"{\"a\":1}\n{}\n".to_vec());

        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"{\"a\":1}".to_vec())
        );
        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"{}".to_vec())
        );
        assert_eq!(decoder.next_frame(&mut reader).unwrap(), Frame::Eof);
    }

    #[test]
    fn strips_carriage_returns() {
        let mut decoder = FrameDecoder::new(16);
        let mut reader = Cursor::new(b"{}\r\n".to_vec());

        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"{}".to_vec())
        );
    }

    #[test]
    fn jumps_oversized_frame_without_losing_the_next_request() {
        let mut decoder = FrameDecoder::new(4);
        let mut reader = Cursor::new(b"12345\n{}\n".to_vec());

        assert_eq!(decoder.next_frame(&mut reader).unwrap(), Frame::TooLarge);
        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"{}".to_vec())
        );
        assert_eq!(decoder.next_frame(&mut reader).unwrap(), Frame::Eof);
    }

    #[test]
    fn splits_frames_across_chunks() {
        let mut decoder = FrameDecoder::new(16);
        let mut reader = ChunkReader::new(b"hello\nworld!\nbye".to_vec(), 3);

        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"hello".to_vec())
        );
        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"world!".to_vec())
        );
        // An unterminated final tail at EOF is delivered exactly once and the
        // following call reports Eof.
        assert_eq!(
            decoder.next_frame(&mut reader).unwrap(),
            Frame::Data(b"bye".to_vec())
        );
        assert_eq!(decoder.next_frame(&mut reader).unwrap(), Frame::Eof);
    }

    struct ChunkReader {
        data: Vec<u8>,
        offset: usize,
        chunk: usize,
    }

    impl ChunkReader {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Self {
                data,
                offset: 0,
                chunk,
            }
        }
    }

    impl std::io::Read for ChunkReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let remaining = &self.data[self.offset..];
            let take = remaining.len().min(buffer.len()).min(self.chunk);
            buffer[..take].copy_from_slice(&remaining[..take]);
            self.offset += take;
            Ok(take)
        }
    }

    impl BufRead for ChunkReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            let end = (self.offset + self.chunk).min(self.data.len());
            Ok(&self.data[self.offset..end])
        }

        fn consume(&mut self, amount: usize) {
            self.offset = (self.offset + amount).min(self.data.len());
        }
    }

    #[test]
    fn encoder_is_bounded_and_atomic() {
        let mut output = Vec::new();
        assert!(write_frame(&mut output, b"{}", 4).unwrap());
        assert_eq!(output, b"{}\n");
        assert!(!write_frame(&mut output, b"{}", 2).unwrap());
        assert_eq!(output, b"{}\n");
    }
}
