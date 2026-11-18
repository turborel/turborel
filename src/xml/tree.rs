//! The nodes of the data model [XQueryDM §6]: document, element,
//! attribute, text, comment and processing instruction nodes, each in a
//! tree of nodes made together. A namespace binding is kept on the
//! element that declares it, as the data model lets an implementation
//! "preserve the relevant bindings" rather than make namespace nodes
//! [XQueryDM §6.2.3, §6.4.1]; the bindings in scope of an element are
//! those of it and its ancestors.
//!
//! \[DESIGN\] RULES.md D86: a tree holds its nodes in document order, an
//! element's attributes right after it and before its children [XQueryDM
//! §2.4], so a node's index is its place in document order. Each node
//! links to its parent, its first child and its next sibling. Element and
//! attribute names, and processing instructions' targets, are tokenized:
//! each is kept once in the tree's table of names, as Pal et al. keep
//! tags (RULES.md §9). The characters of every text, comment, processing
//! instruction and attribute are kept in one buffer of the tree.

mod build;
mod typed;

pub use build::{Builder, Copying, ElementStart, Specified, copied_attribute, is_xml_id};
pub use typed::{Annotation, Typing};

use std::ops::Range;

/// A node of a tree: its index, which is its place in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

impl NodeId {
    /// The root of every tree, its first node: the document node of a
    /// tree a parse makes.
    pub const ROOT: NodeId = NodeId(0);

    /// The node at `index`, if a tree may hold one there.
    fn at(index: usize) -> Option<NodeId> {
        u32::try_from(index).ok().map(NodeId)
    }

    /// Its index in its tree's nodes.
    fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// The kind of a node [XQueryDM §5.10]. Namespace nodes are kept as
/// bindings of their elements (`Binding`) [XQueryDM §6.4.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A document node [§6.1].
    Document,
    /// An element node [§6.2].
    Element,
    /// An attribute node [§6.3].
    Attribute,
    /// A processing instruction node [§6.5].
    ProcessingInstruction,
    /// A comment node [§6.6].
    Comment,
    /// A text node [§6.7].
    Text,
}

/// An expanded QName with the prefix it was written with [XQueryDM §2.1,
/// §3.3.3]: two are the same name if their namespace URIs and local names
/// are, whatever their prefixes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QName {
    /// The prefix, if written with one.
    pub prefix: Option<Box<str>>,
    /// The namespace URI, if the name is in a namespace.
    pub uri: Option<Box<str>>,
    /// The local name.
    pub local: Box<str>,
}

impl QName {
    /// The name `local` in no namespace, of no prefix.
    #[must_use]
    pub fn local(local: &str) -> QName {
        QName { prefix: None, uri: None, local: local.into() }
    }

    /// Whether `self` and `other` are the same expanded QName [XQueryDM
    /// §3.3.3]: their namespace URIs and local names alike.
    #[must_use]
    pub fn same(&self, other: &QName) -> bool {
        self.uri == other.uri && self.local == other.local
    }
}

/// A namespace binding an element declares [XQueryDM §6.2.3]: a prefix,
/// or none for the default namespace, and its URI; an empty URI undeclares
/// the prefix, or the default namespace, in the element's scope
/// [Namespaces 1.0 §5.2] [Namespaces 1.1 §6.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The prefix bound, none for the default namespace.
    pub prefix: Option<Box<str>>,
    /// The URI it is bound to, empty to undeclare it.
    pub uri: Box<str>,
}

/// The prefix `xml`, bound in every element's scope [XQueryDM §6.4.1]
/// [Namespaces 1.0 §4].
pub const XML_PREFIX: &str = "xml";
/// The namespace the prefix `xml` is bound to [Namespaces 1.0 §4].
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace the prefix `xmlns` is bound to, which no other prefix
/// is [Namespaces 1.1 §3].
pub const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
/// [ISO/IEC 9075-14:2006 draft §4.9, Table 2] The prefix `xs` and the
/// namespace of XML Schema's data types.
pub const XS_PREFIX: &str = "xs";
/// The namespace `xs` is bound to.
pub const XS_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema";
/// [Table 2] The prefix `xsi` and the namespace of XML Schema's
/// attributes of instances, as `xsi:nil`.
pub const XSI_PREFIX: &str = "xsi";
/// The namespace `xsi` is bound to.
pub const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// [Table 2] The prefix `sqlxml` and SQL/XML's namespace.
pub const SQLXML_PREFIX: &str = "sqlxml";
/// The namespace `sqlxml` is bound to, as Table 2 writes it; §10.12 SR2a
/// ii 2 B I and §10.20 SR2b write it without the slash between `iso` and
/// `9075`.
pub const SQLXML_NAMESPACE: &str = "http://standards.iso.org/iso/9075/2003/sqlxml";

