//! The type annotations a validation gives element and attribute nodes
//! [XQueryDM §3.3.1], and the typed value of a node [XQueryDM §5.15]:
//! read from its string value as its annotation says, as the data model
//! lets an implementation that keeps the string value alone do it
//! [XQueryDM §3.3.1.3].

use super::{Kind, NodeId, QName, Tree};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::xml::{Atomic, AtomicType, XS_NAMESPACE, from_lexical};

/// How the typed value of a node of a type is determined [XQueryDM
/// §3.3.1.2, §6.2.4]: by the variety of its simple type, or its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Typing {
    /// `xs:anySimpleType`: its schema normalized value as
    /// `xdt:untypedAtomic`.
    AnySimple,
    /// Mixed content, `xs:anyType`'s too: its string value as
    /// `xdt:untypedAtomic`.
    Mixed,
    /// Empty content: the empty sequence.
    Empty,
    /// Element-only content: no typed value [XQueryDM §6.2.4].
    ElementOnly,
    /// An atomic type: one value of it.
    Atomic(AtomicType),
    /// A list of an atomic type: a value of it for each item.
    List(AtomicType),
}

/// A type annotation [XQueryDM §3.3.1.1]: the name of the type, an
/// anonymous type's one the validation made, and how a typed value of it
/// is determined.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Annotation {
    /// The type's name.
    pub name: QName,
    /// How its typed values are determined.
    pub typing: Typing,
}

impl Annotation {
    /// `xs:anyType`, of mixed content [XQueryDM §3.3.1.1]: the annotation
    /// of an element a constructor makes in construction mode preserve
    /// [XQuery §3.7.1.3], or a validation does not assess, or assesses
    /// laxly [XQuery §3.13].
    #[must_use]
    pub fn any_type() -> Annotation {
        Annotation { name: QName { prefix: None, uri: Some(XS_NAMESPACE.into()), local: "anyType".into() }, typing: Typing::Mixed }
    }
}

impl NodeId {
    /// The `n`th attribute of element `self` (counting from 0), whose
    /// attributes follow it in its tree.
    #[must_use]
    pub fn nth_attribute(self, n: usize) -> Option<NodeId> {
        NodeId::at(self.index().checked_add(n)?.checked_add(1)?)
    }
}

impl Tree {
    /// The typed value of node `id` [XQueryDM §5.15, §3.3.1.2, §6.2.4,
    /// §6.3.4]: none of a nilled element; of a node no schema typed, or of
    /// mixed content or `xs:anySimpleType`, its string value as an
    /// `xdt:untypedAtomic`; of empty content, none; of a simple type, its
    /// string value read as that type's, a list's item by item; of
    /// another node, its string value as XQuery's data model gives it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000 of an element of element-only content, which has no
    /// typed value [XQueryFO §2.4 err:FOTY0012], or of a value its type no
    /// longer reads.
    pub fn typed_value(&self, id: NodeId) -> Result<Vec<Atomic>> {
        if self.nilled(id) {
            return Ok(Vec::new());
        }
        let text = self.string_value(id);
        let untyped = |t: String| vec![Atomic::new(AtomicType::UntypedAtomic, t)];
        let Some(annotation) = self.annotation(id).filter(|_| matches!(self.kind(id), Some(Kind::Element | Kind::Attribute))) else {
            return Ok(untyped(text));
        };
        match annotation.typing {
            Typing::AnySimple | Typing::Mixed => Ok(untyped(text)),
            Typing::Empty => Ok(Vec::new()),
            Typing::ElementOnly => {
                Err(xquery_error("an element of element-only content has no typed value", "[XQueryFO §2.4] err:FOTY0012"))
            }
            Typing::Atomic(t) => Ok(vec![read(&text, t)?]),
            Typing::List(t) => text.split_ascii_whitespace().map(|item| read(item, t)).collect(),
        }
    }
}

/// The error of XQuery code `code`, `why`: SQLSTATE 10000, XQuery error.
fn xquery_error(why: &str, code: &str) -> SqlError {
    SqlError::new(Code::Data, format!("{code} XQuery error: {why}")).with_state(SqlState::XQUERY_ERROR)
}

/// `text` read as a value of `t`.
fn read(text: &str, t: AtomicType) -> Result<Atomic> {
    from_lexical(text, t)?.ok_or_else(|| xquery_error(&format!("'{text}' is no value of {}", t.name()), "[XQueryFO §17.1.1] err:FORG0001"))
}
