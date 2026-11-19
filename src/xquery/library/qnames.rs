//! The functions related to QNames [XQueryFO §11]: their constructors,
//! the parts of a QName, and the namespaces in scope of an element, which
//! are `xml`'s and those it and its ancestors declare [XQueryDM §5.7].
//!
//! \[DESIGN\] RULES.md D106: a QName's lexical form is read by XML 1.1's
//! names, as XMLQUERY reads its query [ISO/IEC 9075-14:2006 draft §6.18
//! GR2a].

use super::super::collation::Collator;
use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, items, node, optional, optional_of, optional_string, string, wrong_type};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Kind, Node, Sequence, Version, is_space, qname_parts};

/// The functions of [XQueryFO §11].
pub(super) const FUNCTIONS: [Builtin; 7] = [
    Builtin::function("resolve-QName", 2, 2, resolve_qname),
    Builtin::function("QName", 2, 2, qname),
    Builtin::function("prefix-from-QName", 1, 1, prefix_from_qname),
    Builtin::function("local-name-from-QName", 1, 1, local_name_from_qname),
    Builtin::function("namespace-uri-from-QName", 1, 1, namespace_uri_from_qname),
    Builtin::function("namespace-uri-for-prefix", 2, 2, namespace_uri_for_prefix),
    Builtin::function("in-scope-prefixes", 1, 1, in_scope_prefixes),
];

/// The prefix and local name of the QName `a`'s lexical form.
pub(super) fn parts(a: &Atomic) -> (Option<&str>, &str) {
    a.lexical.split_once(':').map_or((None, &*a.lexical), |(p, l)| (Some(p), l))
}

/// `text`, white space at either end stripped, split as a QName.
///
/// # Errors
///
/// err:FOCA0002 if it is no QName [XQueryFO §11.1].
fn lexical(text: &str) -> Result<(Option<&str>, &str)> {
    let trimmed = text.trim_matches(is_space);
    qname_parts(Version::V11, trimmed).ok_or_else(|| error(ErrorCode::FOCA0002, &format!("'{trimmed}' is no QName [XQueryFO §11.1]")))
}

/// The element `value` is, as a parameter of `element()` takes it.
fn element(value: &Sequence) -> Result<Node> {
    let n = node(value)?;
    if n.kind() == Some(Kind::Element) { Ok(n) } else { Err(wrong_type("an element", "it is another node")) }
}

/// The namespaces in scope of `element` [XQueryDM §5.7]: each prefix,
/// none of the default namespace, and its URI.
fn in_scope(element: &Node) -> Vec<(Option<String>, String)> {
    element.tree().in_scope(element.id()).into_iter().map(|(p, uri)| (p.map(str::to_owned), uri.to_owned())).collect()
}

/// [XQueryFO §11.1.1] `fn:resolve-QName($qname as xs:string?, $element as
/// element()) as xs:QName?`: the QName of the namespace its prefix, or the
/// default namespace if it has none, is bound to in scope of the element.
///
/// # Errors
///
/// err:FOCA0002 of no QName; err:FONS0004 of a prefix not in scope.
fn resolve_qname(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let scope = in_scope(&element(arg(args, 1))?);
    let Some(text) = optional_string(arg(args, 0))? else { return Ok(Sequence::empty()) };
    let (prefix, local) = lexical(&text)?;
    let bound = scope.iter().find(|(p, _)| p.as_deref() == prefix).map(|(_, uri)| uri.as_str());
    if bound.is_none() && prefix.is_some() {
        return Err(error(ErrorCode::FONS0004, &format!("no namespace is bound to '{}' [XQueryFO §11.1.1]", prefix.unwrap_or_default())));
    }
    let written = prefix.map_or_else(|| local.to_owned(), |p| format!("{p}:{local}"));
    Ok(optional(Some(Atomic::qname(bound, &written))))
}

