//! The functions on strings [XQueryFO §7.2 to §7.4]: code points, the
//! comparison of strings by a collation, and `fn:concat`,
//! `fn:string-join`, `fn:substring`, `fn:string-length`,
//! `fn:normalize-space` and `fn:translate`. A string's length and the
//! positions in it are counted in characters, code points [XQueryFO §7.1].
//!
//! \[DESIGN\] RULES.md D86: a code point is a character of XML 1.1
//! [XML 1.1 §2.2], by whose lexical rules XMLQUERY reads its query
//! [ISO/IEC 9075-14:2006 draft §6.18 GR2a].

use std::cmp::Ordering;

use super::accessors::string_of;
use super::args::{
    arg, atomized, double, integer_value, items, optional, optional_atomic, optional_string, or_context, string, string_or_empty,
    string_value, strings, within_items, within_string,
};
use super::codes::{FOCH0001, fail};
use super::collation::Collator;
use super::decimal::Decimal;
use super::rounding::round_real;
use super::{Builtin, Focus};
use crate::error::Result;
use crate::xml::{AtomicType, Item, Sequence, Version, cast, is_char, is_space};

/// The functions of [XQueryFO §7.2 to §7.4], but those of Unicode's
/// mappings and of URIs.
pub(super) const FUNCTIONS: [Builtin; 10] = [
    Builtin::function("codepoints-to-string", 1, 1, codepoints_to_string),
    Builtin::function("string-to-codepoints", 1, 1, string_to_codepoints),
    Builtin::function("compare", 2, 3, compare),
    Builtin::function("codepoint-equal", 2, 2, codepoint_equal),
    Builtin::function("concat", 2, usize::MAX, concat),
    Builtin::function("string-join", 2, 2, string_join),
    Builtin::function("substring", 2, 3, substring),
    Builtin::function("string-length", 0, 1, string_length),
    Builtin::function("normalize-space", 0, 1, normalize_space),
    Builtin::function("translate", 3, 3, translate),
];

/// [XQueryFO §7.2.1] `fn:codepoints-to-string($arg as xs:integer*) as
/// xs:string`: the string of the code points.
///
/// # Errors
///
/// err:FOCH0001 of a code point that is no character of XML.
fn codepoints_to_string(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let mut out = String::new();
    for a in atomized(arg(args, 0)) {
        let n = super::args::integer_of(&a)?;
        let c = n.to_i128().and_then(|n| u32::try_from(n).ok()).and_then(char::from_u32).filter(|&c| is_char(Version::V11, c));
        let c = c.ok_or_else(|| fail(FOCH0001, &format!("{} is no character of XML [XQueryFO §7.2.1]", n.text())))?;
        within_string(out.len())?;
        out.push(c);
    }
    string_value(out)
}

/// [XQueryFO §7.2.2] `fn:string-to-codepoints($arg as xs:string?) as
/// xs:integer*`: the code points of the string; the empty sequence of the
/// empty sequence or the zero-length string.
fn string_to_codepoints(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_empty(arg(args, 0))?;
    within_items(text.chars().count())?;
    items(text.chars().map(|c| Item::Atomic(integer_value(&Decimal::integer(i128::from(u32::from(c)))))).collect())
}

/// The collation of `focus` and the optional argument `n` of `args`.
fn collator(focus: &Focus, args: &[Sequence], n: usize) -> Result<Collator> {
    let given = args.get(n).map(string).transpose()?;
    Collator::of(focus, given.as_deref())
}

