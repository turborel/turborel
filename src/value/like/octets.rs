//! [ISO/IEC 9075-2:2023 §8.5 GR4] `<octet like predicate>`: whether a
//! binary string matches a pattern of octets. `_` and `%` are the octets
//! of those characters in SQL_TEXT [GR4b, GR4c], and an escape octet
//! makes either, or itself, stand for itself [GR4d]. The pattern is split
//! at its arbitrary string specifiers into runs of a fixed number of
//! octets. The first must match the value's first octets and the last its
//! last ones, and each between matches where it first can after the one
//! before, as each `%` between takes what is left. A binary large object
//! is read so, a chunk at a time, never held whole (RULES.md D79).

use std::collections::VecDeque;

use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::lob::{Lob, LobReader};

/// The octet of `_` in SQL_TEXT [GR4c].
const UNDERSCORE: u8 = b'_';
/// The octet of `%` in SQL_TEXT [GR4b].
const PERCENT: u8 = b'%';

/// A substring specifier of an octet pattern, one octet long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spec {
    /// `_`: any single octet.
    AnyOctet,
    /// `%`: any sequence of zero or more octets.
    AnyString,
    /// An octet that represents itself.
    Octet(u8),
}

/// [§8.5 GR4] An `<octet pattern>` split into its specifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OctetPattern {
    specs: Vec<Spec>,
}

/// What `matched` reads a binary string by: its octets in memory, or a
/// binary large object's reader.
trait Octets {
    /// Its length in octets.
    fn length(&self) -> u64;
    /// `count` of its octets from octet `from`.
    fn take(&mut self, from: u64, count: u64) -> Result<Vec<u8>>;
    /// Each piece of `count` octets from octet `from`, in order, until `f`
    /// gives false; whether it never did.
    fn pieces(&mut self, from: u64, count: u64, f: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<bool>;
}

/// A binary string's octets in memory.
struct Held<'a>(&'a [u8]);

impl Octets for Held<'_> {
    fn length(&self) -> u64 {
        u64::try_from(self.0.len()).unwrap_or(u64::MAX)
    }
    fn take(&mut self, from: u64, count: u64) -> Result<Vec<u8>> {
        let at = |n: u64| usize::try_from(n).unwrap_or(usize::MAX).min(self.0.len());
        Ok(self.0.get(at(from)..at(from.saturating_add(count))).unwrap_or_default().to_vec())
    }
    fn pieces(&mut self, from: u64, count: u64, f: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<bool> {
        let piece = self.take(from, count)?;
        f(&piece)
    }
}

/// A binary large object, read through its reader.
struct Large<'a> {
    octets: u64,
    reader: LobReader<'a>,
}

impl Octets for Large<'_> {
    fn length(&self) -> u64 {
        self.octets
    }
    fn take(&mut self, from: u64, count: u64) -> Result<Vec<u8>> {
        self.reader.read(from, count)
    }
    fn pieces(&mut self, from: u64, count: u64, f: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<bool> {
        self.reader.each(from, count, f)
    }
}

impl OctetPattern {
    /// `pattern`, `escape` its escape octet if one was given.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200D if the escape octet is not one octet \[GR4d i 1\];
    /// 22025 if the pattern has no partitioning by it \[GR4d i 2\].
    pub fn new(pattern: &[u8], escape: Option<&[u8]>) -> Result<OctetPattern> {
        let escape = match escape {
            None => None,
            Some(&[z]) => Some(z),
            Some(_) => {
                let msg = "[ISO/IEC 9075-2:2023 §8.5 GR4d i 1] an escape octet is one octet";
                return fail_with(Code::Data, SqlState::INVALID_ESCAPE_OCTET, msg);
            }
        };
        let mut specs = Vec::with_capacity(pattern.len());
        let mut octets = pattern.iter().copied();
        while let Some(b) = octets.next() {
            specs.push(match (Some(b) == escape, b) {
                (true, _) => match octets.next() {
                    Some(next) if Some(next) == escape || next == UNDERSCORE || next == PERCENT => Spec::Octet(next),
                    Some(_) | None => {
                        let msg = "[ISO/IEC 9075-2:2023 §8.5 GR4d i 2] the escape octet is followed by none but itself, '_' or '%'";
                        return fail_with(Code::Data, SqlState::INVALID_ESCAPE_SEQUENCE, msg);
                    }
                },
                (false, UNDERSCORE) => Spec::AnyOctet,
                (false, PERCENT) => Spec::AnyString,
                (false, other) => Spec::Octet(other),
            });
        }
        Ok(OctetPattern { specs })
    }

    /// Whether `value`, a binary string, matches it \[GR4f\].
    #[must_use]
    pub fn matches(&self, value: &[u8]) -> bool {
        matched(&self.specs, &mut Held(value)).unwrap_or(false)
    }

