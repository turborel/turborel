//! \[DESIGN\] A block file written and read as a stream of bytes (RULES.md
//! D30, D161), in the format of `format.rs`, version 1: its header, then
//! its blocks one after another. A backup's objects go to a target that
//! keeps bytes, not files at offsets, so they are made here and read back
//! here, each block checked against its digest and decoded through the
//! encodings its header names.

use std::path::Path;
use std::sync::Arc;

use super::super::encoding::{Encoding, Pipeline};
use super::DEFAULT_MAX_BLOCK;
use super::format::{self, HEADER_BYTES, LEN_BYTES, le_u32};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;

/// \[DESIGN\] What makes a block stream: the hash function every digest is
/// made with and the encodings every payload passes through.
#[derive(Clone)]
pub struct BlockWriter {
    hash: Arc<dyn HashFunction>,
    pipeline: Pipeline,
}

impl BlockWriter {
    /// A stream checksummed with `hash`, its payloads encoded by `pipeline`.
    pub fn new(hash: Arc<dyn HashFunction>, pipeline: Pipeline) -> BlockWriter {
        BlockWriter { hash, pipeline }
    }

    /// The header the stream begins with.
    pub fn header(&self) -> Vec<u8> {
        format::header(&*self.hash, &self.pipeline)
    }

    /// The bytes of a block holding `payload`, encoded.
    ///
    /// # Errors
    ///
    /// An encoding's; SQLCODE -910 if the payload encoded is longer than
    /// `DEFAULT_MAX_BLOCK`.
    pub fn block(&self, payload: &[u8]) -> Result<Vec<u8>> {
        // Context 0, as a block file's are (`BlockFile::append`).
        let stored = self.pipeline.encode(0, payload)?;
        format::frame(&*self.hash, &stored, DEFAULT_MAX_BLOCK)
    }
}

/// The hash functions and encodings a reader may find a stream written
/// with.
pub type Available<'a> = (&'a [Arc<dyn HashFunction>], &'a [Arc<dyn Encoding>]);

/// \[DESIGN\] What reads a block stream back: the hash function and
/// encodings its header named.
pub struct BlockReader {
    hash: Arc<dyn HashFunction>,
    pipeline: Pipeline,
}

/// Bytes of a stream's header.
pub const STREAM_HEADER: usize = HEADER_BYTES;

impl BlockReader {
    /// The reader of the stream whose first `STREAM_HEADER` bytes are
    /// `header`, named `name` in messages, with the hash functions
    /// `hashes` and encodings `encodings` available.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the header is not a block file's or fails its
    /// digest; -910 if a function or encoding it names is not available,
    /// or a key it names is not in the keyring.
    pub fn new(header: &[u8], name: &Path, (hashes, encodings): Available<'_>) -> Result<BlockReader> {
        let (hash, ids) = format::read_header(header, hashes, name)?;
        let pipeline = Pipeline::resolve(&ids, encodings, name)?;
        Ok(BlockReader { hash, pipeline })
    }

    /// The block at the start of `bytes`, decoded, and the bytes it takes;
    /// none if `bytes` holds less than the whole block.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the block fails its digest or is malformed; an
    /// encoding's, as a decryption that fails to authenticate.
    pub fn next(&self, bytes: &[u8]) -> Result<Option<(Vec<u8>, usize)>> {
        let Some(len) = le_u32(bytes, 0) else { return Ok(None) };
        if len > DEFAULT_MAX_BLOCK {
            return Err(SqlError::new(Code::Corrupt, format!("[DESIGN] RULES.md D161: a block claims {len} bytes, past the limit")));
        }
        let payload = usize::try_from(len).unwrap_or(usize::MAX);
        let digest_len = self.hash.digest_len();
        let whole = format::frame_len(payload, digest_len);
        if bytes.len() < whole {
            return Ok(None);
        }
        let prefix = LEN_BYTES.saturating_add(digest_len);
        let digest = bytes.get(LEN_BYTES..prefix).unwrap_or_default();
        let rest = bytes.get(prefix..whole).unwrap_or_default();
        if !format::verify(&*self.hash, len, digest, rest) {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] RULES.md D161: a block fails its digest"));
        }
        let decoded = self.pipeline.decode(0, rest.get(..payload).unwrap_or_default())?;
        Ok(Some((decoded.into_owned(), whole)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::crc::{CRC32_ISCSI, Crc};

    fn crc() -> Arc<dyn HashFunction> {
        Arc::new(Crc::new(CRC32_ISCSI).unwrap())
    }

    /// Verifies: [DESIGN] RULES.md D161, a block stream: blocks written
    /// after a header are read back in order, a partial block asks for
    /// more, and a flipped byte fails its digest.
    #[test]
    fn blocks_round_trip_and_damage_is_found() {
        let writer = BlockWriter::new(crc(), Pipeline::none());
        let mut bytes = writer.header();
        bytes.extend(writer.block(b"one").unwrap());
        bytes.extend(writer.block(b"second").unwrap());
        let reader = BlockReader::new(&bytes[..STREAM_HEADER], Path::new("t"), (&[crc()], &[])).unwrap();
        let rest = &bytes[STREAM_HEADER..];
        let (first, used) = reader.next(rest).unwrap().unwrap();
        assert_eq!(first, b"one");
        assert!(reader.next(&rest[used..used + 3]).unwrap().is_none());
        let (second, _) = reader.next(&rest[used..]).unwrap().unwrap();
        assert_eq!(second, b"second");
        let mut damaged = rest.to_vec();
        damaged[used + 9] ^= 1;
        assert_eq!(reader.next(&damaged[used..]).err().map(|e| e.code), Some(Code::Corrupt));
    }
}
