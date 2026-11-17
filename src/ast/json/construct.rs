//! Abstract syntax of the SQL/JSON constructor functions [ISO/IEC
//! 9075-2:2023 §6.34, §10.11] and of the `<JSON predicate>` [§8.22]:
//! JSON_OBJECT and JSON_ARRAY, each a `JsonExpression::Constructor`;
//! JSON_OBJECTAGG and JSON_ARRAYAGG, each a `<set function
//! specification>` of a `JsonAggregate`; and IS JSON, a
//! `Predicate::IsJson`.
//!
//! A clause not written is none here; the binder applies the Syntax Rules
//! that make one implicit.

use super::{JsonFormat, JsonInput};
use crate::ast::{Subquery, TypeName, ValueExpression};
use crate::json::{JsonType, Keys, OnNull};

/// [§10.13] A `<JSON output clause>`: its `<data type>`, and its `<JSON
/// representation>` if FORMAT is written.
pub type JsonOutput = (TypeName, Option<JsonFormat>);

/// [§6.34] A `<JSON name and value>`: `[ KEY ] name VALUE value`, or
/// `name : value` (Feature T814).
#[derive(Debug, Clone)]
pub struct JsonMember {
    /// The `<JSON name>`, a `<character value expression>`.
    pub name: ValueExpression,
    /// The `<JSON input expression>`.
    pub value: JsonInput,
    /// Whether it is written with a colon, `<JSON name and value 2>`.
    pub colon: bool,
}

/// [§6.34] A `<JSON value constructor>`.
#[derive(Debug, Clone)]
pub enum JsonConstructor {
    /// `<JSON object constructor>`: JSON_OBJECT ( \[members \[`<JSON
    /// constructor null clause>`\] \[`<JSON key uniqueness
    /// constraint>`\]\] \[`<JSON output clause>`\] ).
    Object {
        /// The `<JSON name and value>`s, in order.
        members: Vec<JsonMember>,
        /// The `<JSON constructor null clause>`.
        on_null: Option<OnNull>,
        /// The `<JSON key uniqueness constraint>`.
        keys: Option<Keys>,
        /// The `<JSON output clause>`.
        returning: Option<JsonOutput>,
    },
    /// `<JSON array constructor by enumeration>`: JSON_ARRAY (
    /// \[elements \[`<JSON constructor null clause>`\]\] \[`<JSON output
    /// clause>`\] ).
    Array {
        /// The `<JSON input expression>`s, in order.
        elements: Vec<JsonInput>,
        /// The `<JSON constructor null clause>`.
        on_null: Option<OnNull>,
        /// The `<JSON output clause>`.
        returning: Option<JsonOutput>,
    },
    /// `<JSON array constructor by query>`: JSON_ARRAY ( `<query
    /// expression>` \[`<JSON input clause>`\] \[`<JSON constructor null
    /// clause>`\] \[`<JSON output clause>`\] ), or of a lone `<scalar
    /// subquery>` \[SR7d\].
    ArrayQuery {
        /// The `<query expression>`.
        query: Box<Subquery>,
        /// The `<JSON input clause>`.
        format: Option<JsonFormat>,
        /// The `<JSON constructor null clause>`.
        on_null: Option<OnNull>,
        /// The `<JSON output clause>`.
        returning: Option<JsonOutput>,
    },
}

impl JsonConstructor {
    /// Calls `f` with each value expression of it: each name and value, or
    /// element, in order; none of a query, which is a subquery's.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            JsonConstructor::Object { members, .. } => members.iter().for_each(|m| {
                f(&m.name);
                f(&m.value.value);
            }),
            JsonConstructor::Array { elements, .. } => elements.iter().for_each(|e| f(&e.value)),
            JsonConstructor::ArrayQuery { .. } => {}
        }
    }
}

/// [§10.11] What a `<JSON aggregate function>` writes besides its
/// operands, which its `<set function specification>`'s argument holds:
/// JSON_ARRAYAGG's `<value expression>`, or JSON_OBJECTAGG's ROW of its
/// `<JSON name>` and `<value expression>`; JSON_ARRAYAGG's `<JSON array
/// aggregate order by clause>` is its `order_by`.
#[derive(Debug, Clone)]
pub struct JsonAggregate {
    /// The `<JSON input clause>` of its `<JSON input expression>`.
    pub format: Option<JsonFormat>,
    /// JSON_OBJECTAGG's `<JSON name and value 2>`, with a colon.
    pub colon: bool,
    /// The `<JSON constructor null clause>`.
    pub on_null: Option<OnNull>,
    /// JSON_OBJECTAGG's `<JSON key uniqueness constraint>`.
    pub keys: Option<Keys>,
    /// The `<JSON output clause>`.
    pub returning: Option<JsonOutput>,
}

/// [§8.22] A `<JSON predicate>`: `input IS [ NOT ] JSON [ type ] [
/// uniqueness ]`.
#[derive(Debug, Clone)]
pub struct IsJson {
    /// The `<JSON predicate input>` and its `<JSON input clause>`.
    pub input: JsonInput,
    /// IS NOT JSON was written.
    pub not: bool,
    /// The `<JSON predicate type constraint>`.
    pub kind: Option<JsonType>,
    /// The `<JSON key uniqueness constraint>`.
    pub keys: Option<Keys>,
}