    /// Whether `value`, a binary large object, matches it, read a chunk at
    /// a time \[GR4f\].
    ///
    /// # Errors
    ///
    /// As reading a chunk fails.
    pub fn matches_lob(&self, value: &Lob) -> Result<bool> {
        matched(&self.specs, &mut Large { octets: value.octets(), reader: value.reader() })
    }
}

/// Whether `b` is matched by `spec`, a specifier other than `%`.
fn fits(spec: Spec, b: u8) -> bool {
    match spec {
        Spec::AnyOctet | Spec::AnyString => true,
        Spec::Octet(x) => x == b,
    }
}

/// Whether `octets` are as many as `run` and each fits its specifier.
fn run_fits(run: &[Spec], octets: &[u8]) -> bool {
    run.len() == octets.len() && run.iter().zip(octets).all(|(&s, &b)| fits(s, b))
}

/// The length of `run` in octets.
fn length(run: &[Spec]) -> u64 {
    u64::try_from(run.len()).unwrap_or(u64::MAX)
}

/// Whether `value` matches `specs` [GR4f]: the first run at its start, the
/// last at its end, those between in order where each first can.
fn matched(specs: &[Spec], value: &mut dyn Octets) -> Result<bool> {
    let n = value.length();
    let runs: Vec<&[Spec]> = specs.split(|s| *s == Spec::AnyString).collect();
    let [first, middle @ .., last] = runs.as_slice() else {
        // No `%`: one run, as long as the value.
        let only = runs.first().copied().unwrap_or_default();
        return Ok(length(only) == n && run_fits(only, &value.take(0, n)?));
    };
    let Some(limit) = n.checked_sub(length(last)) else { return Ok(false) };
    if length(first) > limit || !run_fits(first, &value.take(0, length(first))?) || !run_fits(last, &value.take(limit, length(last))?) {
        return Ok(false);
    }
    let mut at = length(first);
    for run in middle.iter().filter(|r| !r.is_empty()) {
        let Some(end) = found(run, value, at, limit)? else { return Ok(false) };
        at = end;
    }
    Ok(true)
}

/// Where `run` first ends in `value` after octet `at`, ending at octet
/// `limit` at most; none if nowhere.
fn found(run: &[Spec], value: &mut dyn Octets, at: u64, limit: u64) -> Result<Option<u64>> {
    let mut window: VecDeque<u8> = VecDeque::with_capacity(run.len());
    let (mut position, mut end) = (at, None);
    value.pieces(at, limit.saturating_sub(at), &mut |piece| {
        for &b in piece {
            window.push_back(b);
            position = position.saturating_add(1);
            if window.len() > run.len() {
                window.pop_front();
            }
            if window.len() == run.len() && window.iter().zip(run).all(|(&x, &s)| fits(s, x)) {
                end = Some(position);
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::lob::fixtures::octets_by;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR4] octets matched by
    /// `_`, `%` and themselves, the empty pattern matching the empty string
    /// alone, an escape octet making `_`, `%` and itself literal; the same of a
    /// binary large object in chunks of every size, runs crossing them.
    #[test]
    fn octet_patterns_match_strings_and_large_objects() {
        let value = [0_u8, b'%', 7, 7, b'_', 9, 0];
        for (pattern, escape, expected) in [
            (&b"%"[..], None, true),
            (&[0, b'_', 7, b'%', 0], None, true),
            (&[0, b'%', 9, 0], None, true),
            (&[b'%', 7, 7, b'%'], None, true),
            (&[b'%', 7, 8, b'%'], None, false),
            (&[0, 1, b'!', b'%', b'%'], Some(&b"!"[..]), false),
            (&[0, b'!', b'%', b'%', b'!', b'_', b'%'], Some(&b"!"[..]), true),
            (&[b'%', 9, 0, 0], None, false),
            (&[], None, false),
        ] {
            let p = OctetPattern::new(pattern, escape).unwrap();
            assert_eq!(p.matches(&value), expected, "{pattern:?}");
            for size in 1..=value.len() {
                assert_eq!(p.matches_lob(&octets_by(&value, size)).unwrap(), expected, "{pattern:?} in chunks of {size}");
            }
        }
        assert!(OctetPattern::new(&[], None).unwrap().matches(&[]));
        let state = |e: crate::error::SqlError| e.state;
        assert_eq!(OctetPattern::new(b"%", Some(b"ab")).map_err(state).err(), Some(Some(SqlState::INVALID_ESCAPE_OCTET)));
        assert_eq!(OctetPattern::new(b"a!", Some(b"!")).map_err(state).err(), Some(Some(SqlState::INVALID_ESCAPE_SEQUENCE)));
        assert_eq!(OctetPattern::new(b"!a", Some(b"!")).map_err(state).err(), Some(Some(SqlState::INVALID_ESCAPE_SEQUENCE)));
    }
}