/// The index of no node or name: a node with no parent, child, sibling or
/// name.
const NONE: u32 = u32::MAX;

/// A node of a tree.
#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    /// Its name or target, an index into the tree's names, or `NONE`.
    name: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    /// The nodes of its attributes, which follow it, for an element.
    attributes: u32,
    /// Its bindings, a range of the tree's, for an element.
    bindings: Range<u32>,
    /// Its characters, a range of the tree's text: a text, comment or
    /// attribute node's content, a processing instruction's.
    text: Range<u32>,
    /// Whether it is an ID [XQueryDM §5.5].
    is_id: bool,
    /// Whether it is an IDREF or IDREFS [XQueryDM §5.6].
    is_idrefs: bool,
    /// Whether it is nilled [XQueryDM §5.9], of an element.
    nilled: bool,
    /// Its type annotation, an index into the tree's annotations, or
    /// `NONE` of a node no schema typed: `xdt:untyped` of an element,
    /// `xdt:untypedAtomic` of an attribute [XQueryDM §6.2.3, §6.3.3].
    annotation: u32,
}

/// A tree of nodes [XQueryDM §6]: what a parse, or a constructor, makes.
/// It never changes once made.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    names: Vec<QName>,
    bindings: Vec<Binding>,
    text: String,
    /// The type annotations of its nodes [XQueryDM §3.3.1], each once.
    annotations: Vec<Annotation>,
}

/// An index of a tree's tables as a `usize`.
fn wide(index: u32) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

impl Tree {
    /// The node `id`, if the tree has it.
    fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// The node linked as `link`, if one is.
    fn linked(&self, link: u32) -> Option<NodeId> {
        (link != NONE).then_some(NodeId(link)).filter(|id| id.index() < self.nodes.len())
    }

    /// \[DESIGN\] How many octets it holds, about: its nodes, names,
    /// bindings and text, for what a spill counts.
    #[must_use]
    pub fn held_bytes(&self) -> usize {
        let names: usize = self.names.iter().map(|n| n.local.len().saturating_add(n.uri.as_ref().map_or(0, |u| u.len()))).sum();
        let bindings: usize = self.bindings.iter().map(|b| b.uri.len().saturating_add(size_of::<Binding>())).sum();
        self.nodes.len().saturating_mul(size_of::<Node>()).saturating_add(names).saturating_add(bindings).saturating_add(self.text.len())
    }

    /// How many nodes it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether it has no node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The kind of `id` [XQueryDM §5.10], if the tree has it.
    #[must_use]
    pub fn kind(&self, id: NodeId) -> Option<Kind> {
        self.node(id).map(|n| n.kind)
    }

    /// The name of element or attribute `id` [XQueryDM §5.11], or the
    /// target of processing instruction `id` as a name of no namespace
    /// [XQueryDM §6.5.2]; none of another node.
    #[must_use]
    pub fn name(&self, id: NodeId) -> Option<&QName> {
        self.node(id).and_then(|n| self.names.get(wide(n.name)))
    }

