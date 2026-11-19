//! \[EXTENSION\] zlib (RFC 1950) over DEFLATE (RFC 1951), written in the
//! tree (RULES.md D193): a two-byte header, a DEFLATE stream, and the
//! Adler-32 of the data, most significant byte first. Pure, with no I/O
//! and no state kept between calls, so any protocol may compress a whole
//! message with it.
//!
//! `compress` writes LZ77 with hash chains and dynamic Huffman blocks, or
//! stored blocks where those come out smaller. `decompress` reads every
//! stream RFC 1950 allows except one with a preset dictionary, and stops
//! at a limit its caller gives, so a decompression bomb is refused once
//! it passes the limit rather than held.

use turborel::error::{Code, Result, SqlError};

pub mod adler;
mod deflate;
mod inflate;
mod tables;

pub use adler::adler32;

/// CMF: compression method 8 (deflate), CINFO 7 (a 32K window).
const CMF: u8 = 0x78;
/// FLG of FLEVEL 2, the default, with FCHECK making `CMF FLG` a multiple
/// of 31.
const FLG: u8 = 0x9C;
/// The compression method deflate, CMF's low four bits.
const DEFLATE: u8 = 8;
/// The largest CINFO, the base-2 logarithm of the window less 8.
const MAX_CINFO: u8 = 7;
/// FLG's preset dictionary bit.
const FDICT: u8 = 0x20;
/// The divisor FCHECK makes the header a multiple of.
const FCHECK_DIVISOR: u16 = 31;
/// The bytes of the header and of the Adler-32 trailer.
const HEADER_BYTES: usize = 2;
const TRAILER_BYTES: usize = 4;

/// `data` as a zlib stream.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let body = deflate::deflate(data);
    let mut out = Vec::with_capacity(body.len().saturating_add(HEADER_BYTES).saturating_add(TRAILER_BYTES));
    out.extend_from_slice(&[CMF, FLG]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// The data of the zlib stream `stream`, which must be all of it, at most
/// `limit` bytes.
///
/// # Errors
///
/// SQLCODE -916 for a stream not of RFC 1950 and 1951, one with a preset
/// dictionary, a wrong Adler-32 or bytes after it; SQLCODE -918 for one
/// that inflates past `limit` (`is_too_large`).
pub fn decompress(stream: &[u8], limit: usize) -> Result<Vec<u8>> {
    let (cmf, flg) = match stream {
        [cmf, flg, ..] => (*cmf, *flg),
        _ => return Err(malformed("the stream is shorter than its header")),
    };
    if cmf & 0x0F != DEFLATE || cmf >> 4_u8 > MAX_CINFO {
        return Err(malformed("the stream's compression method is not deflate"));
    }
    if u16::from_be_bytes([cmf, flg]).checked_rem(FCHECK_DIVISOR) != Some(0) {
        return Err(malformed("the stream's header check fails"));
    }
    if flg & FDICT != 0 {
        return Err(malformed("the stream needs a preset dictionary"));
    }
    let mut bits = inflate::Bits::new(stream.get(HEADER_BYTES..).unwrap_or_default());
    let data = inflate::inflate(&mut bits, limit)?;
    let trailer =
        <[u8; TRAILER_BYTES]>::try_from(bits.rest()).map_err(|_| malformed("the stream's Adler-32 is missing or followed by bytes"))?;
    if u32::from_be_bytes(trailer) != adler32(&data) {
        return Err(malformed("the stream's Adler-32 does not match its data"));
    }
    Ok(data)
}

/// Whether `e` is `decompress` refusing a stream past its limit.
pub fn is_too_large(e: &SqlError) -> bool {
    e.sqlcode() == SqlError::new(Code::Resources, "").sqlcode()
}

/// A stream not of its format: SQLCODE -916.
fn malformed(what: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] zlib: {what}"))
}

/// A stream inflating past `limit`: SQLCODE -918.
fn too_large(limit: usize) -> SqlError {
    SqlError::new(Code::Resources, format!("[DESIGN] zlib: the data inflates past the {limit} bytes taken"))
}

#[cfg(test)]
mod tests;
