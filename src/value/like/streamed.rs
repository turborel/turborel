//! [ISO/IEC 9075-2:2023 §8.5 GR3d] `<like predicate>` of a character large
//! object, its characters read once, in order, never held whole (RULES.md
//! D79). The pattern is split at its arbitrary string specifiers into
//! runs of `_` and characters. Each run is of a fixed number of
//! characters, so the first must match the value's first characters and
//! the last its last ones, and those between match where each first can,
//! in turn after the one before: if they match anywhere in order, they
//! match at those places, as each `%` between takes what is left.

use std::collections::VecDeque;

use super::{LikePattern, Spec};
use crate::error::Result;
use crate::value::lob::{Lob, LobChars, broken};

/// Whether `c` is matched by `spec`, a specifier other than `%`.
fn fits(spec: Spec, c: char) -> bool {
    match spec {
        Spec::AnyCharacter | Spec::AnyString => true,
        Spec::Character(x) => x == c,
    }
}

/// The next character of `chars`, which the value's count says is there.
fn next(chars: &mut LobChars) -> Result<char> {
    chars.next_char()?.ok_or_else(|| broken("a character large object has fewer characters than its count"))
}

/// Whether the next characters of `chars` match `run`, one each.
fn run_matches(run: &[Spec], chars: &mut LobChars) -> Result<bool> {
    for &spec in run {
        let c = next(chars)?;
        if !fits(spec, c) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The count of `run` as a number of characters.
fn count(run: &[Spec]) -> u64 {
    u64::try_from(run.len()).unwrap_or(u64::MAX)
}

/// Reads `chars` from character `at` until the last characters read match
/// `run`, reading none past character `limit`; whether they did. `at` is
/// left after them.
fn found(run: &[Spec], chars: &mut LobChars, at: &mut u64, limit: u64) -> Result<bool> {
    let mut window: VecDeque<char> = VecDeque::with_capacity(run.len());
    while *at < limit {
        window.push_back(next(chars)?);
        *at = at.saturating_add(1);
        if window.len() > run.len() {
            window.pop_front();
        }
        if window.len() == run.len() && window.iter().zip(run).all(|(&c, &spec)| fits(spec, c)) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl LikePattern {
    /// [ISO/IEC 9075-2:2023 §8.5 GR3d] Whether `value`, a character large
    /// object, matches it, read a chunk at a time.
    ///
    /// # Errors
    ///
    /// As reading a chunk fails; SQLCODE -910 if its octets are not UTF-8.
    pub fn matches_lob(&self, value: &Lob) -> Result<bool> {
        let n = value.chars();
        let runs: Vec<&[Spec]> = self.specs.split(|s| *s == Spec::AnyString).collect();
        let mut chars = LobChars::new(value, 0)?;
        let [first, middle @ .., last] = runs.as_slice() else {
            // No `%`: one run, as long as the value.
            let only = runs.first().copied().unwrap_or_default();
            if count(only) != n {
                return Ok(false);
            }
            return run_matches(only, &mut chars);
        };
        let limit = n.checked_sub(count(first).saturating_add(count(last))).map(|between| between.saturating_add(count(first)));
        let Some(limit) = limit else { return Ok(false) };
        if !run_matches(first, &mut chars)? {
            return Ok(false);
        }
        let mut at = count(first);
        for run in middle.iter().filter(|r| !r.is_empty()) {
            if !found(run, &mut chars, &mut at, limit)? {
                return Ok(false);
            }
        }
        run_matches(last, &mut LobChars::new(value, limit)?)
    }
}
