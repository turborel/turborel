//! \[EXTENSION\] LZ4 compression for turborel, as an encoding (RULES.md
//! D30, D48), on lz4_flex's safe block format: registered with a database
//! (`Database::register_encoding`), it is named by STORAGE.ENCODINGS for
//! the database's files, or by a table's option ENCODINGS for its rows.
//!
//! ```text
//! encoded:  STORED | the input as it is, if compressing would not shrink it
//!         | COMPRESSED | its length, u32, little-endian | an LZ4 block
//! ```
//!
//! A block decoded must give exactly the length it records, which may be
//! at most `MAX_RATIO` times the block's, so a damaged one is refused
//! (SQLCODE -916) before memory is taken for it.

use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::{Code, Result, SqlError};

/// \[EXTENSION\] Its id in file formats: never changed or reused (RULES.md
/// D48).
pub const LZ4_ID: EncodingId = EncodingId(1);

/// The tag of input kept as it is.
const STORED: u8 = 0;
/// The tag of input compressed.
const COMPRESSED: u8 = 1;
/// Bytes of the length before a block.
const LENGTH: usize = 4;
/// \[DESIGN\] The most an LZ4 block expands: 255 bytes for each of its own,
/// which the format cannot pass.
const MAX_RATIO: usize = 255;

/// \[EXTENSION\] LZ4 compression, named "LZ4".
#[derive(Debug, Clone, Copy, Default)]
pub struct Lz4;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[EXTENSION] LZ4 data is malformed: {why}"))
}

impl Encoding for Lz4 {
    fn id(&self) -> EncodingId {
        LZ4_ID
    }

    fn name(&self) -> &'static str {
        "LZ4"
    }

    /// Input kept as it is, behind its tag: a block is kept only if it is
    /// shorter than the input.
    fn overhead(&self, _: &str) -> usize {
        1
    }

    fn encode(&self, _context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let block = lz4_flex::block::compress(input);
        let shrunk = block.len().saturating_add(LENGTH) < input.len();
        if let Some(length) = u32::try_from(input.len()).ok().filter(|_| shrunk) {
            out.push(COMPRESSED);
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&block);
        } else {
            out.push(STORED);
            out.extend_from_slice(input);
        }
        Ok(())
    }

    fn decode(&self, _context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        match input.split_first() {
            Some((&STORED, stored)) => out.extend_from_slice(stored),
            Some((&COMPRESSED, rest)) => {
                let (length, block) = rest.split_first_chunk::<LENGTH>().ok_or_else(|| corrupt("it is too short"))?;
                let length = usize::try_from(u32::from_le_bytes(*length)).map_err(|_| corrupt("its length is out of range"))?;
                if length > block.len().saturating_mul(MAX_RATIO) {
                    return Err(corrupt("its length is more than its block can hold"));
                }
                let start = out.len();
                out.resize(start.saturating_add(length), 0);
                let target = out.get_mut(start..).unwrap_or_default();
                let written = lz4_flex::block::decompress_into(block, target);
                if written.as_ref().ok() != Some(&length) {
                    out.truncate(start);
                    return Err(
                        written.map_or_else(|e| corrupt(&e.to_string()), |_| corrupt("its block does not give the length it records"))
                    );
                }
            }
            Some(_) | None => return Err(corrupt("it has no tag of this format")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
