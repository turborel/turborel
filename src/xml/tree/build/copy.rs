//! A node of another tree copied into a tree being built [XQuery
//! §3.7.1.3]: each node copied new, the relations among them kept, by the
//! construction mode and the copy-namespaces mode (`Copying`).
//!
//! \[DESIGN\] RULES.md D86: the copy follows the source tree's links, so it
//! does not recurse.

use super::super::{Binding, Kind, NodeId, QName, Tree, XML_PREFIX};
use super::{Builder, ElementStart, Specified};
use crate::error::Result;
use crate::xml::{XML_NAMESPACE, beyond};

/// [XQuery §3.7.1.1] The local name of `xml:id`.
const ID: &str = "id";

/// What a copy keeps of the types of what it copies [XQuery §3.7.1.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Construction {
    /// Construction mode strip: no type annotation, and no copied
    /// attribute an ID but `xml:id`, none an IDREF.
    Strip,
    /// Construction mode preserve: the type annotations, the nilled
    /// property and the ID and IDREFS properties kept.
    Preserve,
    /// The copy of an unvalidated document node [ISO/IEC 9075-14:2006
    /// draft §10.19 GR2]: every element `xdt:untyped` and not nilled, every
    /// attribute `xdt:untypedAtomic`, the ID and IDREFS properties kept.
    Unvalidated,
}

/// How a copy treats the properties of what it copies [XQuery §3.7.1.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Copying {
    /// The construction mode, or §10.19's unvalidated copy.
    pub construction: Construction,
    /// Copy-namespaces mode preserve: a copied element keeps every
    /// binding in its scope; no-preserve those its name and attributes'
    /// names use.
    pub preserve: bool,
    /// Copy-namespaces mode inherit: the copy inherits the bindings of the
    /// element it is copied into; no-inherit undeclares them.
    pub inherit: bool,
}

impl Copying {
    /// Construction mode preserve, copy-namespaces mode preserve and
    /// inherit: the defaults [XQuery Appendix C.1], as XMLDOCUMENT copies
    /// [ISO/IEC 9075-14:2006 draft §6.13 GR5].
    pub const PRESERVE: Copying = Copying { construction: Construction::Preserve, preserve: true, inherit: true };

    /// As `PRESERVE`, but every element `xdt:untyped` and not nilled, every
    /// attribute `xdt:untypedAtomic`: the copy of an unvalidated document
    /// node [ISO/IEC 9075-14:2006 draft §10.19 GR2].
    pub const UNVALIDATED: Copying = Copying { construction: Construction::Unvalidated, ..Copying::PRESERVE };
}

impl Builder {
    /// A copy of node `root` of `tree` and the nodes below it, added to the
    /// open node's children [XQuery §3.7.1.3]: each copied node new, the
    /// relations among them kept; an element's ID and IDREFS properties
    /// kept, as construction mode preserve keeps them [ISO/IEC 9075-14:2006
    /// draft §6.13 GR5]; and the root, an element that had a parent,
    /// declaring every binding in its scope, as copy-namespaces preserve
    /// keeps them [XQueryDM §6.2.1]. A document node's children are copied
    /// in its place. An attribute is no child, so it is not copied here.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if `root` is an attribute, or the tree would nest too
    /// deep or be too big.
    pub fn copy(&mut self, tree: &Tree, root: NodeId) -> Result<()> {
        self.copy_as(tree, root, Copying::PRESERVE)
    }

    /// As `copy`, by the modes `mode` [XQuery §3.7.1.3].
    ///
    /// # Errors
    ///
    /// As `copy`.
    pub fn copy_as(&mut self, tree: &Tree, root: NodeId, mode: Copying) -> Result<()> {
        if tree.kind(root) == Some(Kind::Document) {
            return tree.children(root).try_for_each(|child| self.copy_subtree(tree, child, mode));
        }
        self.copy_subtree(tree, root, mode)
    }

    /// A copy of node `root` of `tree`, no document node, and the nodes
    /// below it, added to the open node's children (`copy_as`).
    fn copy_subtree(&mut self, tree: &Tree, root: NodeId, mode: Copying) -> Result<()> {
        let mut at = Some(root);
        while let Some(id) = at {
            let opened = self.copy_node(tree, id, id == root, mode)?;
            at = match tree.first_child(id).filter(|_| opened) {
                Some(child) => Some(child),
                None => self.copied_after(tree, root, id, opened)?,
            };
        }
        Ok(())
    }

    /// Node `id` of `tree` copied, the root of what is copied if `root`;
    /// whether it opened an element, whose children follow it.
    fn copy_node(&mut self, tree: &Tree, id: NodeId, root: bool, mode: Copying) -> Result<bool> {
        match tree.kind(id) {
            Some(Kind::Element) => {
                let name = tree.name(id).cloned().ok_or_else(|| beyond("an element has no name"))?;
                let attributes = tree.attributes(id).map(|a| copied_attribute(tree, a, mode)).collect::<Result<Vec<_>>>()?;
                let bindings = self.copied_bindings((tree, id), root, mode, &attributes);
                self.start_element(ElementStart { name, bindings, attributes })?;
                if mode.construction == Construction::Preserve {
                    self.copy_annotations(tree, id)?;
                }
                Ok(true)
            }
            Some(Kind::Text) => self.text(tree.content(id)).map(|()| false),
            Some(Kind::Comment) => self.comment(tree.content(id)).map(|()| false),
            Some(Kind::ProcessingInstruction) => {
                let target = tree.name(id).map_or("", |n| &n.local);
                self.processing_instruction(target, tree.content(id)).map(|()| false)
            }
            Some(Kind::Attribute | Kind::Document) | None => Err(beyond("an attribute or document node is no node's child")),
        }
    }

