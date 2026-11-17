//! Abstract syntax of the SQL/XML operators [ISO/IEC 9075-14:2006 draft
//! §6], each a `ValueExpression::Xml`: XMLPARSE [§6.16] and XMLSERIALIZE
//! [§6.8].

use super::ValueExpression;
use crate::value::DataType;
use crate::xml::{Syntax, Whitespace};

/// [ISO/IEC 9075-14:2006 draft §6.5, §6.18] `<XML passing mechanism>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Passing {
    /// BY REF: the value's nodes themselves.
    ByRef,
    /// BY VALUE: a copy of them [§10.18].
    ByValue,
}

/// An SQL/XML operator.
#[derive(Debug, Clone)]
pub enum XmlOperator {
    /// [§6.16] `<XML parse>` ::= XMLPARSE ( `<document or content>`
    /// `<string value expression>` `<XML whitespace option>` ).
    Parse {
        /// DOCUMENT or CONTENT.
        syntax: Syntax,
        /// The string parsed.
        operand: ValueExpression,
        /// PRESERVE or STRIP WHITESPACE.
        whitespace: Whitespace,
    },
    /// [§6.8] `<XML character string serialization>` and `<XML binary
    /// string serialization>` ::= XMLSERIALIZE ( \[`<document or
    /// content>`\] `<XML value expression>` AS `<data type>` \[ENCODING
    /// `<XML encoding name>`\] \[`<XML serialize version>`\] \[`<XML
    /// declaration option>`\] ).
    Serialize {
        /// DOCUMENT or CONTENT, CONTENT if neither is written \[SR2a,
        /// SR4a\].
        syntax: Syntax,
        /// The XML value serialized.
        operand: ValueExpression,
        /// The string type it is serialized as.
        target: DataType,
        /// The `<XML encoding name>`, if written, of a binary string.
        encoding: Option<String>,
        /// The `<XML serialize version>`'s literal, if written.
        version: Option<String>,
        /// INCLUDING (true) or EXCLUDING (false) XMLDECLARATION, if
        /// written.
        declaration: Option<bool>,
    },
}

impl XmlOperator {
    /// Calls `f` on each value expression it holds.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            XmlOperator::Parse { operand, .. } | XmlOperator::Serialize { operand, .. } => f(operand),
        }
    }
}