    /// The parent of `id` [XQueryDM §5.12], if it has one.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).and_then(|n| self.linked(n.parent))
    }

    /// The first child of `id`, if it has children.
    #[must_use]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).and_then(|n| self.linked(n.first_child))
    }

    /// The next sibling of `id`, if it has one: the child after it of its
    /// parent.
    #[must_use]
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).and_then(|n| self.linked(n.next_sibling))
    }

    /// The children of `id` [XQueryDM §5.3], in document order.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.first_child(id), move |&c| self.next_sibling(c))
    }

    /// The attributes of element `id` [XQueryDM §5.1], in the order they
    /// were made, which is stable.
    pub fn attributes(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let count = self.node(id).map_or(0, |n| wide(n.attributes));
        let first = id.index().saturating_add(1);
        (first..first.saturating_add(count)).filter_map(NodeId::at)
    }

    /// The bindings element `id` declares.
    #[must_use]
    pub fn declared(&self, id: NodeId) -> &[Binding] {
        self.node(id).and_then(|n| self.bindings.get(wide(n.bindings.start)..wide(n.bindings.end))).unwrap_or_default()
    }

    /// The characters of text, comment, attribute or processing
    /// instruction `id`: its content [XQueryDM §6.3, §6.5, §6.6, §6.7];
    /// none of another node.
    #[must_use]
    pub fn content(&self, id: NodeId) -> &str {
        self.node(id).and_then(|n| self.text.get(wide(n.text.start)..wide(n.text.end))).unwrap_or_default()
    }

    /// Whether `id` is an ID [XQueryDM §5.5].
    #[must_use]
    pub fn is_id(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.is_id)
    }

    /// Whether `id` is an IDREF or IDREFS [XQueryDM §5.6].
    #[must_use]
    pub fn is_idrefs(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.is_idrefs)
    }

    /// Whether element `id` is nilled [XQueryDM §5.9].
    #[must_use]
    pub fn nilled(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.nilled)
    }

    /// The type annotation of element or attribute `id` [XQueryDM §5.14],
    /// none of one no schema typed: `xdt:untyped` of an element,
    /// `xdt:untypedAtomic` of an attribute [XQueryDM §6.2.3, §6.3.3].
    #[must_use]
    pub fn annotation(&self, id: NodeId) -> Option<&Annotation> {
        self.node(id).and_then(|n| self.annotations.get(wide(n.annotation)))
    }

    /// Whether any node of it is typed, as only a validation makes one.
    #[must_use]
    pub fn is_typed(&self) -> bool {
        !self.annotations.is_empty()
    }

    /// The string value of `id` [XQueryDM §5.13]: of a document or element
    /// node, the contents of its text node descendants in document order
    /// [§6.1.3, §6.2.3]; of another node, its content.
    #[must_use]
    pub fn string_value(&self, id: NodeId) -> String {
        match self.kind(id) {
            Some(Kind::Document | Kind::Element) => {
                self.descendants(id).filter(|&d| self.kind(d) == Some(Kind::Text)).map(|d| self.content(d)).collect()
            }
            Some(Kind::Attribute | Kind::ProcessingInstruction | Kind::Comment | Kind::Text) => self.content(id).to_owned(),
            None => String::new(),
        }
    }

    /// The descendants of `id` but its attributes, in document order: the
    /// nodes after it in the tree up to the first that is not below it.
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let end = self.following(id).map_or(self.nodes.len(), NodeId::index);
        (id.index().saturating_add(1)..end).filter_map(NodeId::at).filter(move |&d| self.kind(d) != Some(Kind::Attribute))
    }

    /// Its nodes, in document order.
    pub fn nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).filter_map(NodeId::at)
    }

    /// The first node after `id` in document order that is not below it:
    /// its next sibling, or the next sibling of the nearest ancestor that
    /// has one.
    #[must_use]
    pub fn following(&self, id: NodeId) -> Option<NodeId> {
        let mut at = Some(id);
        while let Some(node) = at {
            let n = self.node(node)?;
            if let Some(next) = self.linked(n.next_sibling) {
                return Some(next);
            }
            at = self.linked(n.parent);
        }
        None
    }

    /// The namespace bindings in scope of element `id` [XQueryDM §5.7,
    /// §6.4.1]: `xml`'s, then each of its ancestors' declarations from the
    /// root down, the nearer overriding; an undeclared prefix, or default
    /// namespace, is not in scope.
    #[must_use]
    pub fn in_scope(&self, id: NodeId) -> Vec<(Option<&str>, &str)> {
        let mut chain: Vec<NodeId> = std::iter::successors(Some(id), |&a| self.parent(a)).collect();
        chain.reverse();
        let mut scope: Vec<(Option<&str>, &str)> = vec![(Some(XML_PREFIX), XML_NAMESPACE)];
        for binding in chain.into_iter().flat_map(|a| self.declared(a)) {
            scope.retain(|(prefix, _)| *prefix != binding.prefix.as_deref());
            if !binding.uri.is_empty() {
                scope.push((binding.prefix.as_deref(), &binding.uri));
            }
        }
        scope
    }
}

#[cfg(test)]
mod tests;
