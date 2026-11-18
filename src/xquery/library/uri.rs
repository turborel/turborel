//! The function on URIs [XQueryFO §8]: `fn:resolve-uri`, which resolves a
//! relative URI reference against a base URI by the algorithm of [RFC
//! 3986 §5.2], one [XQueryFO §8.1] names.
//!
//! \[DESIGN\] RULES.md D86: a reference is split into its five components
//! as [RFC 3986 Appendix B] splits a well-formed one, a scheme only if it
//! is one [RFC 3986 §3.1]; resolution is strict, a reference with a scheme
//! returned as it is. Every string is a lexical form of `xs:anyURI` here
//! (`xml/atomic.rs`), so no argument is refused as no URI.

use super::args::{arg, optional_string, single, string, typed};
use super::codes::{FONS0005, FORG0009, fail};
use super::{Builtin, Focus};
use crate::error::Result;
use crate::xml::{AtomicType, Sequence};

/// The functions of [XQueryFO §8].
pub(super) const FUNCTIONS: [Builtin; 1] = [Builtin::function("resolve-uri", 1, 2, resolve_uri)];

/// A URI reference's five components [RFC 3986 §3]: each but the path
/// undefined if its delimiter is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reference<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

/// Whether `text` is a scheme [RFC 3986 §3.1]: `ALPHA *( ALPHA / DIGIT /
/// "+" / "-" / "." )`.
fn is_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `text` split into its components [RFC 3986 Appendix B]:
/// `^(([^:/?#]+):)?(//([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?`.
fn parse(text: &str) -> Reference<'_> {
    let (rest, fragment) = text.split_once('#').map_or((text, None), |(r, f)| (r, Some(f)));
    let (rest, query) = rest.split_once('?').map_or((rest, None), |(r, q)| (r, Some(q)));
    let scheme = rest.split_once(':').map(|(s, _)| s).filter(|s| !s.contains('/') && is_scheme(s));
    let rest = scheme.map_or(rest, |s| rest.get(s.len().saturating_add(1)..).unwrap_or_default());
    let (authority, path) = match rest.strip_prefix("//") {
        Some(after) => {
            let end = after.find('/').unwrap_or(after.len());
            (after.get(..end), after.get(end..).unwrap_or_default())
        }
        None => (None, rest),
    };
    Reference { scheme, authority, path, query, fragment }
}

/// [RFC 3986 §5.2.4] `path` with its "." and ".." complete segments
/// removed, as the algorithm of two buffers removes them.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_owned();
    let mut output = String::with_capacity(path.len());
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../").or_else(|| input.strip_prefix("./")) {
            input = rest.to_owned();
        } else if input.starts_with("/./") || input == "/." {
            input = format!("/{}", input.get(3..).unwrap_or_default());
        } else if input.starts_with("/../") || input == "/.." {
            input = format!("/{}", input.get(4..).unwrap_or_default());
            output.truncate(output.rfind('/').unwrap_or(0));
        } else if input == "." || input == ".." {
            input.clear();
        } else {
            let end = input.get(1..).and_then(|after| after.find('/')).map_or(input.len(), |at| at.saturating_add(1));
            output.push_str(input.get(..end).unwrap_or_default());
            input = input.get(end..).unwrap_or_default().to_owned();
        }
    }
    output
}

/// [RFC 3986 §5.2.3] The path of `reference` merged with `base`'s.
fn merge(base: &Reference<'_>, reference: &str) -> String {
    if base.authority.is_some() && base.path.is_empty() {
        return format!("/{reference}");
    }
    let kept = base.path.rfind('/').map_or("", |at| base.path.get(..=at).unwrap_or_default());
    format!("{kept}{reference}")
}

/// [RFC 3986 §5.3] The URI of the components given.
fn recompose(scheme: Option<&str>, authority: Option<&str>, path: &str, query: Option<&str>, fragment: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(s) = scheme {
        out.push_str(s);
        out.push(':');
    }
    if let Some(a) = authority {
        out.push_str("//");
        out.push_str(a);
    }
    out.push_str(path);
    for (delimiter, part) in [('?', query), ('#', fragment)] {
        if let Some(p) = part {
            out.push(delimiter);
            out.push_str(p);
        }
    }
    out
}

/// [RFC 3986 §5.2.2] The target URI of `relative` against `base`, by the
/// strict algorithm; none if `base` has no scheme, as a base URI must
/// [RFC 3986 §5.2.1].
pub(super) fn resolve(relative: &str, base: &str) -> Option<String> {
    let r = parse(relative);
    if r.scheme.is_some() {
        return Some(recompose(r.scheme, r.authority, &remove_dot_segments(r.path), r.query, r.fragment));
    }
    let b = parse(base);
    let scheme = b.scheme?;
    if r.authority.is_some() {
        return Some(recompose(Some(scheme), r.authority, &remove_dot_segments(r.path), r.query, r.fragment));
    }
    let (path, query) = if r.path.is_empty() {
        (b.path.to_owned(), r.query.or(b.query))
    } else if r.path.starts_with('/') {
        (remove_dot_segments(r.path), r.query)
    } else {
        (remove_dot_segments(&merge(&b, r.path)), r.query)
    };
    Some(recompose(Some(scheme), b.authority, &path, query, r.fragment))
}

/// Whether `text` is an absolute URI, of a scheme [RFC 3986 §4.3].
pub(super) fn is_absolute(text: &str) -> bool {
    parse(text).scheme.is_some()
}

/// [XQueryFO §8.1] `fn:resolve-uri($relative as xs:string?, $base as
/// xs:string) as xs:anyURI?`: `$relative` resolved against `$base`, or
/// against the static context's base URI if `$base` is not given; itself
/// if it is absolute; the empty sequence of the empty sequence.
///
/// # Errors
///
/// err:FONS0005 if `$base` is not given and the static context has no base
/// URI; err:FORG0009 if the base URI is not absolute.
fn resolve_uri(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let Some(relative) = optional_string(arg(args, 0))? else { return Ok(Sequence::empty()) };
    let base = match args.get(1) {
        Some(given) => string(given)?,
        None => focus.base_uri.clone().ok_or_else(|| fail(FONS0005, "the static context has no base URI [XQueryFO §8.1]"))?,
    };
    let target = if is_absolute(&relative) {
        relative
    } else {
        resolve(&relative, &base).ok_or_else(|| fail(FORG0009, &format!("the base URI '{base}' is not absolute [XQueryFO §8.1]")))?
    };
    Ok(single(typed(AtomicType::AnyUri, target)?))
}

#[cfg(test)]
mod tests;
