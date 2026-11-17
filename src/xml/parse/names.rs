//! Names as Namespaces in XML reads them [ISO/IEC 9075-14:2006 draft
//! §10.16 GR2, GR3]: each element's namespace declarations, its name and
//! its attributes' names resolved by the declarations in scope, and the
//! constraints on them [Namespaces 1.0 §2 to §6] [Namespaces 1.1 §3 to
//! §7]. Namespaces 1.1 numbers from §3 one section further than 1.0, so
//! each rule is cited by the version of the text being read.

use std::collections::HashSet;
use std::rc::Rc;

use super::Parser;
use super::attlist::Kind as AttKind;
use crate::error::Result;
use crate::xml::chars::{COLON, Version, is_ncname};
use crate::xml::tree::{Binding, ElementStart, QName, Specified, XML_NAMESPACE, XML_PREFIX};

/// The attribute that declares the default namespace, and the prefix of
/// those that declare a prefix [Namespaces 1.0 §2 \[1\] to \[3\]] [Namespaces 1.1 §3 \[1\] to \[3\]].
pub(super) const XMLNS: &str = "xmlns";
/// [Namespaces 1.1 §3] The namespace the prefix `xmlns` is bound to.
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
/// [XQueryDM §6.3.3] The local name of `xml:id`.
const ID: &str = "id";

/// An attribute as its start tag gives it, or its declaration's default
/// does: its name as written, its value normalized by its type, which
/// the declaration gives, CDATA if none does [XML §3.3.3].
#[derive(Debug, Clone)]
pub(super) struct RawAttribute {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) kind: AttKind,
}

/// The namespace bindings in scope: the declarations of each open
/// element, its own after its ancestors'.
#[derive(Debug, Default)]
pub(super) struct Scope {
    bindings: Vec<(Option<Rc<str>>, Rc<str>)>,
    marks: Vec<usize>,
}

impl Scope {
    /// An element's declarations brought into scope.
    fn push(&mut self, declared: &[Binding]) {
        self.marks.push(self.bindings.len());
        self.bindings.extend(declared.iter().map(|b| (b.prefix.as_deref().map(Rc::from), Rc::from(&*b.uri))));
    }

    /// The last element's declarations taken out of scope.
    pub(super) fn pop(&mut self) {
        if let Some(mark) = self.marks.pop() {
            self.bindings.truncate(mark);
        }
    }

    /// The URI `prefix`, or the default namespace if none, is bound to;
    /// none if it is not bound, or undeclared [Namespaces 1.1 §6.1].
    fn resolve(&self, prefix: Option<&str>) -> Option<&str> {
        if prefix == Some(XML_PREFIX) {
            return Some(XML_NAMESPACE);
        }
        self.bindings.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, uri)| &**uri).filter(|uri| !uri.is_empty())
    }
}

