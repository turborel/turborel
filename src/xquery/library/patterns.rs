//! The functions on strings that use pattern matching [XQueryFO §7.6]:
//! `fn:matches`, `fn:replace` and `fn:tokenize`, by turborel's matcher of
//! XQuery regular expressions (`regex.rs`, RULES.md D61) in the dialect of
//! [XQueryFO §7.6.1] (`Regex::xquery`): a newline is a line feed alone,
//! the flags are `s`, `m`, `i` and `x` [XQueryFO §7.6.1.1].
//!
//! \[DESIGN\] RULES.md D106: the matcher's errors are raised as XQuery's:
//! a pattern that is none err:FORX0002, flags that are none err:FORX0001,
//! a pattern that matches the zero-length string err:FORX0003, a
//! replacement that is none err:FORX0004. A pattern is compiled for each
//! call, as the library keeps nothing between calls.

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, boolean, items, string, string_or_empty, string_value, within_items};
use crate::error::{Result, SqlError, SqlState};
use crate::regex::Regex;
use crate::xml::{Atomic, AtomicType, Item, Sequence};

/// The functions of [XQueryFO §7.6].
pub(super) const FUNCTIONS: [Builtin; 3] = [
    Builtin::function("matches", 2, 3, matches),
    Builtin::function("replace", 3, 4, replace),
    Builtin::function("tokenize", 2, 3, tokenize),
];

/// [XQueryFO §7.6.1.1] The letters of the flags.
const FLAGS: &str = "smix";

/// `error`, of the matcher, as the XQuery error [XQueryFO §7.6] names.
fn as_xquery(error: SqlError) -> SqlError {
    let state = error.sqlstate();
    let code = if state == SqlState::INVALID_XQUERY_REGULAR_EXPRESSION {
        ErrorCode::FORX0002
    } else if state == SqlState::INVALID_XQUERY_OPTION_FLAG {
        ErrorCode::FORX0001
    } else if state == SqlState::ATTEMPT_TO_REPLACE_A_ZERO_LENGTH_STRING {
        ErrorCode::FORX0003
    } else if state == SqlState::INVALID_XQUERY_REPLACEMENT_STRING {
        ErrorCode::FORX0004
    } else {
        return error;
    };
    super::super::error::error(code, &error.described())
}

/// The pattern of `args`'s second argument under the flags of its argument
/// `flags`, compiled.
///
/// # Errors
///
/// err:FORX0001 of a flag none of `smix`; err:FORX0002 of a pattern that
/// is no regular expression [XQueryFO §7.6.1].
fn compiled(args: &[Sequence], flags: usize) -> Result<Regex> {
    let pattern = string(arg(args, 1))?;
    let flags = match args.get(flags) {
        Some(given) => string(given)?,
        None => String::new(),
    };
    if let Some(bad) = flags.chars().find(|c| !FLAGS.contains(*c)) {
        return Err(error(ErrorCode::FORX0001, &format!("'{bad}' is none of the flags s, m, i and x [XQueryFO §7.6.1.1]")));
    }
    Regex::xquery(&pattern, &flags).map_err(as_xquery)
}

/// The error of a pattern that matches the zero-length string [XQueryFO
/// §7.6.3, §7.6.4].
fn matches_nothing() -> SqlError {
    error(ErrorCode::FORX0003, "the pattern matches the zero-length string [XQueryFO §7.6.3]")
}

/// [XQueryFO §7.6.2] `fn:matches($input as xs:string?, $pattern as
/// xs:string, $flags as xs:string) as xs:boolean`: whether some substring
/// of the input matches the pattern; the empty sequence the zero-length
/// string.
fn matches(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let regex = compiled(args, 2)?;
    let input = string_or_empty(arg(args, 0))?;
    Ok(boolean(regex.is_match(&input).map_err(as_xquery)?))
}

/// [XQueryFO §7.6.3] `fn:replace($input as xs:string?, $pattern as
/// xs:string, $replacement as xs:string, $flags as xs:string) as
/// xs:string`: each match, the first of those that overlap, replaced by
/// the replacement, its `$N` what group N captured.
///
/// # Errors
///
/// As `compiled`; err:FORX0003 if the pattern matches the zero-length
/// string; err:FORX0004 of a replacement that is none.
fn replace(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let regex = compiled(args, 3)?;
    let input: Vec<char> = string_or_empty(arg(args, 0))?.chars().collect();
    let replacement = string(arg(args, 2))?;
    string_value(regex.replace(&input, &replacement).map_err(as_xquery)?)
}

/// [XQueryFO §7.6.4] `fn:tokenize($input as xs:string?, $pattern as
/// xs:string, $flags as xs:string) as xs:string*`: the substrings between
/// the matches, a zero-length one before a match at the start, after one
/// at the end and between two adjacent; the empty sequence of the empty
/// sequence or the zero-length string.
///
/// # Errors
///
/// As `compiled`; err:FORX0003 if the pattern matches the zero-length
/// string.
fn tokenize(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let regex = compiled(args, 2)?;
    if regex.is_match("").map_err(as_xquery)? {
        return Err(matches_nothing());
    }
    let input: Vec<char> = string_or_empty(arg(args, 0))?.chars().collect();
    if input.is_empty() {
        return Ok(Sequence::empty());
    }
    let mut tokens = Vec::new();
    let mut after = 0;
    regex
        .each(input.as_slice(), 0, usize::MAX, |found| {
            let (start, end) = found.get(0).unwrap_or((after, after));
            tokens.push(token(&input, after, start));
            after = end;
            within_items(tokens.len())?;
            Ok(true)
        })
        .map_err(as_xquery)?;
    tokens.push(token(&input, after, input.len()));
    items(tokens)
}

/// The string of `input`'s characters from `from` to `to`.
fn token(input: &[char], from: usize, to: usize) -> Item {
    Item::Atomic(Atomic::new(AtomicType::String, input.get(from..to).unwrap_or_default().iter().collect::<String>()))
}

#[cfg(test)]
mod tests;
