//! The top-level definitions and declarations of a schema's documents, by
//! symbol space and name [Schema1 §2.5, §3.15.2], with those a
//! `<redefine>` replaces [Schema1 §4.2.2]; and the identity-constraint
//! definitions, whose names are one symbol space for the schema [Schema1
//! §3.11.1].

use std::collections::BTreeMap;

use super::super::component::Expanded;
use super::super::invalid;
use super::attrs::At;
use super::documents::Doc;
use crate::error::Result;
use crate::xml::{Kind as NodeKind, NodeId};

/// A symbol space of top-level components [Schema1 §2.5]: type
/// definitions, simple and complex together, and each other kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Space {
    /// Type definitions.
    Type,
    /// Element declarations.
    Element,
    /// Attribute declarations.
    Attribute,
    /// Attribute group definitions.
    AttributeGroup,
    /// Model group definitions.
    Group,
    /// Notation declarations.
    Notation,
}

impl Space {
    /// The symbol space of a top-level element `local` of `<schema>` or
    /// `<redefine>`, if it represents a named component.
    fn of(local: &str) -> Option<Space> {
        Some(match local {
            "simpleType" | "complexType" => Space::Type,
            "element" => Space::Element,
            "attribute" => Space::Attribute,
            "attributeGroup" => Space::AttributeGroup,
            "group" => Space::Group,
            "notation" => Space::Notation,
            _ => return None,
        })
    }
}

/// Where a top-level component is represented: a document's place and its
/// element; for one a `<redefine>` replaces, the definition it replaces
/// too [Schema1 §4.2.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Entry {
    /// The document.
    pub doc: usize,
    /// The element.
    pub node: NodeId,
    /// The definition redefined, of a redefinition.
    pub original: Option<(usize, NodeId)>,
}

/// The top-level components of a schema's documents, by space and name.
#[derive(Debug, Clone, Default)]
pub(super) struct Index {
    /// The components.
    pub entries: BTreeMap<(Space, Expanded), Entry>,
    /// The identity-constraint definitions, by name: the document and the
    /// `<key>`, `<keyref>` or `<unique>` element of each.
    pub constraints: BTreeMap<Expanded, (usize, NodeId)>,
}

/// The local names of the elements of identity-constraint definitions
/// [Schema1 §3.11.2].
const IDENTITY: [&str; 3] = ["key", "keyref", "unique"];

impl Index {
    /// The index of `docs`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if two components of a space have one name [Schema1
    /// §3.15.6 sch-props-correct.2], or a redefinition names no component
    /// of the document it redefines [Schema1 §4.2.2 src-redefine].
    pub fn of(docs: &[Doc]) -> Result<Index> {
        let mut index = Index::default();
        for (place, doc) in docs.iter().enumerate() {
            for (child, local) in At::new(&doc.tree, doc.root).children()? {
                let Some(space) = Space::of(local) else { continue };
                index.insert(space, doc, &child, Entry { doc: place, node: child.node, original: None })?;
            }
            index.constraints_of(place, doc)?;
        }
        for (place, doc) in docs.iter().enumerate() {
            for &(node, target) in &doc.redefines {
                index.redefine(docs, (place, doc), node, target)?;
            }
        }
        Ok(index)
    }

    /// `entry`, of `space`, named by the name attribute of `at` in `doc`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has no name, or one of its space has it.
    fn insert(&mut self, space: Space, doc: &Doc, at: &At<'_>, entry: Entry) -> Result<()> {
        let name = name_of(doc, at)?;
        if self.entries.insert((space, name.clone()), entry).is_some() {
            return Err(invalid("[Schema1 §3.15.6 sch-props-correct.2]", &format!("two top-level components are named {}", name.text())));
        }
        Ok(())
    }

    /// The identity-constraint definitions of `doc`, at `place`, by name.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if two have one name [Schema1 §3.11.1].
    fn constraints_of(&mut self, place: usize, doc: &Doc) -> Result<()> {
        let tree = &doc.tree;
        for node in tree.descendants(doc.root).filter(|&n| tree.kind(n) == Some(NodeKind::Element)) {
            let at = At::new(tree, node);
            if !at.xs_name().is_some_and(|l| IDENTITY.contains(&l)) {
                continue;
            }
            let name = name_of(doc, &at)?;
            if self.constraints.insert(name.clone(), (place, node)).is_some() {
                return Err(invalid("[Schema1 §3.11.1]", &format!("two identity-constraint definitions are named {}", name.text())));
            }
        }
        Ok(())
    }

    /// The definitions of `<redefine>` `node` of document `by` put in place
    /// of those of the same name of document `target` [Schema1 §4.2.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if one names no top-level definition of its kind in
    /// `target` [Schema1 §4.2.2 src-redefine], or two redefine one.
    fn redefine(&mut self, docs: &[Doc], (place, by): (usize, &Doc), node: NodeId, target: usize) -> Result<()> {
        for (child, local) in At::new(&by.tree, node).children()? {
            let Some(space) = Space::of(local) else { continue };
            let name = name_of(by, &child)?;
            let replaced = self.entries.get(&(space, name.clone())).copied().filter(|e| e.doc == target && e.original.is_none());
            let Some(replaced) = replaced.filter(|_| docs.get(target).is_some()) else {
                return Err(invalid(
                    "[Schema1 §4.2.2 src-redefine]",
                    &format!("a redefinition of {} names a top-level definition of the document it redefines", name.text()),
                ));
            };
            let entry = Entry { doc: place, node: child.node, original: Some((replaced.doc, replaced.node)) };
            self.entries.insert((space, name), entry);
        }
        Ok(())
    }

    /// The component of `space` named `name`, if the schema has one.
    pub fn get(&self, space: Space, name: &Expanded) -> Option<Entry> {
        self.entries.get(&(space, name.clone())).copied()
    }
}

/// The {name} and {target namespace} of the top-level component `at` of
/// `doc` [Schema1 §3.x.2]: its name attribute, of the document's target
/// namespace.
///
/// # Errors
///
/// SQLSTATE 22000 if it has no name, or one that is no NCName.
fn name_of(doc: &Doc, at: &At<'_>) -> Result<Expanded> {
    let name = at.ncname("name")?.ok_or_else(|| invalid("[Schema1 Appendix A]", &format!("a top-level <{}> has a name", at.local())))?;
    Ok(Expanded::new(doc.tns.as_deref(), &name))
}
