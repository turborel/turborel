//! \[DESIGN\] The share of rows a LIKE keeps (RULES.md D381). A constant
//! pattern that begins with characters (`value::constant_prefix`) keeps
//! only strings that begin with them, which lie in the range from the
//! prefix up to the prefix with its last character the next: that
//! range's share, by the column's common values and histogram, times
//! `LIKE_REST` for what the rest of the pattern asks, unless the rest is
//! `%` alone, which asks nothing. Any other pattern no synopsis answers;
//! the sample does if it can (`estimate/sampled.rs`), else the default.

use std::cmp::Ordering;

use super::{Known, compare};
use crate::logical::{Cond, Expr};
use crate::value::{CompOp, Value};
use crate::value::{constant_prefix, constant_rest_is_any};

/// \[DESIGN\] The share of the strings with a pattern's prefix that the
/// rest of the pattern keeps, when it asks more than `%`: a quarter, a
/// guess, as no statistic tells how often characters follow others.
pub(super) const LIKE_REST: f64 = 0.25;

/// The share of rows the LIKE `c` keeps, if it is of a column and a
/// constant pattern with a prefix and its table's statistics are known;
/// none if not.
pub(super) fn selectivity(c: &Cond, known: Known<'_>) -> Option<f64> {
    known.stats?;
    let Prefixed { column, pattern, escape, prefix } = prefixed(c)?;
    let from = compare((column, CompOp::Ge, &Value::Char(prefix.clone())), known);
    let past = next_prefix(&prefix).map_or(0.0_f64, |next| compare((column, CompOp::Ge, &Value::Char(next)), known));
    let rest = if constant_rest_is_any(pattern, escape) { 1.0_f64 } else { LIKE_REST };
    Some(((from - past).max(0.0_f64) * rest).clamp(0.0, 1.0))
}

/// A LIKE of a column and a constant pattern that begins with
/// characters: the column, the pattern, its escape and its prefix.
pub(super) struct Prefixed<'c> {
    column: usize,
    pattern: &'c Value,
    escape: Option<&'c Value>,
    prefix: String,
}

/// `c` as a LIKE of a column and a constant pattern with a prefix, if
/// it is one.
pub(super) fn prefixed(c: &Cond) -> Option<Prefixed<'_>> {
    let Cond::Like(Expr::Column(column), Expr::Const(pattern), escape) = c else { return None };
    let escape = match escape {
        None => None,
        Some(Expr::Const(e)) => Some(e),
        Some(_) => return None,
    };
    let prefix = constant_prefix(pattern, escape).filter(|p| !p.is_empty())?;
    Some(Prefixed { column: *column, pattern, escape, prefix })
}

/// The least string past every string beginning with `prefix`: `prefix`
/// with its last character that has a next replaced by the next, those
/// after it dropped; none if no character has a next.
pub(super) fn next_prefix(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        if let Some(after) = next_char(last) {
            chars.push(after);
            let past: String = chars.into_iter().collect();
            // A string compares past every string with the prefix only if
            // the order of characters is their code points'.
            return (past.as_str().cmp(prefix) == Ordering::Greater).then_some(past);
        }
    }
    None
}

/// The character after `c` by code point, past the surrogates.
fn next_char(c: char) -> Option<char> {
    let mut code = u32::from(c);
    while code < u32::from(char::MAX) {
        code = code.saturating_add(1);
        if let Some(next) = char::from_u32(code) {
            return Some(next);
        }
    }
    None
}

// Unit tests: the next prefix (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::next_prefix;

    /// Verifies: [DESIGN] RULES.md D381, LIKE: the prefix past every string
    /// beginning with one is it with its last character the next; a last
    /// character with no next is dropped for the one before; none past a
    /// prefix of only such characters.
    #[test]
    fn the_next_prefix_is_past_every_string_beginning_with_one() {
        assert_eq!(next_prefix("ab").as_deref(), Some("ac"));
        assert_eq!(next_prefix("a\u{10FFFF}").as_deref(), Some("b"));
        assert_eq!(next_prefix("\u{D7FF}").as_deref(), Some("\u{E000}"));
        assert_eq!(next_prefix("\u{10FFFF}"), None);
    }
}
