//! \[EXTENSION\] Equality of character large objects under a collation,
//! read a piece at a time (RULES.md D60, D79). A sort key holds all of a
//! string's weights, level after level, so the keys of a large object's
//! pieces are not pieces of its key. Each level's weights are: of a string
//! cut where its collation says it may be (`Collation::safe_cut`), the
//! weights of its pieces, joined. So each value is read in windows, each
//! window cut at its last safe place, the rest kept for the next, and each
//! level's weights of the two compared as they come, what they share let
//! go: two large objects are equal exactly when every level's weights are.
//! Under PAD SPACE trailing spaces are left off first, as every built-in
//! collation's sort key leaves them off.

use super::{CollationRef, Pad};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::lob::{CHUNK_OCTETS, Lob, LobReader, broken};

/// \[DESIGN\] The most octets a window grows to, looking for a place a
/// collation may cut a value at, before the comparison is refused: four
/// chunks, which no text of natural language goes without such a place.
const MOST_WINDOW: usize = 4 * CHUNK_OCTETS;

/// One value's side of a comparison: where it is read to, the text read
/// and not yet weighed, and each level's weights not yet compared.
struct Side<'l> {
    reader: LobReader<'l>,
    at: u64,
    end: u64,
    pending: Vec<u8>,
    weights: Vec<Vec<u8>>,
}

impl<'l> Side<'l> {
    /// The side of `lob`, read up to `end`, of `levels` levels.
    fn new(lob: &'l Lob, end: u64, levels: usize) -> Side<'l> {
        Side { reader: lob.reader(), at: 0, end, pending: Vec::new(), weights: vec![Vec::new(); levels] }
    }

    /// Whether all of it is read and weighed.
    fn done(&self) -> bool {
        self.at >= self.end && self.pending.is_empty()
    }

    /// Reads a window and weighs it to its last safe place, or all of it
    /// at the end.
    fn advance(&mut self, c: &CollationRef) -> Result<()> {
        loop {
            let want = u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX).min(self.end.saturating_sub(self.at));
            let piece = self.reader.read(self.at, want)?;
            self.at = self.at.saturating_add(u64::try_from(piece.len()).unwrap_or(u64::MAX));
            self.pending.extend_from_slice(&piece);
            let whole = self.at >= self.end;
            let text = utf8_prefix(&self.pending);
            let cut = if whole && text.len() == self.pending.len() { Some(text.len()) } else { c.collation.safe_cut(text) };
            match cut.filter(|&n| n > 0 || whole) {
                Some(n) => return self.weigh(c, n),
                None if self.pending.len() >= MOST_WINDOW => return Err(no_cut(c)),
                None if whole => return Err(broken("a character large object's octets are not UTF-8")),
                None => {}
            }
        }
    }

    /// Weighs the first `n` octets pending, at each level.
    fn weigh(&mut self, c: &CollationRef, n: usize) -> Result<()> {
        let head: Vec<u8> = self.pending.drain(..n.min(self.pending.len())).collect();
        let text = std::str::from_utf8(&head).map_err(|_| broken("a character large object's octets are not UTF-8"))?;
        for (level, out) in self.weights.iter_mut().enumerate() {
            c.collation.level_weights(text, level.saturating_add(1), out);
        }
        Ok(())
    }
}

/// The longest prefix of `bytes` that is whole UTF-8 characters.
fn utf8_prefix(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => std::str::from_utf8(bytes.get(..e.valid_up_to()).unwrap_or_default()).unwrap_or_default(),
    }
}

/// A value no place of which the collation may cut at.
fn no_cut(c: &CollationRef) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[DESIGN] a character large object has no place within {MOST_WINDOW} octets the collation {} may compare it in pieces at",
            c.collation.name()
        ),
    )
    .with_state(SqlState::FEATURE_NOT_SUPPORTED)
}

/// The octets of `lob` before its trailing spaces, found by reading from
/// its end a chunk at a time.
fn without_trailing_spaces(lob: &Lob) -> Result<u64> {
    let (mut end, window) = (lob.octets(), u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX));
    let mut reader = lob.reader();
    while end > 0 {
        let from = end.saturating_sub(window);
        let piece = reader.read(from, end.saturating_sub(from))?;
        let spaces = piece.iter().rev().take_while(|&&b| b == b' ').count();
        end = end.saturating_sub(u64::try_from(spaces).unwrap_or(u64::MAX));
        if spaces < piece.len() {
            break;
        }
    }
    Ok(end)
}

/// Drops what the two sides' weights share, at each level; whether they
/// still agree: neither holds a weight the other differs at.
fn settle(a: &mut Side<'_>, b: &mut Side<'_>) -> bool {
    for (x, y) in a.weights.iter_mut().zip(b.weights.iter_mut()) {
        let shared = x.iter().zip(y.iter()).take_while(|(p, q)| p == q).count();
        if shared < x.len().min(y.len()) {
            return false;
        }
        x.drain(..shared);
        y.drain(..shared);
    }
    true
}

/// [ISO/IEC 9075-2:2023 §8.2 GR3] Whether `a` and `b`, character large
/// objects, compare equal under `c`, read a piece at a time.
///
/// # Errors
///
/// As reading a chunk fails; SQLSTATE 0A000 if a value has no place the
/// collation may cut it at within `MOST_WINDOW` octets, as a collation
/// that does not say where has none.
pub fn lob_equal(a: &Lob, b: &Lob, c: &CollationRef) -> Result<bool> {
    let ends = match c.pad {
        Pad::Space => (without_trailing_spaces(a)?, without_trailing_spaces(b)?),
        Pad::None => (a.octets(), b.octets()),
    };
    let levels = c.collation.levels();
    let (mut left, mut right) = (Side::new(a, ends.0, levels), Side::new(b, ends.1, levels));
    loop {
        // The side behind, of fewer weights at the first level, reads on.
        let left_behind = left.weights.first().map_or(0, Vec::len) <= right.weights.first().map_or(0, Vec::len);
        let left_reads = match (left.done(), right.done()) {
            (true, true) => return Ok(settle(&mut left, &mut right) && left.weights.iter().chain(&right.weights).all(Vec::is_empty)),
            (false, true) => true,
            (true, false) => false,
            (false, false) => left_behind,
        };
        if left_reads {
            left.advance(c)?;
        } else {
            right.advance(c)?;
        }
        if !settle(&mut left, &mut right) {
            return Ok(false);
        }
    }
}

// Unit tests: level weights and cuts against sort keys, and `lob_equal`.
#[cfg(test)]
mod tests;
