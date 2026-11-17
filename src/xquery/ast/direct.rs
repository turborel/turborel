//! Constructors [XQuery §3.7] [XQuery Appendix A.1 \[94\] to \[116\]]: the
//! direct constructors, written as XML is, and the computed constructors,
//! written as expressions are.

use super::QName;
use super::expr::Expr;

/// `Constructor` [XQuery Appendix A.1 \[94\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constructor {
    /// `DirElemConstructor` \[96\] [XQuery §3.7.1].
    Element(DirElement),
    /// `DirCommentConstructor` \[103\]: its `DirCommentContents` [XQuery
    /// §3.7.2].
    Comment(String),
    /// `DirPIConstructor` \[105\].
    Pi(DirPi),
    /// `CompDocConstructor` \[110\] [XQuery §3.7.3.3].
    Document(Expr),
    /// `CompElemConstructor` \[111\] [XQuery §3.7.3.1]: its name, and its
    /// `ContentExpr` \[112\] if it has one.
    ComputedElement(ComputedName, Option<Expr>),
    /// `CompAttrConstructor` \[113\] [XQuery §3.7.3.2].
    ComputedAttribute(ComputedName, Option<Expr>),
    /// `CompTextConstructor` \[114\] [XQuery §3.7.3.4].
    Text(Expr),
    /// `CompCommentConstructor` \[115\] [XQuery §3.7.3.6].
    ComputedComment(Expr),
    /// `CompPIConstructor` \[116\] [XQuery §3.7.3.5].
    ComputedPi(PiName, Option<Expr>),
}

/// The name of a computed element or attribute [XQuery Appendix A.1
/// \[111\], \[113\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputedName {
    /// A QName.
    Name(QName),
    /// An expression in braces, its name expression.
    Expr(Expr),
}

/// The target of a computed processing instruction [XQuery Appendix A.1
/// \[116\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiName {
    /// An `NCName`.
    Name(String),
    /// An expression in braces.
    Expr(Expr),
}

/// `DirElemConstructor` [XQuery Appendix A.1 \[96\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirElement {
    /// Its name, which its end tag repeats [XQuery §3.7.1].
    pub name: QName,
    /// `DirAttributeList` \[97\]'s attributes, namespace declaration
    /// attributes among them [XQuery §3.7.1.2].
    pub attributes: Vec<DirAttribute>,
    /// `DirElemContent` \[101\]: nothing for `/>`.
    pub content: Vec<DirContent>,
}

/// An attribute of a direct element constructor [XQuery Appendix A.1
/// \[97\]] [XQuery §3.7.1.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirAttribute {
    /// Its name.
    pub name: QName,
    /// `DirAttributeValue` \[98\], its parts in order.
    pub value: Vec<AttributeValuePart>,
}

/// A part of a `DirAttributeValue` [XQuery Appendix A.1 \[98\] to \[100\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeValuePart {
    /// Characters as written, with `EscapeQuot` \[146\], `EscapeApos`
    /// \[147\], `{{` and `}}` each the one character it stands for.
    Chars(String),
    /// A `PredefinedEntityRef` \[145\] or `CharRef` \[153\]: the character
    /// it stands for, which attribute value normalization keeps [XML
    /// §3.3.3].
    Reference(char),
    /// `EnclosedExpr` \[29\].
    Enclosed(Expr),
}

/// `DirElemContent` [XQuery Appendix A.1 \[101\]] and its `CommonContent`
/// \[102\].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirContent {
    /// `ElementContentChar` \[148\]s, with `{{` and `}}` each the one
    /// character it stands for: as many as are written together.
    Chars(String),
    /// A `PredefinedEntityRef` or `CharRef`: the character it stands for,
    /// which is no whitespace [XQuery §3.7.1.4].
    Reference(char),
    /// `CDataSection` \[107\]'s contents.
    CData(String),
    /// `EnclosedExpr`.
    Enclosed(Expr),
    /// A nested `DirElemConstructor`.
    Element(DirElement),
    /// A nested `DirCommentConstructor`.
    Comment(String),
    /// A nested `DirPIConstructor`.
    Pi(DirPi),
}

/// `DirPIConstructor` [XQuery Appendix A.1 \[105\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirPi {
    /// `PITarget` \[152\].
    pub target: String,
    /// `DirPIContents` \[106\], the whitespace after the target not
    /// included.
    pub contents: String,
}

/// Whether `content[index]` is boundary whitespace [XQuery §3.7.1.4]:
/// whitespace characters alone, delimited at each end by the start or end
/// of the content, a direct constructor or an enclosed expression. The
/// boundary-space policy decides whether it is kept [XQuery §4.3]; a
/// character a reference or a CDATA section stands for is no whitespace,
/// and delimits nothing.
#[must_use]
pub fn boundary_whitespace(content: &[DirContent], index: usize) -> bool {
    let white = if let Some(DirContent::Chars(text)) = content.get(index) { text.chars().all(crate::xml::is_space) } else { false };
    let before = index.checked_sub(1).is_none_or(|at| delimits(content.get(at)));
    white && before && delimits(content.get(index.saturating_add(1)))
}

/// Whether `part`, beside whitespace, makes it boundary whitespace: none,
/// past the content's start or end, or a direct constructor or an
/// enclosed expression [XQuery §3.7.1.4].
fn delimits(part: Option<&DirContent>) -> bool {
    match part {
        None | Some(DirContent::Enclosed(_) | DirContent::Element(_) | DirContent::Comment(_) | DirContent::Pi(_)) => true,
        Some(DirContent::Chars(_) | DirContent::Reference(_) | DirContent::CData(_)) => false,
    }
}
