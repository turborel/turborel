//! [ISO/IEC 9075-14:2006 draft §11.3, §10.12] The `<XML namespace
//! declaration>`s in scope as an expression is bound, and the namespace
//! URI of an XML QName written in it.
//!
//! An XMLELEMENT's or XMLFOREST's declaration is in scope within it
//! [§6.14 SR1, §6.15 SR1], so within a subquery of it too; those of the
//! `<XML lexically scoped options>` of a query, a definition, a
//! statement or a compound statement are in scope within it [§7.2 SR2,
//! §12.1 SR1, §12.2 SR1, §12.4 SR1, §14.3 to §14.7 SR1, §15.1 SR1]. They
//! are searched the innermost first (`binder/xml/scope.rs`).

use std::cell::RefCell;

use super::super::{Binder, rule};
use crate::ast::{LexicallyScopedOptions, NamespaceItem};
use crate::error::Result;
use crate::xml::{
    SQLXML_NAMESPACE, SQLXML_PREFIX, Version, XML_NAMESPACE, XML_PREFIX, XMLNS_NAMESPACE, XS_NAMESPACE, XS_PREFIX, XSI_NAMESPACE,
    XSI_PREFIX, is_ncname,
};

/// The prefix `xmlns`, which no declaration binds [§11.3 SR5].
const XMLNS_PREFIX: &str = "xmlns";

/// [§11.3 SR2 to SR7] The error unless `items` declare at most one
/// default namespace, each prefix an XML 1.1 NCName, none twice, none
/// `xml` or `xmlns`, no URI the namespace of `xml` or `xmlns`, and each
/// prefix's URI not empty.
///
/// # Errors
///
/// SQLCODE -103 if they break one of those rules.
pub(crate) fn checked_namespaces(items: &[NamespaceItem]) -> Result<()> {
    let cite = |sr: &str, what: String| rule(format!("[ISO/IEC 9075-14:2006 draft §11.3 {sr}] {what}"));
    let defaults = items.iter().filter(|i| matches!(i, NamespaceItem::Default(_) | NamespaceItem::NoDefault)).count();
    if defaults > 1 {
        return Err(cite("SR2", "XMLNAMESPACES declares the default namespace once at most".into()));
    }
    for (n, item) in items.iter().enumerate() {
        let (uri, prefix) = match item {
            NamespaceItem::Regular { uri, prefix } => (uri.as_str(), Some(prefix.as_str())),
            NamespaceItem::Default(uri) => (uri.as_str(), None),
            NamespaceItem::NoDefault => continue,
        };
        if uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE {
            return Err(cite("SR6", format!("XMLNAMESPACES binds nothing to {uri}")));
        }
        let Some(prefix) = prefix else { continue };
        if !is_ncname(Version::V11, prefix) {
            return Err(cite("SR3", format!("the prefix {prefix} is not an XML 1.1 NCName")));
        }
        if prefix == XML_PREFIX || prefix == XMLNS_PREFIX {
            return Err(cite("SR5", format!("XMLNAMESPACES declares no prefix {prefix}")));
        }
        if uri.is_empty() {
            return Err(cite("SR7", format!("the prefix {prefix} is bound to a zero-length URI")));
        }
        if declared(items.get(n.saturating_add(1)..).unwrap_or_default(), Some(prefix)).is_some() {
            return Err(cite("SR4", format!("XMLNAMESPACES declares the prefix {prefix} twice")));
        }
    }
    Ok(())
}

/// The URI `items` bind `prefix` to, or, of none, the default namespace,
/// empty of NO DEFAULT; none if they do not declare it.
fn declared<'a>(items: &'a [NamespaceItem], prefix: Option<&str>) -> Option<&'a str> {
    items.iter().find_map(|item| match (item, prefix) {
        (NamespaceItem::Regular { uri, prefix: p }, Some(wanted)) => (p == wanted).then_some(uri.as_str()),
        (NamespaceItem::Default(uri), None) => Some(uri.as_str()),
        (NamespaceItem::NoDefault, None) => Some(""),
        (NamespaceItem::Regular { .. }, None) | (NamespaceItem::Default(_) | NamespaceItem::NoDefault, Some(_)) => None,
    })
}

impl Binder<'_> {
    /// `bind` run with `items`, if written, in scope, after they are
    /// checked [§11.3].
    pub(super) fn in_namespaces<T>(&self, items: Option<&[NamespaceItem]>, bind: impl FnOnce() -> Result<T>) -> Result<T> {
        let Some(items) = items else { return bind() };
        checked_namespaces(items)?;
        let level = LexicallyScopedOptions { namespaces: Some(items.to_vec()), binary: None };
        self.xml.levels.borrow_mut().push(level);
        let bound = bind();
        self.xml.levels.borrow_mut().pop();
        bound
    }

    /// [§10.20 SR2d] The prefixes the `<XML namespace declaration>`s in
    /// scope bind, those of a query, a statement, a definition or a
    /// compound statement too, each to its URI, the outermost first, so an
    /// inner one overrides; and the default namespace the innermost that
    /// declares one sets, the zero-length string of NO DEFAULT.
    pub(super) fn namespaces_in_scope(&self) -> (Vec<(String, String)>, Option<String>) {
        // Each declaration in scope, the innermost first.
        let found = RefCell::new(Vec::new());
        self.in_xml_scope(|o| {
            found.borrow_mut().extend(o.namespaces.clone());
            None::<()>
        });
        let (mut prefixes, mut default) = (Vec::new(), None);
        for item in found.into_inner().into_iter().rev().flatten() {
            match item {
                NamespaceItem::Regular { uri, prefix } => prefixes.push((prefix, uri)),
                NamespaceItem::Default(uri) => default = Some(uri),
                NamespaceItem::NoDefault => default = Some(String::new()),
            }
        }
        (prefixes, default)
    }

    /// The URI the innermost declaration in scope that declares `prefix`,
    /// or the default namespace if none, binds it to.
    fn declared_uri(&self, prefix: Option<&str>) -> Option<String> {
        self.in_xml_scope(|o| declared(o.namespaces.as_deref()?, prefix).map(str::to_owned))
    }

    /// [§10.12 SR2] The namespace URI of an XML QName of `prefix`, if it
    /// has one, `what` naming it: of `xml`, its namespace \[a ii 1\]; of
    /// another prefix, the URI the innermost declaration in scope binds it
    /// to \[a ii 2 A\], else that of `sqlxml`, `xs` or `xsi` \[a ii 2 B\];
    /// of none, the default namespace in scope, else none \[b\]. None if
    /// it is in no namespace.
    pub(super) fn namespace_uri(&self, prefix: Option<&str>, what: &str) -> Result<Option<Box<str>>> {
        if prefix == Some(XML_PREFIX) {
            return Ok(Some(XML_NAMESPACE.into()));
        }
        let predeclared = [(SQLXML_PREFIX, SQLXML_NAMESPACE), (XS_PREFIX, XS_NAMESPACE), (XSI_PREFIX, XSI_NAMESPACE)];
        let uri = match (self.declared_uri(prefix), prefix) {
            (Some(uri), _) => uri,
            (None, None) => String::new(),
            (None, Some(p)) => match predeclared.iter().find(|(q, _)| *q == p) {
                Some((_, uri)) => (*uri).to_owned(),
                None => {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-14:2006 draft §10.12 SR2a ii 2] the prefix {p} of {what} is declared by no XMLNAMESPACES in scope"
                    )));
                }
            },
        };
        Ok(Some(uri.into_boxed_str()).filter(|u| !u.is_empty()))
    }
}
