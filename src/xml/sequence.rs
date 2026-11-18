//! XML values: XQuery sequences [ISO/IEC 9075-14:2006 draft §4.2.3], each
//! item a node of a tree or an atomic value [XQueryDM §2.5].
//!
//! \[DESIGN\] RULES.md D86: a node is its tree, shared, and its place in
//! it, so the nodes of one tree in many values are the same nodes, and
//! their identity holds [XQueryDM §2.3]; a copy makes a tree of its own
//! (§10.18).

use std::sync::Arc;

use super::tree::{Kind, NodeId, Tree};

/// A node of a tree [XQueryDM §6].
#[derive(Debug, Clone)]
pub struct Node {
    tree: Arc<Tree>,
    id: NodeId,
}

impl Node {
    /// The node `id` of `tree`.
    #[must_use]
    pub fn new(tree: Arc<Tree>, id: NodeId) -> Node {
        Node { tree, id }
    }

    /// The root of `tree`: the document node, of a tree a parse makes.
    #[must_use]
    pub fn root(tree: Tree) -> Node {
        Node { tree: Arc::new(tree), id: NodeId::ROOT }
    }

    /// Its tree.
    #[must_use]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Its place in its tree.
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Its kind, a document node's if its tree does not have it, which no
    /// node made by `new` or `root` is.
    #[must_use]
    pub fn kind(&self) -> Option<Kind> {
        self.tree.kind(self.id)
    }

    /// Whether `self` and `other` are the same node [XQueryDM §2.3]
    /// [ISO/IEC 9075-14:2006 draft §10.6 GR1c].
    #[must_use]
    pub fn is(&self, other: &Node) -> bool {
        Arc::ptr_eq(&self.tree, &other.tree) && self.id == other.id
    }

    /// The node `id` of its tree.
    #[must_use]
    pub fn at(&self, id: NodeId) -> Node {
        Node { tree: Arc::clone(&self.tree), id }
    }

    /// Where `self` is in document order beside `other` [XQueryDM §2.4]:
    /// of one tree, by their places in it; of two, by where their trees
    /// are held. \[DESIGN\] RULES.md D86: the order of trees is
    /// implementation-dependent, but stable and all of a tree's nodes
    /// together [XQuery §2.4.1], as a tree is held at one address while a
    /// node of it is, and every node holds its tree.
    #[must_use]
    pub fn document_order(&self, other: &Node) -> std::cmp::Ordering {
        if Arc::ptr_eq(&self.tree, &other.tree) {
            return self.id.cmp(&other.id);
        }
        Arc::as_ptr(&self.tree).addr().cmp(&Arc::as_ptr(&other.tree).addr())
    }
}

/// The type of an atomic value [XQueryDM §2.6.2]: each atomic built-in
/// datatype of XML Schema [Schema2 §3.2, §3.3], and the three XQuery adds,
/// `xdt:untypedAtomic` and the two durations (`xml/atomic/types.rs`). A
/// value of a type an XML Schema derives from one is of the built-in type
/// it is derived from, with the facets checked (`xml/schema.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicType {
    /// `xs:string`.
    String,
    /// `xdt:untypedAtomic`, the type of the value of an attribute or
    /// element no schema types [XQueryDM §6.2.3, §6.3.3].
    UntypedAtomic,
    /// `xs:boolean` [Schema2 §3.2.2].
    Boolean,
    /// `xs:decimal` [Schema2 §3.2.3].
    Decimal,
    /// `xs:integer` [Schema2 §3.3.13].
    Integer,
    /// `xs:float` [Schema2 §3.2.4].
    Float,
    /// `xs:double` [Schema2 §3.2.5].
    Double,
    /// `xs:date` [Schema2 §3.2.9].
    Date,
    /// `xs:time` [Schema2 §3.2.8].
    Time,
    /// `xs:dateTime` [Schema2 §3.2.7].
    DateTime,
    /// `xdt:yearMonthDuration` [XQueryFO §10.3.1].
    YearMonthDuration,
    /// `xdt:dayTimeDuration` [XQueryFO §10.3.2].
    DayTimeDuration,
    /// `xs:hexBinary` [Schema2 §3.2.15].
    HexBinary,
    /// `xs:duration` [Schema2 §3.2.6].
    Duration,
    /// `xs:gYearMonth` [Schema2 §3.2.10].
    GYearMonth,
    /// `xs:gYear` [Schema2 §3.2.11].
    GYear,
    /// `xs:gMonthDay` [Schema2 §3.2.12].
    GMonthDay,
    /// `xs:gDay` [Schema2 §3.2.13].
    GDay,
    /// `xs:gMonth` [Schema2 §3.2.14].
    GMonth,
    /// `xs:base64Binary` [Schema2 §3.2.16].
    Base64Binary,
    /// `xs:anyURI` [Schema2 §3.2.17].
    AnyUri,
    /// `xs:QName` [Schema2 §3.2.18].
    QName,
    /// `xs:NOTATION` [Schema2 §3.2.19].
    Notation,
    /// `xs:normalizedString` [Schema2 §3.3.1].
    NormalizedString,
    /// `xs:token` [Schema2 §3.3.2].
    Token,
    /// `xs:language` [Schema2 §3.3.3].
    Language,
    /// `xs:NMTOKEN` [Schema2 §3.3.4].
    NmToken,
    /// `xs:Name` [Schema2 §3.3.6].
    Name,
    /// `xs:NCName` [Schema2 §3.3.7].
    NcName,
    /// `xs:ID` [Schema2 §3.3.8].
    Id,
    /// `xs:IDREF` [Schema2 §3.3.9].
    IdRef,
    /// `xs:ENTITY` [Schema2 §3.3.11].
    Entity,
    /// `xs:nonPositiveInteger` [Schema2 §3.3.14].
    NonPositiveInteger,
    /// `xs:negativeInteger` [Schema2 §3.3.15].
    NegativeInteger,
    /// `xs:long` [Schema2 §3.3.16].
    Long,
    /// `xs:int` [Schema2 §3.3.17].
    Int,
    /// `xs:short` [Schema2 §3.3.18].
    Short,
    /// `xs:byte` [Schema2 §3.3.19].
    Byte,
    /// `xs:nonNegativeInteger` [Schema2 §3.3.20].
    NonNegativeInteger,
    /// `xs:unsignedLong` [Schema2 §3.3.21].
    UnsignedLong,
    /// `xs:unsignedInt` [Schema2 §3.3.22].
    UnsignedInt,
    /// `xs:unsignedShort` [Schema2 §3.3.23].
    UnsignedShort,
    /// `xs:unsignedByte` [Schema2 §3.3.24].
    UnsignedByte,
    /// `xs:positiveInteger` [Schema2 §3.3.25].
    PositiveInteger,
}

