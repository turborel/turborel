//! The type annotations a validation gives element and attribute nodes
//! [XQueryDM §3.3.1], and the typed value of a node [XQueryDM §5.15]:
//! read from its string value as its annotation says, as the data model
//! lets an implementation that keeps the string value alone do it
//! [XQueryDM §3.3.1.3].

use std::sync::Arc;

use super::{Kind, NodeId, QName, Tree};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::xml::{Atomic, AtomicType, Primitive, XS_NAMESPACE, from_lexical};

/// How the typed value of a node of a type is determined [XQueryDM
/// §3.3.1.2, §6.2.4]: by the variety of its simple type, or its content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    Atomic(ValueType),
    /// A list of an atomic type: a value of it for each item.
    List(ValueType),
    /// A list whose items are of several atomic types, as a list of a
    /// union's are: a value of each item's own type, in order, the member
    /// type that validated it [XQueryDM §3.3.1.3] [Schema2 §2.5.1.3].
    Items(Arc<[ValueType]>),
}

/// The atomic type of a value of a typed value [XQueryDM §3.3.1.2]: the
/// built-in type it is or restricts, and the name of the user-defined
/// type, if it is one, whose instance the value is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValueType {
    /// The built-in type.
    pub kind: AtomicType,
    /// The user-defined type's name, none of a built-in type.
    pub name: Option<Arc<QName>>,
}

impl From<AtomicType> for ValueType {
    /// The built-in type `kind`.
    fn from(kind: AtomicType) -> ValueType {
        ValueType { kind, name: None }
    }
}

impl ValueType {
    /// The type of `value`.
    #[must_use]
    pub fn of(value: &Atomic) -> ValueType {
        ValueType { kind: value.kind, name: value.annotation.clone() }
    }
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
    /// string value read as that type's, a list's item by item, a
    /// qualified name's with its namespace URI (`Tree::qualified`); of
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
        match &annotation.typing {
            Typing::AnySimple | Typing::Mixed => Ok(untyped(text)),
            Typing::Empty => Ok(Vec::new()),
            Typing::ElementOnly => {
                Err(xquery_error("an element of element-only content has no typed value", "[XQueryFO §2.4] err:FOTY0012"))
            }
            Typing::Atomic(t) => Ok(vec![self.qualified(id, read(&text, t)?)]),
            Typing::List(t) => text.split_ascii_whitespace().map(|item| Ok(self.qualified(id, read(item, t)?))).collect(),
            Typing::Items(kinds) => {
                let items: Vec<&str> = text.split_ascii_whitespace().collect();
                if items.len() != kinds.len() {
                    let why = format!("'{text}' is not {} items, as its type annotation says", kinds.len());
                    return Err(xquery_error(&why, "[XQueryFO §17.1.1] err:FORG0001"));
                }
                items.into_iter().zip(kinds.iter()).map(|(item, t)| Ok(self.qualified(id, read(item, t)?))).collect()
            }
        }
    }

    /// `value`, read from the string value of node `id`; of a qualified
    /// name, an `xs:QName`, an `xs:NOTATION` or a type derived from one,
    /// with the namespace URI its prefix is bound to, or the default
    /// namespace of none, among the namespaces in scope of the element
    /// `id` is or is an attribute of [Schema2 §3.2.18 NOTE] [XQueryDM
    /// §3.3.1.3, §3.3.3]. A parentless attribute has no namespaces in
    /// scope, so its qualified names have no namespace URI.
    fn qualified(&self, id: NodeId, mut value: Atomic) -> Atomic {
        if !matches!(value.kind.primitive(), Primitive::QName | Primitive::Notation) {
            return value;
        }
        let element = if self.kind(id) == Some(Kind::Attribute) { self.parent(id) } else { Some(id) };
        let prefix = value.lexical.split_once(':').map(|(prefix, _)| prefix);
        value.namespace = element.and_then(|e| self.in_scope(e).into_iter().find(|(p, _)| *p == prefix).map(|(_, uri)| uri.into()));
        value
    }
}

/// The error of XQuery code `code`, `why`: SQLSTATE 10000, XQuery error.
fn xquery_error(why: &str, code: &str) -> SqlError {
    SqlError::new(Code::Data, format!("{code} XQuery error: {why}")).with_state(SqlState::XQUERY_ERROR)
}

/// `text` read as a value of `t`: of its built-in type, annotated with its
/// user-defined type's name if it has one [XQueryDM §3.3.1.2].
fn read(text: &str, t: &ValueType) -> Result<Atomic> {
    let value = from_lexical(text, t.kind)?;
    let value =
        value.ok_or_else(|| xquery_error(&format!("'{text}' is no value of {}", t.kind.name()), "[XQueryFO §17.1.1] err:FORG0001"))?;
    Ok(value.annotated(t.name.clone()))
}
