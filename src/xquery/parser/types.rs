//! Sequence types and kind tests [XQuery §2.5.3] [XQuery Appendix A.1
//! \[117\] to \[139\]].

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{
    AttributeTest, DocumentTest, ElementTest, ItemType, KindTest, NameOrWildcard, Occurrence, PiTest, QName, SequenceType, SingleType,
};
use crate::xquery::keywords::{
    ATTRIBUTE, COMMENT, DOCUMENT_NODE, ELEMENT, EMPTY_SEQUENCE, ITEM, NODE, PROCESSING_INSTRUCTION, SCHEMA_ATTRIBUTE, SCHEMA_ELEMENT, TEXT,
};
use crate::xquery::lexer::{Sym, Tok};

/// [XQuery Appendix A.1 \[123\]] The names of the kind tests, each
/// followed by `(`.
const KIND_TESTS: [&str; 9] =
    [DOCUMENT_NODE, ELEMENT, ATTRIBUTE, SCHEMA_ELEMENT, SCHEMA_ATTRIBUTE, PROCESSING_INSTRUCTION, COMMENT, TEXT, NODE];

impl Parser {
    /// Whether a `KindTest` [XQuery Appendix A.1 \[123\]] is next: its name
    /// and `(`.
    pub(super) fn kind_test_follows(&mut self) -> bool {
        KIND_TESTS.iter().any(|name| self.is_keyword(0, name)) && self.is_sym(1, Sym::LParen)
    }

    /// `SequenceType` [XQuery Appendix A.1 \[119\]]. An `?`, `*` or `+`
    /// right after an `ItemType` is its `OccurrenceIndicator` \[120\],
    /// never an operator [Appendix A.1.2, occurrence-indicators].
    pub(super) fn sequence_type(&mut self) -> Result<SequenceType> {
        if self.is_keyword(0, EMPTY_SEQUENCE) && self.is_sym(1, Sym::LParen) {
            self.ahead.pop_front();
            self.ahead.pop_front();
            self.expect_sym(Sym::RParen, "empty-sequence takes no argument [XQuery Appendix A.1 [119]]")?;
            return Ok(SequenceType::Empty);
        }
        let item = self.item_type()?;
        let occurrence = if self.accept_sym(Sym::Question) {
            Occurrence::Optional
        } else if self.accept_sym(Sym::Star) {
            Occurrence::Any
        } else if self.accept_sym(Sym::Plus) {
            Occurrence::Many
        } else {
            Occurrence::One
        };
        Ok(SequenceType::Item(item, occurrence))
    }

    /// `ItemType` [XQuery Appendix A.1 \[121\]].
    fn item_type(&mut self) -> Result<ItemType> {
        if self.kind_test_follows() {
            return self.kind_test().map(ItemType::Kind);
        }
        if self.is_keyword(0, ITEM) && self.is_sym(1, Sym::LParen) {
            self.ahead.pop_front();
            self.ahead.pop_front();
            self.expect_sym(Sym::RParen, "item takes no argument [XQuery Appendix A.1 [121]]")?;
            return Ok(ItemType::Item);
        }
        let rule = "a sequence type is empty-sequence(), item(), a kind test or an atomic type's QName [XQuery Appendix A.1 [119], [121]]";
        Ok(ItemType::Atomic(self.expect_name(rule)?))
    }

    /// `SingleType` [XQuery Appendix A.1 \[117\]].
    pub(super) fn single_type(&mut self) -> Result<SingleType> {
        let name = self.expect_name("a cast is to an atomic type's QName [XQuery Appendix A.1 [117]]")?;
        let optional = self.accept_sym(Sym::Question);
        Ok(SingleType { name, optional })
    }