/// [XQueryFO §7.3.2] `fn:compare($comparand1 as xs:string?, $comparand2
/// as xs:string?, $collation as xs:string) as xs:integer?`: -1, 0 or 1 as
/// the first is less than, equal to or greater than the second by the
/// collation; the empty sequence if either is.
///
/// # Errors
///
/// err:FOCH0002 of a collation not supported.
fn compare(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let collation = collator(focus, args, 2)?;
    let (Some(a), Some(b)) = (optional_string(arg(args, 0))?, optional_string(arg(args, 1))?) else {
        return Ok(Sequence::empty());
    };
    let order = match collation.compare(&a, &b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(optional(Some(integer_value(&Decimal::integer(order)))))
}

/// [XQueryFO §7.3.3] `fn:codepoint-equal($comparand1 as xs:string?,
/// $comparand2 as xs:string?) as xs:boolean?`: whether they are equal by
/// the code point collation; the empty sequence if either is.
fn codepoint_equal(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let (Some(a), Some(b)) = (optional_string(arg(args, 0))?, optional_string(arg(args, 1))?) else {
        return Ok(Sequence::empty());
    };
    Ok(super::args::boolean(a == b))
}

/// [XQueryFO §7.4.1] `fn:concat($arg1 as xdt:anyAtomicType?, $arg2 as
/// xdt:anyAtomicType?, ...) as xs:string`: each argument cast to a string,
/// joined; the empty sequence the zero-length string.
fn concat(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let mut out = String::new();
    for value in args {
        if let Some(a) = optional_atomic(value)? {
            out.push_str(&cast(&a, AtomicType::String)?.lexical);
        }
        within_string(out.len())?;
    }
    string_value(out)
}

/// [XQueryFO §7.4.2] `fn:string-join($arg1 as xs:string*, $arg2 as
/// xs:string) as xs:string`: the strings joined by the separator.
fn string_join(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let separator = string(arg(args, 1))?;
    let parts = strings(arg(args, 0))?;
    let length = parts.iter().map(String::len).fold(separator.len().saturating_mul(parts.len()), usize::saturating_add);
    within_string(length)?;
    string_value(parts.join(&separator))
}

/// [XQueryFO §7.4.3] `fn:substring($sourceString as xs:string?,
/// $startingLoc as xs:double, $length as xs:double) as xs:string`: the
/// characters at positions `p`, from 1, such that `round(start) <= p <
/// round(start) + round(length)`, by the comparisons of doubles, so NaN
/// selects none; the length infinite if not given.
fn substring(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_empty(arg(args, 0))?;
    let start = round_real(double(arg(args, 1))?);
    let end = match args.get(2) {
        Some(length) => start + round_real(double(length)?),
        None => f64::INFINITY,
    };
    let mut position = 0.0_f64;
    let kept: String = text
        .chars()
        .filter(|_| {
            position += 1.0_f64;
            position >= start && position < end
        })
        .collect();
    string_value(kept)
}

/// The string of the argument `n` of `args`, or of the context item if it
/// is not given, as `fn:string` makes it.
fn string_or_context(focus: &Focus, args: &[Sequence], n: usize) -> Result<String> {
    if args.get(n).is_some() {
        return string_or_empty(arg(args, n));
    }
    let value = or_context(args, n, focus)?;
    value.items().first().map_or_else(|| Ok(String::new()), string_of)
}

/// [XQueryFO §7.4.4] `fn:string-length($arg as xs:string?) as
/// xs:integer`: its length in characters, 0 of the empty sequence; of the
/// context item's string if it is not given.
///
/// # Errors
///
/// err:FONC0001 if it is not given and there is no context item.
fn string_length(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_context(focus, args, 0)?;
    let count = i128::try_from(text.chars().count()).unwrap_or(i128::MAX);
    Ok(optional(Some(integer_value(&Decimal::integer(count)))))
}

/// [XQueryFO §7.4.5] `fn:normalize-space($arg as xs:string?) as
/// xs:string`: its white space, XML's `S`, stripped at either end and
/// each run of it one space; of the context item's string if it is not
/// given.
///
/// # Errors
///
/// err:FONC0001 if it is not given and there is no context item.
fn normalize_space(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_context(focus, args, 0)?;
    string_value(text.split(is_space).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" "))
}

/// [XQueryFO §7.4.9] `fn:translate($arg as xs:string?, $mapString as
/// xs:string, $transString as xs:string) as xs:string`: each character of
/// `$mapString` replaced by the one at its first place in `$transString`,
/// or left out if that is shorter.
fn translate(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_empty(arg(args, 0))?;
    let from: Vec<char> = string(arg(args, 1))?.chars().collect();
    let to: Vec<char> = string(arg(args, 2))?.chars().collect();
    let out: String = text
        .chars()
        .filter_map(|c| match from.iter().position(|&m| m == c) {
            Some(at) => to.get(at).copied(),
            None => Some(c),
        })
        .collect();
    string_value(out)
}

#[cfg(test)]
mod tests;
