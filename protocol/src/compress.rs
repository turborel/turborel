//! \[DESIGN\] Compressed frames (RULES.md D196): once the capability
//! `compression` is granted, either side may send any frame of at least a
//! threshold's bytes compressed, as a frame of kind `z` whose body is the
//! frame's kind, its body's length, and its body as a zlib stream
//! (`zlib.rs`). A frame that would come out no shorter is sent as it is.
//! The reader inflates at most the length declared, itself at most the
//! longest body it takes, so a stream that inflates past it is refused
//! before it is held.
//!
//! ```text
//! 'z' compressed: kind u8, length u32, then the zlib stream of the body
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::Result;

use crate::{HEADER, broken, frame, zlib};

/// The kind of a compressed frame, the same both ways.
pub const COMPRESSED: u8 = b'z';
/// \[DESIGN\] The default threshold: a frame shorter than this is never
/// compressed, as the stream's header, its trailer and the time taken
/// outweigh what it saves.
pub const DEFAULT_THRESHOLD: usize = 1024;
/// The bytes a compressed frame's body takes before its stream: the kind
/// and the length.
const PREFIX: usize = 5;

/// The frames of `frames`, each of at least `threshold` bytes compressed
/// where that makes it shorter, appended to `out`.
pub fn compress_frames(out: &mut Vec<u8>, mut frames: &[u8], threshold: usize) {
    while let Some((head, rest)) = frames.split_first_chunk::<HEADER>() {
        let [kind, length @ ..] = *head;
        let length = usize::try_from(u32::from_le_bytes(length)).unwrap_or(usize::MAX).min(rest.len());
        let (body, after) = rest.split_at(length);
        let whole = HEADER.saturating_add(length);
        let stream = (whole >= threshold).then(|| zlib::compress(body)).filter(|s| s.len().saturating_add(PREFIX) < length);
        match stream {
            Some(stream) => frame(out, COMPRESSED, |b| {
                b.put_u8(kind);
                b.put_u32(u32::try_from(length).unwrap_or(u32::MAX));
                b.extend_from_slice(&stream);
            }),
            None => out.extend_from_slice(frames.get(..whole).unwrap_or_default()),
        }
        frames = after;
    }
}

/// The kind and body of the frame the compressed frame's body `body`
/// holds: its length at most `max`, inflated no further.
///
/// # Errors
///
/// A connection failure if it is malformed, holds another compressed
/// frame, declares more than `max`, or inflates to other than it declares.
pub fn decompress(body: &[u8], max: u32) -> Result<(u8, Vec<u8>)> {
    let mut r = Reader::received(body, "compressed frame");
    let (kind, length) = (r.u8()?, r.u32()?);
    if kind == COMPRESSED {
        return Err(broken("[DESIGN] a compressed frame holds another"));
    }
    if length > max {
        return Err(broken(format!("[DESIGN] a compressed message of {length} bytes is longer than the {max} taken")));
    }
    let length = usize::try_from(length).map_err(|_| broken("[DESIGN] a message is too long for this machine"))?;
    let inflated = zlib::decompress(r.rest(), length).map_err(|e| broken(format!("[DESIGN] a compressed frame: {}", e.msg)))?;
    if inflated.len() != length {
        return Err(broken("[DESIGN] a compressed frame inflates to another length than it declares"));
    }
    Ok((kind, inflated))
}