/// [XQueryFO §11.1.2] `fn:QName($paramURI as xs:string?, $paramQName as
/// xs:string) as xs:QName`: the QName of the namespace given, its prefix
/// and local name the second's.
///
/// # Errors
///
/// err:FOCA0002 of no QName, or of a prefix with no namespace.
fn qname(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let uri = optional_string(arg(args, 0))?.filter(|u| !u.is_empty());
    let text = string(arg(args, 1))?;
    let (prefix, local) = lexical(&text)?;
    if uri.is_none() && prefix.is_some() {
        return Err(error(ErrorCode::FOCA0002, &format!("'{text}' has a prefix but no namespace [XQueryFO §11.1.2]")));
    }
    let written = prefix.map_or_else(|| local.to_owned(), |p| format!("{p}:{local}"));
    Ok(optional(Some(Atomic::qname(uri.as_deref(), &written))))
}

/// [XQueryFO §11.2.1] `op:QName-equal`: whether the namespace URIs of `a`
/// and `b` are, by code point, and their local names are by `collation`,
/// the default collation; their prefixes ignored.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no QName.
pub fn qname_equal(a: &Atomic, b: &Atomic, collation: &Collator) -> Result<bool> {
    let (x, y) = (as_qname(a)?, as_qname(b)?);
    Ok(x.namespace == y.namespace && collation.equal(parts(&x).1, parts(&y).1))
}

/// `a`, which must be an `xs:QName`.
fn as_qname(a: &Atomic) -> Result<Atomic> {
    if a.kind == AtomicType::QName { Ok(a.clone()) } else { Err(wrong_type("an xs:QName", &format!("it is an {}", a.kind.name()))) }
}

/// The QName of `args`'s first; none of the empty sequence.
fn qname_arg(args: &[Sequence]) -> Result<Option<Atomic>> {
    optional_of(arg(args, 0), AtomicType::QName)
}

/// [XQueryFO §11.2.2] `fn:prefix-from-QName($arg as xs:QName?) as
/// xs:NCName?`: its prefix; the empty sequence of none.
fn prefix_from_qname(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let prefix = qname_arg(args)?.and_then(|q| parts(&q).0.map(|p| Atomic::new(AtomicType::NcName, p)));
    Ok(optional(prefix))
}

/// [XQueryFO §11.2.3] `fn:local-name-from-QName($arg as xs:QName?) as
/// xs:NCName?`: its local name.
fn local_name_from_qname(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    Ok(optional(qname_arg(args)?.map(|q| Atomic::new(AtomicType::NcName, parts(&q).1))))
}

/// [XQueryFO §11.2.4] `fn:namespace-uri-from-QName($arg as xs:QName?) as
/// xs:anyURI?`: its namespace URI, the zero-length one of none.
fn namespace_uri_from_qname(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    Ok(optional(qname_arg(args)?.map(|q| Atomic::new(AtomicType::AnyUri, q.namespace.as_deref().unwrap_or_default()))))
}

/// [XQueryFO §11.2.5] `fn:namespace-uri-for-prefix($prefix as
/// xs:string?, $element as element()) as xs:anyURI?`: the URI the prefix,
/// or the default namespace of the zero-length string or the empty
/// sequence, is bound to in scope of the element; the empty sequence if
/// none is.
fn namespace_uri_for_prefix(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let scope = in_scope(&element(arg(args, 1))?);
    let prefix = optional_string(arg(args, 0))?.filter(|p| !p.is_empty());
    let uri = scope.into_iter().find(|(p, _)| *p == prefix).map(|(_, uri)| Atomic::new(AtomicType::AnyUri, uri));
    Ok(optional(uri))
}

/// [XQueryFO §11.2.6] `fn:in-scope-prefixes($element as element()) as
/// xs:string*`: the prefixes in scope of the element, `xml` among them,
/// the zero-length string of a default namespace.
fn in_scope_prefixes(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let scope = in_scope(&element(arg(args, 0))?);
    items(scope.into_iter().map(|(p, _)| Item::Atomic(Atomic::new(AtomicType::String, p.unwrap_or_default()))).collect())
}

#[cfg(test)]
mod tests;
