//! Sequence types, single types and kind tests, resolved in the static
//! context [XQuery §2.5.3, §2.5.4, §3.12.3]: their names expanded, each
//! atomic type and type name one of the in-scope schema types.

use super::super::ast::{
    AttributeTest, DocumentTest, ElementTest, ItemType, KindTest, NameOrWildcard, PiTest, QName as Lexical, SequenceType, SingleType,
};
use super::super::error::{ErrorCode, error, not_yet};
use super::super::plan::CastTarget;
use super::super::types::{AttributeKind, ElementKind, ItemKind, NodeKind, SchemaType, SeqType, XDT_NAMESPACE};
use super::Analysis;
use super::scope::{Unprefixed, display};
use crate::error::Result;
use crate::xml::{Primitive, QName, XS_NAMESPACE};

impl Analysis<'_> {
    /// `t` resolved [XQuery §2.5.3].
    pub(super) fn sequence_type(&self, t: &SequenceType) -> Result<SeqType> {
        Ok(match t {
            SequenceType::Empty => SeqType::Empty,
            SequenceType::Item(item, occurrence) => SeqType::Of(self.item_type(item)?, *occurrence),
        })
    }

    /// `t` resolved [XQuery §2.5.3]: an atomic type one of the in-scope
    /// schema types \[err:XPST0051\].
    fn item_type(&self, t: &ItemType) -> Result<ItemKind> {
        Ok(match t {
            ItemType::Item => ItemKind::Item,
            ItemType::Kind(kind) => ItemKind::Node(self.kind_test(kind)?),
            ItemType::Atomic(name) => {
                let expanded = self.scope.name(name, Unprefixed::Element)?;
                let Some(found) = SchemaType::named(expanded.uri.as_deref(), &expanded.local).filter(SchemaType::is_atomic) else {
                    let why = format!("{name} is no atomic type of the in-scope schema types [XQuery §2.5.4.2]");
                    return Err(error(ErrorCode::XPST0051, &why));
                };
                ItemKind::Atomic(found)
            }
        })
    }

    /// The kind test `test` resolved [XQuery §2.5.4]: no schema element or
    /// attribute test, as no element or attribute is declared in scope
    /// \[err:XPST0008\].
    pub(super) fn kind_test(&self, test: &KindTest) -> Result<NodeKind> {
        Ok(match test {
            KindTest::AnyKind => NodeKind::Any,
            KindTest::Text => NodeKind::Text,
            KindTest::Comment => NodeKind::Comment,
            KindTest::Pi(target) => NodeKind::Pi(target.as_ref().map(|t| match t {
                PiTest::Name(name) | PiTest::Literal(name) => name.as_str().into(),
            })),
            KindTest::Element(test) => NodeKind::Element(self.element_test(test.as_ref())?),
            KindTest::Attribute(test) => NodeKind::Attribute(self.attribute_test(test.as_ref())?),
            KindTest::Document(None) => NodeKind::Document(None),
            KindTest::Document(Some(DocumentTest::Element(test))) => NodeKind::Document(Some(self.element_test(test.as_ref())?)),
            KindTest::SchemaElement(name) | KindTest::Document(Some(DocumentTest::SchemaElement(name))) => {
                return Err(undeclared(name, "element", "§2.5.4.4"));
            }
            KindTest::SchemaAttribute(name) => return Err(undeclared(name, "attribute", "§2.5.4.6")),
        })
    }

    /// An element test resolved [XQuery §2.5.4.3]: its name in the default
    /// element/type namespace if unprefixed, its type name one of the
    /// in-scope schema types.
    fn element_test(&self, test: Option<&ElementTest>) -> Result<ElementKind> {
        let Some(test) = test else { return Ok(ElementKind { name: None, annotation: None, nillable: true }) };
        let name = match &test.name {
            NameOrWildcard::Name(n) => Some(self.scope.name(n, Unprefixed::Element)?),
            NameOrWildcard::Wildcard => None,
        };
        // [XQuery §2.5.4.3] with no type name, any annotation and nilled
        // property match.
        let nillable = test.nillable || test.type_name.is_none();
        Ok(ElementKind { name, annotation: test.type_name.as_ref().map(|t| self.type_name(t)).transpose()?, nillable })
    }

    /// An attribute test resolved [XQuery §2.5.4.5]: its name in no
    /// namespace if unprefixed [XQuery §2.5.4].
    fn attribute_test(&self, test: Option<&AttributeTest>) -> Result<AttributeKind> {
        let Some(test) = test else { return Ok(AttributeKind { name: None, annotation: None }) };
        let name = match &test.name {
            NameOrWildcard::Name(n) => Some(self.scope.name(n, Unprefixed::None)?),
            NameOrWildcard::Wildcard => None,
        };
        Ok(AttributeKind { name, annotation: test.type_name.as_ref().map(|t| self.type_name(t)).transpose()? })
    }

    /// The schema type `name` names, one of the in-scope schema types
    /// [XQuery §2.5.4.3] \[err:XPST0008\].
    fn type_name(&self, name: &Lexical) -> Result<SchemaType> {
        let expanded = self.scope.name(name, Unprefixed::Element)?;
        SchemaType::named(expanded.uri.as_deref(), &expanded.local)
            .ok_or_else(|| error(ErrorCode::XPST0008, &format!("{name} is no type of the in-scope schema types [XQuery §2.5.4.3]")))
    }

    /// The target of a cast, `t` [XQuery §3.12.3]: an atomic type of the
    /// in-scope schema types, not `xs:NOTATION` nor `xdt:anyAtomicType`
    /// \[err:XPST0080\]; one of a value held here.
    pub(super) fn cast_target(&self, t: &SingleType) -> Result<CastTarget> {
        let expanded = self.scope.name(&t.name, Unprefixed::Element)?;
        Self::atomic_target(&expanded, t.optional)
    }

    /// As `cast_target`, of the expanded name `name`.
    pub(super) fn atomic_target(name: &QName, optional: bool) -> Result<CastTarget> {
        let refused = || error(ErrorCode::XPST0080, &format!("{} is no atomic type a value is cast to [XQuery §3.12.3]", display(name)));
        match SchemaType::named(name.uri.as_deref(), &name.local) {
            Some(SchemaType::Atomic(to)) => match to.primitive() {
                Primitive::Notation => Err(refused()),
                // [XQuery §3.12.3] a QName is cast of a string literal
                // alone, resolved in the static context, which turborel
                // does not do yet.
                Primitive::QName => Err(not_yet(&format!("a cast to {}", to.name()))),
                Primitive::Untyped
                | Primitive::String
                | Primitive::Boolean
                | Primitive::Decimal
                | Primitive::Integer
                | Primitive::Float
                | Primitive::Double
                | Primitive::Duration
                | Primitive::YearMonthDuration
                | Primitive::DayTimeDuration
                | Primitive::DateTime
                | Primitive::Time
                | Primitive::Date
                | Primitive::GYearMonth
                | Primitive::GYear
                | Primitive::GMonthDay
                | Primitive::GDay
                | Primitive::GMonth
                | Primitive::HexBinary
                | Primitive::Base64Binary
                | Primitive::AnyUri => Ok(CastTarget { to, optional }),
            },
            Some(
                SchemaType::AnyType | SchemaType::Untyped | SchemaType::AnySimpleType | SchemaType::AnyAtomicType | SchemaType::List(_),
            )
            | None => Err(refused()),
        }
    }

    /// Whether `name` of `arity` is a constructor function's [XQuery
    /// §3.12.5]: of one argument, named by an atomic type of the in-scope
    /// schema types.
    pub(super) fn constructor(name: &QName, arity: usize) -> bool {
        arity == 1
            && matches!(name.uri.as_deref(), Some(XS_NAMESPACE | XDT_NAMESPACE))
            && SchemaType::named(name.uri.as_deref(), &name.local).is_some_and(|t| t.is_atomic())
    }
}

/// The error of a schema element or attribute test of `name`, `kind`,
/// which no declaration in scope declares \[err:XPST0008\].
fn undeclared(name: &Lexical, kind: &str, clause: &str) -> crate::error::SqlError {
    error(ErrorCode::XPST0008, &format!("no {kind} {name} is declared in the in-scope {kind} declarations [XQuery {clause}]"))
}