    /// The annotations of element `id` of `tree` and of its attributes
    /// given the element just started and its attributes, as construction
    /// mode preserve keeps them [XQuery §3.7.1.3].
    fn copy_annotations(&mut self, tree: &Tree, id: NodeId) -> Result<()> {
        if !tree.is_typed() {
            return Ok(());
        }
        let Some(element) = self.current() else { return Ok(()) };
        let nodes = std::iter::once((id, element))
            .chain(tree.attributes(id).enumerate().filter_map(|(i, a)| element.nth_attribute(i).map(|c| (a, c))));
        for (source, copy) in nodes {
            if let Some(annotation) = tree.annotation(source) {
                self.annotate(copy, annotation, (tree.nilled(source), tree.is_id(source), tree.is_idrefs(source)))?;
            }
        }
        Ok(())
    }

    /// The bindings a copy of element `id` declares [XQuery §3.7.1.3]: of
    /// preserve, the root's every binding in its scope if it had a parent,
    /// another's its own; of no-preserve, those its name and its
    /// attributes' use; of no-inherit, the root's undeclaring too what the
    /// element it is copied into binds and it does not.
    fn copied_bindings(&self, (tree, id): (&Tree, NodeId), root: bool, mode: Copying, attributes: &[Specified]) -> Vec<Binding> {
        let scope = tree.in_scope(id);
        let mut bindings: Vec<Binding> = if !mode.preserve {
            let names = tree.name(id).into_iter().chain(attributes.iter().map(|a| &a.name));
            let used: Vec<Option<&str>> = names.filter(|n| n.uri.is_some()).map(|n| n.prefix.as_deref()).collect();
            let kept = scope.iter().filter(|(p, _)| *p != Some(XML_PREFIX) && used.contains(p));
            kept.map(|(p, uri)| Binding { prefix: p.map(Into::into), uri: (*uri).into() }).collect()
        } else if root && tree.parent(id).is_some() {
            let kept = scope.iter().filter(|(p, _)| *p != Some(XML_PREFIX));
            kept.map(|(p, uri)| Binding { prefix: p.map(Into::into), uri: (*uri).into() }).collect()
        } else {
            tree.declared(id).to_vec()
        };
        if root {
            // An element of no default namespace keeps none in the element
            // it is copied into, as the absence of one is a binding too
            // [XQuery §3.7.1.3]; of no-inherit, no binding of that element
            // is inherited.
            let open = self.open_scope();
            let default_kept = mode.preserve && !scope.iter().any(|(p, _)| p.is_none());
            for (prefix, _) in open {
                let undeclared = (prefix.is_none() && default_kept) || (!mode.inherit && prefix != Some(XML_PREFIX));
                if undeclared && !bindings.iter().any(|b| b.prefix.as_deref() == prefix) {
                    bindings.push(Binding { prefix: prefix.map(Into::into), uri: "".into() });
                }
            }
        }
        bindings
    }

    /// The bindings in scope of the element open now, none if none is.
    fn open_scope(&self) -> Vec<(Option<&str>, &str)> {
        let open = self.open.last().filter(|_| self.depth() > 0).map(|o| NodeId(o.node));
        open.map(|at| self.tree.in_scope(at)).unwrap_or_default()
    }

    /// The node after `id` in document order below `root`, each element
    /// left on the way closed, `id` too if it opened one with no child;
    /// none at the end of `root`.
    fn copied_after(&mut self, tree: &Tree, root: NodeId, id: NodeId, opened: bool) -> Result<Option<NodeId>> {
        if opened {
            self.end_element()?;
        }
        let mut at = id;
        while at != root {
            if let Some(sibling) = tree.next_sibling(at) {
                return Ok(Some(sibling));
            }
            let Some(parent) = tree.parent(at) else { break };
            self.end_element()?;
            at = parent;
        }
        Ok(None)
    }
}

/// Attribute `id` of `tree`, as an element copied by `mode` specifies it
/// [XQuery §3.7.1.3]: of construction mode strip, an ID only if it is
/// `xml:id`, and no IDREF.
///
/// # Errors
///
/// SQLCODE -900 if it has no name, which no attribute of a tree lacks.
pub fn copied_attribute(tree: &Tree, id: NodeId, mode: Copying) -> Result<Specified> {
    let name = tree.name(id).cloned().ok_or_else(|| beyond("an attribute has no name"))?;
    let stripped = mode.construction == Construction::Strip;
    let (is_id, is_idrefs) = if stripped { (is_xml_id(&name), false) } else { (tree.is_id(id), tree.is_idrefs(id)) };
    Ok(Specified { name, value: tree.content(id).to_owned(), is_id, is_idrefs })
}

/// Whether `name` is `xml:id` [XQuery §3.7.1.1].
#[must_use]
pub fn is_xml_id(name: &QName) -> bool {
    name.uri.as_deref() == Some(XML_NAMESPACE) && &*name.local == ID
}
