//! Abstract syntax of the SQL/JSON query operators [ISO/IEC 9075-2:2023
//! §4.48.4]: JSON_VALUE [§6.28] and JSON_QUERY [§6.35], each a
//! `ValueExpression::Json`, and JSON_EXISTS [§8.23], a
//! `Predicate::JsonExists`; and the `<JSON API common syntax>` they share
//! [§10.14], with its `<JSON input expression>`s [§10.12].
//!
//! A clause not written is none here; the binder applies the Syntax Rules
//! that make one implicit.

use super::{TypeName, ValueExpression};
use crate::json::{ExistsBehavior, QueryBehavior, Quotes, Wrapper};

/// [§10.12, §10.13] `<JSON representation>`: JSON, of the encoding
/// written after ENCODING, if one is. \[DESIGN\] No `<implementation-defined
/// JSON representation option>` is defined, so FORMAT is followed by JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonFormat {
    /// The UTF8, UTF16 or UTF32 of ENCODING, if written.
    pub encoding: Option<&'static str>,
}

/// [§10.12] `<JSON input expression>` ::= `<value expression>` \[`<JSON
/// input clause>`\].
#[derive(Debug, Clone)]
pub struct JsonInput {
    /// The `<value expression>`.
    pub value: ValueExpression,
    /// The `<JSON input clause>`, FORMAT `<JSON representation>`, if
    /// written.
    pub format: Option<JsonFormat>,
}

/// [§10.14] `<JSON argument>` ::= `<JSON input expression>` AS
/// `<identifier>`.
#[derive(Debug, Clone)]
pub struct JsonArgument {
    /// The `<JSON input expression>`.
    pub input: JsonInput,
    /// The `<identifier>`'s value: a `<regular identifier>` in upper case,
    /// the body of a `<delimited identifier>`.
    pub name: String,
    /// Where the `<identifier>` is written.
    pub at: usize,
}

/// [§10.14] `<JSON API common syntax>` ::= `<JSON context item>` ,
/// `<JSON path specification>` \[`<JSON passing clause>`\]. \[SR2a\] AS
/// `<JSON table path name>` is JSON_TABLE's alone, refused as read.
#[derive(Debug, Clone)]
pub struct JsonCommon {
    /// The `<JSON context item>`, a `<JSON input expression>`.
    pub context: JsonInput,
    /// The value of the `<JSON path specification>`, a `<character
    /// string literal>`: the path expression's text.
    pub path: String,
    /// Where the `<JSON path specification>` is written.
    pub path_at: usize,
    /// The `<JSON argument>`s of the `<JSON passing clause>`, in order;
    /// none without one.
    pub passing: Vec<JsonArgument>,
}

/// [§6.28] A `<JSON value empty behavior>` or `<JSON value error
/// behavior>`.
#[derive(Debug, Clone)]
pub enum JsonValueBehavior {
    /// ERROR.
    Error,
    /// NULL.
    Null,
    /// DEFAULT `<value expression>`.
    Default(Box<ValueExpression>),
}

/// What follows the `<JSON API common syntax>` of an SQL/JSON query
/// operator, as written.
#[derive(Debug, Clone)]
pub enum JsonClauses {
    /// [§8.23] JSON_EXISTS: its `<JSON exists error behavior>`.
    Exists {
        /// The behavior ON ERROR.
        on_error: Option<ExistsBehavior>,
    },
    /// [§6.28] JSON_VALUE.
    Value {
        /// The `<data type>` of the `<JSON returning clause>`.
        returning: Option<TypeName>,
        /// The behavior ON EMPTY.
        on_empty: Option<JsonValueBehavior>,
        /// The behavior ON ERROR.
        on_error: Option<JsonValueBehavior>,
    },
    /// [§6.35] JSON_QUERY.
    Query {
        /// The `<JSON output clause>`: its `<data type>` and `<JSON
        /// representation>`, if FORMAT is written.
        returning: Option<(TypeName, Option<JsonFormat>)>,
        /// The `<JSON query wrapper behavior>`, WITH without CONDITIONAL
        /// or UNCONDITIONAL read as UNCONDITIONAL \[SR7\].
        wrapper: Option<Wrapper>,
        /// The `<JSON query quotes behavior>`.
        quotes: Option<Quotes>,
        /// The behavior ON EMPTY.
        on_empty: Option<QueryBehavior>,
        /// The behavior ON ERROR.
        on_error: Option<QueryBehavior>,
    },
}

/// An SQL/JSON query operator: JSON_EXISTS, JSON_VALUE or JSON_QUERY.
#[derive(Debug, Clone)]
pub struct JsonOperator {
    /// The `<JSON API common syntax>`.
    pub common: JsonCommon,
    /// The operator, and its clauses.
    pub clauses: JsonClauses,
}

impl JsonOperator {
    /// Calls `f` with each value expression of it: the context item's, the
    /// arguments' in order, and a DEFAULT's of ON EMPTY, then of ON ERROR.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        f(&self.common.context.value);
        self.common.passing.iter().for_each(|argument| f(&argument.input.value));
        if let JsonClauses::Value { on_empty, on_error, .. } = &self.clauses {
            for behavior in [on_empty, on_error].into_iter().flatten() {
                if let JsonValueBehavior::Default(value) = behavior {
                    f(value);
                }
            }
        }
    }
}
