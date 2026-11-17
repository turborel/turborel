//! [ISO/IEC 9075-2:2023 §8.5] `<like predicate>` of a character large
//! object as its match value or its pattern (RULES.md D79): the value read
//! a chunk at a time (`value/like/streamed.rs`), the pattern, which is
//! split into specifiers held in memory, read whole up to a chunk.

use crate::error::{Code, Result, fail};
use crate::value::lob::{CHUNK_OCTETS, Lob, LobKind};
use crate::value::{LikePattern, Value, like as like_strings};

/// \[DESIGN\] The most octets of a character large object a LIKE holds in
/// memory whole: one chunk's. It holds its pattern so, which it splits
/// into specifiers, and its match value under a collation whose
/// characters do not compare one at a time, whose substrings it compares.
const MOST_HELD_OCTETS: usize = CHUNK_OCTETS;

/// The text of `lob`, a character large object, read whole: `what` of a
/// LIKE.
///
/// # Errors
///
/// SQLCODE -900 if it is longer than `MOST_HELD_OCTETS`; as reading a
/// chunk fails.
pub(crate) fn held_text(lob: &Lob, what: &str) -> Result<String> {
    let most = u64::try_from(MOST_HELD_OCTETS).unwrap_or(u64::MAX);
    let Some(bytes) = lob.bounded(most)? else {
        return fail(Code::Unsupported, format!("[DESIGN] {what} is a large object of more than {MOST_HELD_OCTETS} octets"));
    };
    String::from_utf8(bytes).map_err(|_| crate::value::lob::broken("a character large object's octets are not UTF-8"))
}

/// [ISO/IEC 9075-2:2023 §8.5 GR3d] Whether `value` matches `pattern`,
/// `escape` its escape character if one was given, either a character
/// large object, neither null.
///
/// # Errors
///
/// SQLCODE -301 if the pattern has no partitioning by the escape character
/// \[GR3d ii\]; -910 if either is no character string; as `held_text`.
pub fn like(value: &Value, pattern: &Value, escape: Option<char>) -> Result<bool> {
    let text = match character(pattern)? {
        Character::Text(p) => p.to_owned(),
        Character::Large(p) => held_text(p, "a LIKE pattern")?,
    };
    match character(value)? {
        Character::Text(v) => like_strings(v, &text, escape),
        Character::Large(v) => LikePattern::new(&text, escape)?.matches_lob(v),
    }
}

/// A character string's value.
enum Character<'v> {
    /// Of a character string type other than a large object.
    Text(&'v str),
    /// Of a character large object.
    Large(&'v Lob),
}

/// `v`, an operand of LIKE, as a character string.
///
/// # Errors
///
/// SQLCODE -301 if it is no character string, or null, which the caller
/// has judged unknown already.
fn character(v: &Value) -> Result<Character<'_>> {
    let not_character = || fail(Code::Data, "[ISO/IEC 9075-2:2023 §8.5 SR1] LIKE matches character strings");
    match v {
        Value::Char(s) => Ok(Character::Text(s)),
        Value::Lob(l) => {
            if l.kind() == LobKind::Character {
                Ok(Character::Large(l))
            } else {
                not_character()
            }
        }
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Extension(_) => not_character(),
    }
}
