//! The names of constructed nodes [XQuery §3.7.3.1, §3.7.3.2] and the
//! namespace bindings they need [XQuery §3.7.4]: a name expression's value
//! resolved to an expanded QName, and each name of a constructed element
//! and its attributes given a binding in its scope, a prefix bound to
//! another URI on the element made anew.

use super::super::super::error::{ErrorCode, error};
use super::super::super::plan::StaticNames;
use super::super::Seq;
use super::super::sequence::atomize;
use crate::error::Result;
use crate::xml::{AtomicType, Binding, QName, Version, XML_PREFIX, is_space, qname_parts};

/// \[DESIGN\] Implementation-dependent [XQuery §3.7.4]: what a prefix made
/// for a name is, this then a number.
const MADE_PREFIX: &str = "ns";

/// The URI `prefix`, none for the default namespace, is bound to by
/// `declared`, the latest binding, else by `scope`; the empty string if
/// it is undeclared, none if neither binds it.
fn bound<'a>(scope: &'a [Binding], declared: &'a [Binding], prefix: Option<&str>) -> Option<&'a str> {
    let find = |list: &'a [Binding]| list.iter().rev().find(|b| b.prefix.as_deref() == prefix).map(|b| &*b.uri);
    find(declared).or_else(|| find(scope))
}

/// A prefix bound to nothing in `scope` nor `declared`, made from
/// `like`, else `MADE_PREFIX`, and a number [XQuery §3.7.4].
fn made_prefix(scope: &[Binding], declared: &[Binding], like: Option<&str>) -> Box<str> {
    let stem = like.unwrap_or(MADE_PREFIX);
    let mut n = 1_usize;
    loop {
        let candidate = format!("{stem}{n}");
        if candidate != XML_PREFIX && bound(scope, declared, Some(&candidate)).is_none() {
            return candidate.into();
        }
        n = n.saturating_add(1);
    }
}

/// `name`, of an element if not `attribute`, given a binding in the scope
/// of the element made [XQuery §3.7.4]: of the bindings `scope` it is in,
/// and those it declares, `declared`, to which a binding it needs is
/// added: its prefix's, or the default namespace's of an element; an
/// element in no namespace undeclaring a default namespace in scope; its
/// prefix made anew if the element binds it to another URI, or if it is an
/// attribute's in a namespace with none.
pub(super) fn bind(scope: &[Binding], declared: &mut Vec<Binding>, name: &mut QName, attribute: bool) {
    let Some(uri) = name.uri.clone().filter(|u| !u.is_empty()) else {
        name.prefix = None;
        let default = bound(scope, declared, None).filter(|u| !u.is_empty());
        if !attribute && default.is_some() {
            declared.push(Binding { prefix: None, uri: "".into() });
        }
        return;
    };
    if name.prefix.as_deref() == Some(XML_PREFIX) {
        return;
    }
    if attribute && name.prefix.is_none() {
        name.prefix = Some(made_prefix(scope, declared, None));
    }
    if bound(scope, declared, name.prefix.as_deref()) == Some(&*uri) {
        return;
    }
    if declared.iter().any(|b| b.prefix == name.prefix) {
        name.prefix = Some(made_prefix(scope, declared, name.prefix.as_deref()));
    }
    declared.push(Binding { prefix: name.prefix.clone(), uri });
}

/// The expanded QName a name expression's value gives [XQuery §3.7.3.1,
/// §3.7.3.2]: one atomic value \[err:XPTY0004\], a string or untyped value
/// that is a lexical QName \[err:XQDY0074\], its prefix bound by `names`,
/// unprefixed in the default element/type namespace of an element,
/// none of an attribute.
pub(super) fn computed(value: &Seq, names: &StaticNames) -> Result<QName> {
    let atomized = atomize(value)?;
    let [name] = atomized.as_slice() else {
        return Err(error(ErrorCode::XPTY0004, "a name expression is not one atomic value [XQuery §3.7.3.1]"));
    };
    if !name.kind.derives_from(AtomicType::String) && name.kind != AtomicType::UntypedAtomic {
        let why = format!("a name expression is a string or untyped, not {} [XQuery §3.7.3.1]", name.kind.name());
        return Err(error(ErrorCode::XPTY0004, &why));
    }
    let invalid =
        || error(ErrorCode::XQDY0074, &format!("'{}' is no QName of the statically known namespaces [XQuery §3.7.3.1]", name.lexical));
    let (prefix, local) = qname_parts(Version::V11, name.lexical.trim_matches(is_space)).ok_or_else(invalid)?;
    let uri = match prefix {
        Some(p) => Some(names.prefixes.iter().rev().find(|(q, _)| &**q == p).map(|(_, u)| u.clone()).ok_or_else(invalid)?),
        None => names.default_element.clone(),
    };
    Ok(QName { prefix: prefix.map(Into::into), uri, local: local.into() })
}