impl Parser {
    /// `v10` if the text is XML 1.0, else `v11`: a rule of Namespaces in
    /// XML cited by the version that governs it.
    fn ns(&self, v10: &'static str, v11: &'static str) -> &'static str {
        match self.version {
            Version::V10 => v10,
            Version::V11 => v11,
        }
    }

    /// `raw` split as a `QName` into its prefix, if any, and local part
    /// [Namespaces 1.0 §3 \[6\]] [Namespaces 1.1 §4 \[6\]].
    pub(super) fn qname<'a>(&self, raw: &'a str) -> Result<(Option<&'a str>, &'a str)> {
        let split = match raw.split_once(COLON) {
            None => Some((None, raw)),
            Some((prefix, local)) => (is_ncname(self.version, prefix) && is_ncname(self.version, local)).then_some((Some(prefix), local)),
        };
        split.ok_or_else(|| {
            let rule = self.ns("[Namespaces 1.0 §3 [6]]", "[Namespaces 1.1 §4 [6]]");
            self.malformed(&format!("'{raw}' is no qualified name {rule}"))
        })
    }

    /// The error unless `raw`, the name of an entity, a processing
    /// instruction's target or a notation, has no colon [Namespaces 1.0
    /// §6] [Namespaces 1.1 §7].
    pub(super) fn colon_free(&self, raw: &str) -> Result<()> {
        if raw.contains(COLON) {
            let rule = self.ns("[Namespaces 1.0 §6]", "[Namespaces 1.1 §7]");
            return Err(self.malformed(&format!("the name '{raw}' has a colon {rule}")));
        }
        Ok(())
    }

    /// The element of name `raw` and attributes `attributes` as the data
    /// model has it: its namespace declarations taken out of its
    /// attributes and brought into scope, and its names resolved
    /// [XQueryDM §6.2.3, §6.3.3].
    pub(super) fn element_start(&mut self, raw: &str, attributes: Vec<RawAttribute>) -> Result<ElementStart> {
        let (declarations, others): (Vec<RawAttribute>, Vec<RawAttribute>) =
            attributes.into_iter().partition(|a| a.name == XMLNS || a.name.strip_prefix(XMLNS).is_some_and(|r| r.starts_with(COLON)));
        let bindings: Vec<Binding> =
            declarations.iter().map(|d| self.binding(d)).collect::<Result<Vec<_>>>()?.into_iter().flatten().collect();
        self.scope.push(&bindings);
        let name = self.resolved(raw, true)?;
        let mut seen = HashSet::new();
        let mut specified = Vec::with_capacity(others.len());
        for attribute in others {
            let attribute_name = self.resolved(&attribute.name, false)?;
            if !seen.insert((attribute_name.uri.clone(), attribute_name.local.clone())) {
                let rule = self.ns("[Namespaces 1.0 §5.3]", "[Namespaces 1.1 §6.3]");
                return Err(
                    self.malformed(&format!("two attributes of <{raw}> are both '{}' in one namespace {rule}", attribute_name.local))
                );
            }
            specified.push(identified(attribute_name, attribute));
        }
        Ok(ElementStart { name, bindings, attributes: specified })
    }

    /// The binding the namespace declaration `declaration` makes, none if
    /// it binds `xml` to its own namespace, which is bound already.
    fn binding(&self, declaration: &RawAttribute) -> Result<Option<Binding>> {
        let prefix = declaration.name.strip_prefix(XMLNS).and_then(|r| r.strip_prefix(COLON));
        let uri = declaration.value.as_str();
        let reserved =
            self.ns("[Namespaces 1.0 §4 NSC: Prefix Declared]", "[Namespaces 1.1 §3 NSC: Reserved Prefixes and Namespace Names]");
        if let Some(p) = prefix {
            if !is_ncname(self.version, p) {
                let rule = self.ns("[Namespaces 1.0 §2 [2]]", "[Namespaces 1.1 §3 [2]]");
                return Err(self.malformed(&format!("'{p}' is no prefix {rule}")));
            }
            if p == XMLNS {
                return Err(self.malformed(&format!("the prefix xmlns is never declared {reserved}")));
            }
            if p == XML_PREFIX {
                return if uri == XML_NAMESPACE {
                    Ok(None)
                } else {
                    Err(self.malformed(&format!("the prefix xml is bound to {XML_NAMESPACE} alone {reserved}")))
                };
            }
            if uri.is_empty() && self.version == Version::V10 {
                return Err(self.malformed(&format!("the prefix '{p}' is declared with no namespace name [Namespaces 1.0 §2]")));
            }
        }
        if uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE {
            return Err(self.malformed(&format!("no prefix but xml or xmlns is bound to {uri} {reserved}")));
        }
        Ok(Some(Binding { prefix: prefix.map(Box::from), uri: uri.into() }))
    }

    /// The expanded name of `raw`, an element's if `element`, else an
    /// attribute's: its prefix bound in scope, and an element's name
    /// without one in the default namespace [Namespaces 1.0 §4, §5.2]
    /// [Namespaces 1.1 §5, §6.2].
    fn resolved(&self, raw: &str, element: bool) -> Result<QName> {
        let (prefix, local) = self.qname(raw)?;
        let uri = match prefix {
            Some(XMLNS) => {
                let rule =
                    self.ns("[Namespaces 1.0 §4 NSC: Prefix Declared]", "[Namespaces 1.1 §3 NSC: Reserved Prefixes and Namespace Names]");
                return Err(self.malformed(&format!("the prefix xmlns names no element or attribute {rule}")));
            }
            Some(p) => Some(self.scope.resolve(Some(p)).ok_or_else(|| {
                let rule = self.ns("[Namespaces 1.0 §4 NSC: Prefix Declared]", "[Namespaces 1.1 §5 NSC: Prefix Declared]");
                self.malformed(&format!("the prefix '{p}' of '{raw}' is not declared {rule}"))
            })?),
            None => {
                if element {
                    self.scope.resolve(None)
                } else {
                    None
                }
            }
        };
        Ok(QName { prefix: prefix.map(Box::from), uri: uri.map(Box::from), local: local.into() })
    }
}

/// The attribute `raw`, of expanded name `name`, as an attribute node:
/// an ID if declared one, or named `xml:id`, whose value is then
/// normalized as an ID's [XQueryDM §6.3.3]; an IDREF or
/// IDREFS if declared one.
fn identified(name: QName, raw: RawAttribute) -> Specified {
    let xml_id = name.uri.as_deref() == Some(XML_NAMESPACE) && &*name.local == ID;
    let value = if xml_id && raw.kind == AttKind::Cdata { super::attlist::tokenized(&raw.value) } else { raw.value };
    Specified { name, value, is_id: xml_id || raw.kind == AttKind::Id, is_idrefs: raw.kind == AttKind::References }
}
