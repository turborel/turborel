//! Sequence types [XQuery §2.5.3] [XQuery Appendix A.1 \[117\] to \[139\]]
//! and node tests [XQuery §3.2.1.2] [Appendix A.1 \[78\] to \[80\]], which
//! share the kind tests.

use super::QName;

/// `SequenceType` [XQuery Appendix A.1 \[119\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceType {
    /// `empty-sequence()`.
    Empty,
    /// `ItemType` \[121\] and its `OccurrenceIndicator` \[120\].
    Item(ItemType, Occurrence),
}

/// How many items a sequence type allows [XQuery Appendix A.1 \[120\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occurrence {
    /// No indicator: exactly one.
    One,
    /// `?`: zero or one.
    Optional,
    /// `*`: zero or more.
    Any,
    /// `+`: one or more.
    Many,
}

/// `ItemType` [XQuery Appendix A.1 \[121\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemType {
    /// `KindTest` \[123\].
    Kind(KindTest),
    /// `item()`.
    Item,
    /// `AtomicType` \[122\], a QName.
    Atomic(QName),
}

/// `SingleType` [XQuery Appendix A.1 \[117\]]: an atomic type, and whether
/// `?` allows the empty sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleType {
    /// The atomic type.
    pub name: QName,
    /// Whether `?` follows it.
    pub optional: bool,
}

/// `KindTest` [XQuery Appendix A.1 \[123\]] [XQuery §2.5.4].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KindTest {
    /// `DocumentTest` \[125\]: `document-node(` its element test, if any
    /// `)`.
    Document(Option<DocumentTest>),
    /// `ElementTest` \[133\]: `element(` its arguments, if any `)`.
    Element(Option<ElementTest>),
    /// `AttributeTest` \[129\].
    Attribute(Option<AttributeTest>),
    /// `SchemaElementTest` \[135\]: its `ElementDeclaration`.
    SchemaElement(QName),
    /// `SchemaAttributeTest` \[131\]: its `AttributeDeclaration`.
    SchemaAttribute(QName),
    /// `PITest` \[128\].
    Pi(Option<PiTest>),
    /// `CommentTest` \[127\].
    Comment,
    /// `TextTest` \[126\].
    Text,
    /// `AnyKindTest` \[124\], `node()`.
    AnyKind,
}

/// What a `DocumentTest` [XQuery Appendix A.1 \[125\]] tests its element
/// by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTest {
    /// An `ElementTest`: its arguments, if any.
    Element(Option<ElementTest>),
    /// A `SchemaElementTest`: its element declaration.
    SchemaElement(QName),
}

/// The arguments of an `ElementTest` [XQuery Appendix A.1 \[133\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementTest {
    /// `ElementNameOrWildcard` \[134\].
    pub name: NameOrWildcard,
    /// The `TypeName` \[139\], if given.
    pub type_name: Option<QName>,
    /// Whether `?` follows the type name: the element may be nilled.
    pub nillable: bool,
}

/// The arguments of an `AttributeTest` [XQuery Appendix A.1 \[129\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeTest {
    /// `AttribNameOrWildcard` \[130\].
    pub name: NameOrWildcard,
    /// The `TypeName`, if given.
    pub type_name: Option<QName>,
}

/// A name, or `*` for any [XQuery Appendix A.1 \[130\], \[134\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameOrWildcard {
    /// A QName.
    Name(QName),
    /// `*`.
    Wildcard,
}

/// The argument of a `PITest` [XQuery Appendix A.1 \[128\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiTest {
    /// An `NCName`.
    Name(String),
    /// A `StringLiteral`'s value.
    Literal(String),
}

/// `NodeTest` [XQuery Appendix A.1 \[78\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeTest {
    /// `KindTest` \[123\].
    Kind(KindTest),
    /// `NameTest` \[79\].
    Name(NameTest),
}

/// `NameTest` [XQuery Appendix A.1 \[79\]], with its `Wildcard` \[80\]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameTest {
    /// A QName.
    Name(QName),
    /// `*`.
    Any,
    /// `NCName:*`: any local name, of the prefix's namespace.
    Prefix(String),
    /// `*:NCName`: the local name, of any namespace.
    Local(String),
}
