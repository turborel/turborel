//! Sequence types, kind tests and node tests written back as text
//! [XQuery Appendix A.1 \[78\] to \[80\], \[117\] to \[139\]].

use std::fmt::{self, Display, Formatter};

use super::string_literal;
use crate::xquery::ast::{
    AttributeTest, DocumentTest, ElementTest, ItemType, KindTest, NameOrWildcard, NameTest, NodeTest, Occurrence, PiTest, SequenceType,
    SingleType,
};
use crate::xquery::keywords::{
    ATTRIBUTE, COMMENT, DOCUMENT_NODE, ELEMENT, EMPTY_SEQUENCE, ITEM, NODE, PROCESSING_INSTRUCTION, SCHEMA_ATTRIBUTE, SCHEMA_ELEMENT, TEXT,
};

impl Display for SequenceType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SequenceType::Empty => write!(f, "{EMPTY_SEQUENCE}()"),
            SequenceType::Item(item, occurrence) => {
                let indicator = match occurrence {
                    Occurrence::One => "",
                    Occurrence::Optional => "?",
                    Occurrence::Any => "*",
                    Occurrence::Many => "+",
                };
                write!(f, "{item}{indicator}")
            }
        }
    }
}

impl Display for ItemType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ItemType::Kind(test) => write!(f, "{test}"),
            ItemType::Item => write!(f, "{ITEM}()"),
            ItemType::Atomic(name) => write!(f, "{name}"),
        }
    }
}

impl Display for SingleType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.name, if self.optional { "?" } else { "" })
    }
}

impl Display for KindTest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            KindTest::Document(test) => {
                write!(f, "{DOCUMENT_NODE}(")?;
                match test {
                    Some(DocumentTest::Element(args)) => element_test(f, args.as_ref())?,
                    Some(DocumentTest::SchemaElement(name)) => write!(f, "{SCHEMA_ELEMENT}({name})")?,
                    None => {}
                }
                f.write_str(")")
            }
            KindTest::Element(args) => element_test(f, args.as_ref()),
            KindTest::Attribute(args) => attribute_test(f, args.as_ref()),
            KindTest::SchemaElement(name) => write!(f, "{SCHEMA_ELEMENT}({name})"),
            KindTest::SchemaAttribute(name) => write!(f, "{SCHEMA_ATTRIBUTE}({name})"),
            KindTest::Pi(test) => {
                write!(f, "{PROCESSING_INSTRUCTION}(")?;
                match test {
                    Some(PiTest::Name(name)) => f.write_str(name)?,
                    Some(PiTest::Literal(value)) => string_literal(f, value)?,
                    None => {}
                }
                f.write_str(")")
            }
            KindTest::Comment => write!(f, "{COMMENT}()"),
            KindTest::Text => write!(f, "{TEXT}()"),
            KindTest::AnyKind => write!(f, "{NODE}()"),
        }
    }
}

/// Writes an `ElementTest` [XQuery Appendix A.1 \[133\]] of `args`.
fn element_test(f: &mut Formatter<'_>, args: Option<&ElementTest>) -> fmt::Result {
    write!(f, "{ELEMENT}(")?;
    if let Some(args) = args {
        write!(f, "{}", args.name)?;
        if let Some(type_name) = &args.type_name {
            write!(f, ", {type_name}{}", if args.nillable { "?" } else { "" })?;
        }
    }
    f.write_str(")")
}

/// Writes an `AttributeTest` [XQuery Appendix A.1 \[129\]] of `args`.
fn attribute_test(f: &mut Formatter<'_>, args: Option<&AttributeTest>) -> fmt::Result {
    write!(f, "{ATTRIBUTE}(")?;
    if let Some(args) = args {
        write!(f, "{}", args.name)?;
        if let Some(type_name) = &args.type_name {
            write!(f, ", {type_name}")?;
        }
    }
    f.write_str(")")
}

impl Display for NameOrWildcard {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            NameOrWildcard::Name(name) => write!(f, "{name}"),
            NameOrWildcard::Wildcard => f.write_str("*"),
        }
    }
}

impl Display for NodeTest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            NodeTest::Kind(test) => write!(f, "{test}"),
            NodeTest::Name(NameTest::Name(name)) => write!(f, "{name}"),
            NodeTest::Name(NameTest::Any) => f.write_str("*"),
            NodeTest::Name(NameTest::Prefix(prefix)) => write!(f, "{prefix}:*"),
            NodeTest::Name(NameTest::Local(local)) => write!(f, "*:{local}"),
        }
    }
}