    /// `KindTest` [XQuery Appendix A.1 \[123\]], which its name and `(`
    /// begin.
    pub(super) fn kind_test(&mut self) -> Result<KindTest> {
        let name = self.expect_name("a kind test is named [XQuery Appendix A.1 [123]]")?;
        self.ahead.pop_front();
        let test = if name.is(DOCUMENT_NODE) {
            KindTest::Document(self.document_test()?)
        } else if name.is(ELEMENT) {
            KindTest::Element(self.element_test()?)
        } else if name.is(ATTRIBUTE) {
            KindTest::Attribute(self.attribute_test()?)
        } else if name.is(SCHEMA_ELEMENT) {
            KindTest::SchemaElement(self.expect_name("schema-element names an element declaration [XQuery Appendix A.1 [135]]")?)
        } else if name.is(SCHEMA_ATTRIBUTE) {
            KindTest::SchemaAttribute(self.expect_name("schema-attribute names an attribute declaration [XQuery Appendix A.1 [131]]")?)
        } else if name.is(PROCESSING_INSTRUCTION) {
            KindTest::Pi(self.pi_test())
        } else if name.is(COMMENT) {
            KindTest::Comment
        } else if name.is(TEXT) {
            KindTest::Text
        } else {
            KindTest::AnyKind
        };
        self.expect_sym(Sym::RParen, "a kind test's arguments close by ')' [XQuery Appendix A.1 [123]]")?;
        Ok(test)
    }

    /// The argument of a `DocumentTest` [XQuery Appendix A.1 \[125\]], if
    /// it has one: an element or schema element test.
    fn document_test(&mut self) -> Result<Option<DocumentTest>> {
        let element = self.is_keyword(0, ELEMENT);
        if !(element || self.is_keyword(0, SCHEMA_ELEMENT)) || !self.is_sym(1, Sym::LParen) {
            return Ok(None);
        }
        self.ahead.pop_front();
        self.ahead.pop_front();
        let test = if element {
            DocumentTest::Element(self.element_test()?)
        } else {
            DocumentTest::SchemaElement(self.expect_name("schema-element names an element declaration [XQuery Appendix A.1 [135]]")?)
        };
        self.expect_sym(Sym::RParen, "a document test's element test closes by ')' [XQuery Appendix A.1 [125]]")?;
        Ok(Some(test))
    }

    /// The arguments of an `ElementTest` [XQuery Appendix A.1 \[133\]], if
    /// it has any.
    fn element_test(&mut self) -> Result<Option<ElementTest>> {
        let Some(name) = self.name_or_wildcard()? else {
            return Ok(None);
        };
        let (type_name, nillable) = if self.accept_sym(Sym::Comma) {
            let type_name = self.type_name("an element test's type is a QName [XQuery Appendix A.1 [133]]")?;
            (Some(type_name), self.accept_sym(Sym::Question))
        } else {
            (None, false)
        };
        Ok(Some(ElementTest { name, type_name, nillable }))
    }

    /// The arguments of an `AttributeTest` [XQuery Appendix A.1 \[129\]],
    /// if it has any.
    fn attribute_test(&mut self) -> Result<Option<AttributeTest>> {
        let Some(name) = self.name_or_wildcard()? else {
            return Ok(None);
        };
        let type_name = if self.accept_sym(Sym::Comma) {
            Some(self.type_name("an attribute test's type is a QName [XQuery Appendix A.1 [129]]")?)
        } else {
            None
        };
        Ok(Some(AttributeTest { name, type_name }))
    }

    /// `ElementNameOrWildcard` or `AttribNameOrWildcard` [XQuery Appendix
    /// A.1 \[130\], \[134\]], unless `)` is next.
    fn name_or_wildcard(&mut self) -> Result<Option<NameOrWildcard>> {
        if self.is_sym(0, Sym::RParen) {
            return Ok(None);
        }
        if self.accept_sym(Sym::Star) {
            return Ok(Some(NameOrWildcard::Wildcard));
        }
        let name = self.expect_name("a kind test names a QName or '*' [XQuery Appendix A.1 [130], [134]]")?;
        Ok(Some(NameOrWildcard::Name(name)))
    }

    /// `TypeName` [XQuery Appendix A.1 \[139\]].
    fn type_name(&mut self, rule: &str) -> Result<QName> {
        self.expect_name(rule)
    }

    /// The argument of a `PITest` [XQuery Appendix A.1 \[128\]], if it has
    /// one: an `NCName` or a string literal.
    fn pi_test(&mut self) -> Option<PiTest> {
        let test = match self.peek(0)? {
            Tok::Name(QName { prefix: None, local }) => PiTest::Name(local.clone()),
            Tok::String(value) => PiTest::Literal(value.clone()),
            Tok::Name(_)
            | Tok::PrefixWildcard(_)
            | Tok::LocalWildcard(_)
            | Tok::Integer(_)
            | Tok::Decimal(_)
            | Tok::Double(_)
            | Tok::Pragma(_)
            | Tok::Sym(_)
            | Tok::End => return None,
        };
        self.ahead.pop_front();
        Some(test)
    }
}
