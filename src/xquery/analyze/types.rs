//! Sequence types, single types and kind tests, resolved in the static
//! context [XQuery §2.5.3, §2.5.4, §3.12.3]: their names expanded, each
//! atomic type and type name one of the in-scope schema types.

use super::super::ast::{
    AttributeTest, DocumentTest, ElementTest, Expr, ItemType, KindTest, Literal, NameOrWildcard, PiTest, QName as Lexical, SequenceType,
    SingleType,
};
use super::super::error::{ErrorCode, error};
use super::super::plan::{CastTarget, Op};
use super::super::types::{AttributeKind, ElementKind, ItemKind, NodeKind, SchemaType, SeqType, XDT_NAMESPACE};
use super::Analysis;
use super::scope::{Unprefixed, display};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Primitive, QName, Version, XS_NAMESPACE, is_space, qname_parts};

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

    /// The kind test `test` resolved [XQuery §2.5.4]: a schema element or
    /// attribute test of a declaration of an imported schema
    /// (`analyze/import.rs`) \[err:XPST0008\].
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
            KindTest::SchemaElement(name) => NodeKind::Element(self.schema_element(name)?),
            KindTest::Document(Some(DocumentTest::SchemaElement(name))) => NodeKind::Document(Some(self.schema_element(name)?)),
            KindTest::SchemaAttribute(name) => NodeKind::Attribute(self.schema_attribute(name)?),
        })
    }

    /// An element test resolved [XQuery §2.5.4.3]: its name in the default
    /// element/type namespace if unprefixed, its type name one of the
    /// in-scope schema types.
    fn element_test(&self, test: Option<&ElementTest>) -> Result<ElementKind> {
        let Some(test) = test else {
            return Ok(ElementKind { name: None, substitutes: Vec::new(), annotation: None, nillable: true, imports: self.imports() });
        };
        let name = match &test.name {
            NameOrWildcard::Name(n) => Some(self.scope.name(n, Unprefixed::Element)?),
            NameOrWildcard::Wildcard => None,
        };
        // [XQuery §2.5.4.3] with no type name, any annotation and nilled
        // property match.
        let nillable = test.nillable || test.type_name.is_none();
        let annotation = test.type_name.as_ref().map(|t| self.type_name(t)).transpose()?;
        Ok(ElementKind { name, substitutes: Vec::new(), annotation, nillable, imports: self.imports() })
    }

    /// An attribute test resolved [XQuery §2.5.4.5]: its name in no
    /// namespace if unprefixed [XQuery §2.5.4].
    fn attribute_test(&self, test: Option<&AttributeTest>) -> Result<AttributeKind> {
        let Some(test) = test else { return Ok(AttributeKind { name: None, annotation: None, imports: self.imports() }) };
        let name = match &test.name {
            NameOrWildcard::Name(n) => Some(self.scope.name(n, Unprefixed::None)?),
            NameOrWildcard::Wildcard => None,
        };
        let annotation = test.type_name.as_ref().map(|t| self.type_name(t)).transpose()?;
        Ok(AttributeKind { name, annotation, imports: self.imports() })
    }

    /// The schema type `name` names, one of the in-scope schema types
    /// [XQuery §2.5.4.3]: a built-in one, or one of an imported schema
    /// \[err:XPST0008\].
    fn type_name(&self, name: &Lexical) -> Result<SchemaType> {
        let expanded = self.scope.name(name, Unprefixed::Element)?;
        SchemaType::named(expanded.uri.as_deref(), &expanded.local)
            .or_else(|| self.imported_type(&expanded))
            .ok_or_else(|| error(ErrorCode::XPST0008, &format!("{name} is no type of the in-scope schema types [XQuery §2.5.4.3]")))
    }

    /// A cast or, if `castable`, a castable expression of `operand` to
    /// `t` [XQuery §3.12.3, §3.12.4]: its target an atomic type of the
    /// in-scope schema types, not `xs:NOTATION` nor `xdt:anyAtomicType`
    /// \[err:XPST0080\]; to `xs:QName`, of a string literal alone.
    pub(super) fn cast_expr(&mut self, operand: &Expr, t: &SingleType, castable: bool) -> Result<Op> {
        let expanded = self.scope.name(&t.name, Unprefixed::Element)?;
        let target = Self::atomic_target(&expanded, t.optional)?;
        if target.to == AtomicType::QName {
            return self.qname_cast(operand, castable);
        }
        let operand = Box::new(self.expr(operand)?);
        Ok(if castable { Op::Castable(operand, target) } else { Op::Cast(operand, target) })
    }

    /// A cast, or if `castable` a castable expression, of `operand` to
    /// `xs:QName` [XQuery §3.12.3, §3.12.4, §3.12.5] [XQueryFO §5.1,
    /// §17.1.1]: of a string literal alone \[err:XPST0083\], evaluated as
    /// the query is analysed. Castable is whether the cast succeeds.
    ///
    /// # Errors
    ///
    /// err:XPST0083 of an operand that is no string literal; of a cast,
    /// err:FORG0001 of a literal that is no QName, err:FONS0004 of a
    /// prefix the statically known namespaces do not bind.
    pub(super) fn qname_cast(&self, operand: &Expr, castable: bool) -> Result<Op> {
        let Expr::Literal(Literal::String(text)) = operand else {
            let why = "a value cast to xs:QName is a string literal [XQuery §3.12.3, §3.12.5]";
            return Err(error(ErrorCode::XPST0083, why));
        };
        let resolved = self.qname_of(text);
        if castable {
            return Ok(Op::Atomic(Atomic::new(AtomicType::Boolean, if resolved.is_ok() { "true" } else { "false" })));
        }
        resolved.map(Op::Atomic)
    }

    /// The `xs:QName` the literal `text` is [XQueryFO §5.1, §17.1.1]: its
    /// whitespace collapsed, its prefix resolved by the statically known
    /// namespaces, none by the default element/type namespace, the prefix
    /// kept.
    fn qname_of(&self, text: &str) -> Result<Atomic> {
        let written = text.trim_matches(is_space);
        let Some((prefix, local)) = qname_parts(Version::V11, written) else {
            return Err(error(ErrorCode::FORG0001, &format!("'{written}' is no QName, so it is no xs:QName [XQueryFO §17.1.1]")));
        };
        let lexical = Lexical { prefix: prefix.map(str::to_owned), local: local.to_owned() };
        let name = self.scope.expand(&lexical, Unprefixed::Element, ErrorCode::FONS0004)?;
        Ok(Atomic::qname(name.uri.as_deref(), written))
    }

    /// As `cast_target`, of the expanded name `name`.
    pub(super) fn atomic_target(name: &QName, optional: bool) -> Result<CastTarget> {
        let refused = || error(ErrorCode::XPST0080, &format!("{} is no atomic type a value is cast to [XQuery §3.12.3]", display(name)));
        match SchemaType::named(name.uri.as_deref(), &name.local) {
            Some(SchemaType::Atomic(to)) => match to.primitive() {
                Primitive::Notation => Err(refused()),
                Primitive::QName
                | Primitive::Untyped
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
                SchemaType::AnyType
                | SchemaType::Untyped
                | SchemaType::AnySimpleType
                | SchemaType::AnyAtomicType
                | SchemaType::List(_)
                | SchemaType::Defined(_),
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
pub(super) fn undeclared(name: &Lexical, kind: &str, clause: &str) -> crate::error::SqlError {
    error(ErrorCode::XPST0008, &format!("no {kind} {name} is declared in the in-scope {kind} declarations [XQuery {clause}]"))
}
