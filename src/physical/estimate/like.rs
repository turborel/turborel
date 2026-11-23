//! \[DESIGN\] The share of rows a LIKE keeps (RULES.md D381). A constant
//! pattern that begins with characters (`value::constant_prefix`) keeps
//! only strings that begin with them, which lie in the range from the
//! prefix up to the prefix with its last character the next: that
//! range's share, by the column's common values and histogram, times
//! what the rest of the pattern asks, unless the rest is `%` alone, which
//! asks nothing: the share of the sampled strings with the prefix that
//! the whole pattern keeps, if the sample holds enough of them, else
//! `LIKE_REST`. A range narrower than a histogram's
//! bucket keeps at least an equality's share. Of a pattern whose rest
//! asks more, or with no prefix, the sample answers if it can
//! (`estimate/sampled.rs`), else this estimate or the default.

use std::cmp::Ordering;

use super::fingerprint::key_of;
use super::sampled::{SAMPLE_HITS, as_given, remembered_as};
use super::{Known, compare};
use crate::exec::{NoSubqueries, eval_cond};
use crate::logical::Truth;
use crate::logical::{Cond, Expr};
use crate::storage::{Row, cost_of};
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
    let from = compare((column, CompOp::Ge, &Value::Char(prefix.clone().into())), known);
    let past = next_prefix(&prefix).map_or(0.0_f64, |next| compare((column, CompOp::Ge, &Value::Char(next.into())), known));
    let rest = if constant_rest_is_any(pattern, escape) { 1.0_f64 } else { sampled_rest(c, column, &prefix, known).unwrap_or(LIKE_REST) };
    // A range narrower than a histogram's bucket shows no share of it; it
    // keeps at least what an equality with the prefix would.
    let equal = compare((column, CompOp::Eq, &Value::Char(prefix.into())), known);
    Some(((from - past).max(equal).max(0.0_f64) * rest).clamp(0.0, 1.0))
}

/// Of the sampled strings of `column` beginning with `prefix`, the share
/// the LIKE `c` keeps, if the table's sample holds at least
/// `SAMPLE_HITS` of them: what the rest of the pattern asks, `_` and
/// characters after a `%` among it, which no synopsis tells.
fn sampled_rest(c: &Cond, column: usize, prefix: &str, known: Known<'_>) -> Option<f64> {
    let sample = known.stats?.sample()?;
    as_given(sample.remembered(key_of("like", &[c], &|_| {}), || remembered_as(rest_of(c, column, prefix, sample.rows()))))
}

/// `sampled_rest`, found from the sampled rows `rows`.
fn rest_of(c: &Cond, column: usize, prefix: &str, rows: &[Row]) -> Option<f64> {
    let begun: Vec<&Row> = rows.iter().filter(|row| row.get(column).and_then(text).is_some_and(|s| s.starts_with(prefix))).collect();
    if begun.len() < SAMPLE_HITS {
        return None;
    }
    let kept = begun.iter().filter(|row| matches!(eval_cond(c, row, &NoSubqueries), Ok(Truth::True))).count();
    Some(cost_of(kept) / cost_of(begun.len()))
}

/// The string `v` holds, if it is one.
fn text(v: &Value) -> Option<&str> {
    if let Value::Char(s) = v { Some(s) } else { None }
}

/// Whether `c` is a LIKE whose prefix answers it whole, the rest of its
/// pattern `%` alone.
pub(super) fn whole(c: &Cond) -> bool {
    prefixed(c).is_some_and(|p| constant_rest_is_any(p.pattern, p.escape))
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
