//! The functions that escape strings for URIs [XQueryFO §7.4.10 to
//! §7.4.12]: each character a function does not keep written as the
//! octets of its UTF-8, each `%HH` of upper case hexadecimal digits.

use std::fmt::Write;

use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, string_or_empty, string_value};
use crate::error::Result;
use crate::xml::Sequence;

/// The functions of [XQueryFO §7.4.10 to §7.4.12].
pub(super) const FUNCTIONS: [Builtin; 3] = [
    Builtin::function("encode-for-uri", 1, 1, encode_for_uri),
    Builtin::function("iri-to-uri", 1, 1, iri_to_uri),
    Builtin::function("escape-html-uri", 1, 1, escape_html_uri),
];

/// [XQueryFO §7.4.10] The marks `fn:encode-for-uri` keeps besides letters
/// and digits of ASCII.
const UNRESERVED: &str = "#-_.!~*'()";
/// [XQueryFO §7.4.11] The marks `fn:iri-to-uri` keeps besides those.
const RESERVED: &str = ";/?:@&=+$,[]%";
/// [XQueryFO §7.4.12] The printable characters of US-ASCII, which
/// `fn:escape-html-uri` keeps.
const PRINTABLE: std::ops::RangeInclusive<char> = ' '..='~';

/// `text` with each character `keep` refuses escaped.
fn escaped(text: &str, keep: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut buffer = [0; 4];
    for c in text.chars() {
        if keep(c) {
            out.push(c);
            continue;
        }
        for octet in c.encode_utf8(&mut buffer).bytes() {
            // Writing to a String does not fail.
            let _ = write!(out, "%{octet:02X}");
        }
    }
    out
}

/// Whether `c` is a letter or digit of ASCII, or one of `marks`.
fn kept(c: char, marks: &[&str]) -> bool {
    c.is_ascii_alphanumeric() || marks.iter().any(|m| m.contains(c))
}

/// [XQueryFO §7.4.10] `fn:encode-for-uri($uri-part as xs:string?) as
/// xs:string`: every character escaped but letters, digits and the marks
/// `#-_.!~*'()`.
fn encode_for_uri(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    string_value(escaped(&string_or_empty(arg(args, 0))?, |c| kept(c, &[UNRESERVED])))
}

/// [XQueryFO §7.4.11] `fn:iri-to-uri($uri-part as xs:string?) as
/// xs:string`: every character escaped but letters, digits and the marks
/// `#-_.!~*'()` and `;/?:@&=+$,[]%`.
fn iri_to_uri(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    string_value(escaped(&string_or_empty(arg(args, 0))?, |c| kept(c, &[UNRESERVED, RESERVED])))
}

/// [XQueryFO §7.4.12] `fn:escape-html-uri($uri as xs:string?) as
/// xs:string`: every character escaped but the printable ones of
/// US-ASCII, 32 to 126.
fn escape_html_uri(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    string_value(escaped(&string_or_empty(arg(args, 0))?, |c| PRINTABLE.contains(&c)))
}

#[cfg(test)]
mod tests;
