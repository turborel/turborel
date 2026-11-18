//! The attributes of the elements of a schema document read as their
//! types in the Schema for Schemas give their actual values [Schema1
//! §3.1.3, Appendix A]: names, booleans, occurrence bounds, derivation
//! sets and forms, each read from its normalized value.

use super::super::component::Derivations;
use super::super::invalid;
use crate::error::Result;
use crate::xml::{Kind, NodeId, Tree, Version, WhiteSpace, XS_NAMESPACE, is_ncname, normalize};

/// [Schema1 Appendix A] `#all`, every method of a derivation set.
const ALL: &str = "#all";
/// [Schema1 §3.9.2] maxOccurs's value of no bound.
const UNBOUNDED: &str = "unbounded";
/// [Schema2 §3.2.2.1] The literals of true.
const TRUE: [&str; 2] = ["true", "1"];
/// [Schema2 §3.2.2.1] The literals of false.
const FALSE: [&str; 2] = ["false", "0"];
/// The literal of each derivation method and the method.
const METHODS: [(&str, Derivations); 5] = [
    ("extension", Derivations::EXTENSION),
    ("restriction", Derivations::RESTRICTION),
    ("substitution", Derivations::SUBSTITUTION),
    ("list", Derivations::LIST),
    ("union", Derivations::UNION),
];

/// An element of a schema document: its tree and node.
#[derive(Debug, Clone, Copy)]
pub(super) struct At<'a> {
    /// The tree.
    pub tree: &'a Tree,
    /// The element.
    pub node: NodeId,
}

impl<'a> At<'a> {
    /// The element `node` of `tree`.
    pub fn new(tree: &'a Tree, node: NodeId) -> At<'a> {
        At { tree, node }
    }

    /// Its local name, if it is an element of the XML Schema namespace.
    pub fn xs_name(&self) -> Option<&'a str> {
        let name = self.tree.name(self.node)?;
        (name.uri.as_deref() == Some(XS_NAMESPACE)).then_some(&*name.local)
    }

    /// The local name of its name, for messages.
    pub fn local(&self) -> &'a str {
        self.tree.name(self.node).map_or("", |n| &*n.local)
    }

    /// Its children that are elements, in order, and the local name of
    /// each.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if one is not of the XML Schema namespace [Schema1
    /// §3.1.2], or text other than white space is among them [Schema1
    /// Appendix A].
    pub fn children(&self) -> Result<Vec<(At<'a>, &'a str)>> {
        let mut out = Vec::new();
        for child in self.tree.children(self.node) {
            match self.tree.kind(child) {
                Some(Kind::Element) => {
                    let at = At::new(self.tree, child);
                    let local = at
                        .xs_name()
                        .ok_or_else(|| invalid("[Schema1 §3.1.2]", &format!("<{}> is not of the XML Schema namespace", at.local())))?;
                    out.push((at, local));
                }
                Some(Kind::Text) => {
                    if !self.tree.content(child).chars().all(crate::xml::is_space) {
                        return Err(invalid("[Schema1 Appendix A]", &format!("<{}> holds text", self.local())));
                    }
                }
                Some(Kind::Comment | Kind::ProcessingInstruction | Kind::Attribute | Kind::Document) | None => {}
            }
        }
        Ok(out)
    }

    /// The value of its attribute `local` of no namespace, if it has one.
    pub fn attr(&self, local: &str) -> Option<&'a str> {
        self.tree
            .attributes(self.node)
            .find(|&a| self.tree.name(a).is_some_and(|n| n.uri.is_none() && &*n.local == local))
            .map(|a| self.tree.content(a))
    }

    /// Its attribute `local`, collapsed, as its type's normalized value.
    pub fn token(&self, local: &str) -> Option<String> {
        self.attr(local).map(|v| normalize(v, WhiteSpace::Collapse))
    }

    /// Its attribute `local`, an NCName [Schema1 Appendix A].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it is no NCName.
    pub fn ncname(&self, local: &str) -> Result<Option<String>> {
        let Some(name) = self.token(local) else { return Ok(None) };
        if !is_ncname(Version::V11, &name) {
            return Err(self.bad(local, &name));
        }
        Ok(Some(name))
    }

    /// Its attribute `local`, an `xs:boolean`, else `default`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it is no boolean.
    pub fn boolean(&self, local: &str, default: bool) -> Result<bool> {
        let Some(value) = self.token(local) else { return Ok(default) };
        if TRUE.contains(&value.as_str()) {
            Ok(true)
        } else if FALSE.contains(&value.as_str()) {
            Ok(false)
        } else {
            Err(self.bad(local, &value))
        }
    }

    /// Its minOccurs and maxOccurs [Schema1 §3.9.2]: 1 each if absent,
    /// maxOccurs none if unbounded.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if one is no `xs:nonNegativeInteger` (or
    /// `unbounded`), or is beyond what 64 bits hold.
    pub fn occurs(&self) -> Result<(u64, Option<u64>)> {
        let min = match self.token("minOccurs") {
            Some(v) => v.parse::<u64>().map_err(|_| self.bad("minOccurs", &v))?,
            None => 1,
        };
        let max = match self.token("maxOccurs").as_deref() {
            None => Some(1),
            Some(UNBOUNDED) => None,
            Some(v) => Some(v.parse::<u64>().map_err(|_| self.bad("maxOccurs", v))?),
        };
        Ok((min, max))
    }

    /// Its attribute `local`, a derivation set of `#all` or a list of
    /// methods [Schema1 Appendix A], of the methods `allowed`; `default`
    /// if absent. `#all` is every method of `allowed`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it names a method not of `allowed`.
    pub fn derivations(&self, local: &str, allowed: Derivations, default: Derivations) -> Result<Derivations> {
        let Some(value) = self.token(local) else { return Ok(default) };
        derivation_set(&value, allowed).ok_or_else(|| self.bad(local, &value))
    }

    /// Its attribute `local` as `derivations` reads it, but `#all` the
    /// methods `all`, wider than those it may list: of a finalDefault, as
    /// a simple type's {final} reads it, and of a simple type's final
    /// [Schema1 §3.14.2].
    ///
    /// # Errors
    ///
    /// As `derivations`.
    pub fn derivations_all(&self, local: &str, (allowed, all): (Derivations, Derivations), default: Derivations) -> Result<Derivations> {
        if self.token(local).as_deref() == Some(ALL) {
            return Ok(all);
        }
        self.derivations(local, allowed, default)
    }

    /// The error of attribute `local` of value `value`, not of its type.
    pub fn bad(&self, local: &str, value: &str) -> crate::error::SqlError {
        invalid("[Schema1 Appendix A]", &format!("'{value}' is no value of {local} of <{}>", self.local()))
    }

    /// The namespace bindings in scope of it [Schema1 §3.15.3].
    pub fn scope(&self) -> Vec<(Option<&'a str>, &'a str)> {
        self.tree.in_scope(self.node)
    }
}

/// The derivation set `value`, `#all` or methods separated by spaces, of
/// the methods `allowed`; none if it names another.
pub(super) fn derivation_set(value: &str, allowed: Derivations) -> Option<Derivations> {
    if value == ALL {
        return Some(allowed);
    }
    let mut set = Derivations::NONE;
    for word in value.split(' ').filter(|w| !w.is_empty()) {
        let (_, method) = METHODS.iter().find(|(name, m)| *name == word && allowed.has(*m))?;
        set = set.with(*method);
    }
    Some(set)
}