/// An atomic value [XQueryDM §2.6.4]: its type and its canonical lexical
/// form, which casting it to `xs:string` gives [XQueryFO §17.1.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atomic {
    /// Its type.
    pub kind: AtomicType,
    /// Its canonical lexical form.
    pub lexical: Box<str>,
}

/// An item of a sequence [XQueryDM §2.5].
#[derive(Debug, Clone)]
pub enum Item {
    /// A node.
    Node(Node),
    /// An atomic value.
    Atomic(Atomic),
}

/// An XQuery sequence, the value of an XML value that is not null
/// [ISO/IEC 9075-14:2006 draft §4.2.3].
#[derive(Debug, Clone, Default)]
pub struct Sequence {
    items: Vec<Item>,
}

impl Sequence {
    /// The sequence of `items`, in order.
    #[must_use]
    pub fn new(items: Vec<Item>) -> Sequence {
        Sequence { items }
    }

    /// The sequence of the one document node of `tree`, what a parse
    /// makes [§10.16 GR10].
    #[must_use]
    pub fn document(tree: Tree) -> Sequence {
        Sequence { items: vec![Item::Node(Node::root(tree))] }
    }

    /// \[DESIGN\] How many octets it holds, about: each item's tree, counted
    /// by every item that shares it, so never too few, or atomic value.
    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.items
            .iter()
            .map(|item| match item {
                Item::Node(node) => node.tree().held_bytes(),
                Item::Atomic(atomic) => atomic.lexical.len(),
            })
            .fold(0, usize::saturating_add)
    }

    /// Its items, in order.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Its items, in order, taken.
    #[must_use]
    pub fn into_items(self) -> Vec<Item> {
        self.items
    }

    /// The document node it is alone, if it is one.
    fn only_document(&self) -> Option<&Node> {
        match self.items.as_slice() {
            [Item::Node(node)] => Some(node).filter(|n| n.kind() == Some(Kind::Document)),
            _ => None,
        }
    }

    /// Whether it is one document node [§4.2.3], what a site of an
    /// XML(CONTENT) type holds [§10.2 GR1a ii].
    #[must_use]
    pub fn is_content(&self) -> bool {
        self.only_document().is_some()
    }

    /// Whether it is one document node whose children are one element and
    /// comments and processing instructions [§4.2.3], what a site of an
    /// XML(DOCUMENT) type holds [§10.2 GR1a i].
    #[must_use]
    pub fn is_document(&self) -> bool {
        self.only_document().is_some_and(|d| {
            let tree = d.tree();
            let kinds: Vec<Option<Kind>> = tree.children(d.id()).map(|c| tree.kind(c)).collect();
            let elements = kinds.iter().filter(|&&k| k == Some(Kind::Element)).count();
            elements == 1 && kinds.iter().all(|&k| matches!(k, Some(Kind::Element | Kind::Comment | Kind::ProcessingInstruction)))
        })
    }
}

#[cfg(test)]
mod tests;
