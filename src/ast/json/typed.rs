//! Abstract syntax of the SQL/JSON functions of the JSON type [ISO/IEC
//! 9075-2:2023 §6.37 to §6.41]: JSON ( ), `<JSON parse>`, and
//! JSON_SCALAR, `<JSON scalar>`, which make a value of it; JSON_SERIALIZE,
//! `<JSON serialize>`, which makes its JSON text; and the `<JSON
//! simplified accessor>` [§6.36], a value expression primary followed by
//! ops of the path language. Each is a `JsonExpression::Typed`.

use super::{JsonInput, JsonOutput, ValueExpression};
use crate::json::Keys;

/// A function of the JSON type, or the simplified accessor.
#[derive(Debug, Clone)]
pub enum JsonTyped {
    /// [§6.40] `<JSON parse>` ::= JSON ( `<string value expression>`
    /// \[`<JSON input clause>`\] \[`<JSON key uniqueness constraint>`\] ).
    Parse {
        /// The string and its `<JSON input clause>`, if written.
        input: JsonInput,
        /// The `<JSON key uniqueness constraint>`, if written.
        keys: Option<Keys>,
    },
    /// [§6.41] `<JSON scalar>` ::= JSON_SCALAR ( `<value expression>` ).
    Scalar(ValueExpression),
    /// [§6.37] `<JSON serialize>` ::= JSON_SERIALIZE ( `<JSON value
    /// expression>` \[`<JSON output clause>`\] ).
    Serialize {
        /// The `<JSON value expression>`.
        value: ValueExpression,
        /// The `<JSON output clause>`, if written.
        returning: Option<JsonOutput>,
    },
    /// [§6.36] `<JSON simplified accessor>`: its `<value expression
    /// primary>`, and its op chain written as a path's accessors after
    /// `$`, its keys' case kept \[NOTE 198\]. Of a chain also read as an
    /// SQL one, `sql` is that reading, which the binder takes unless
    /// `primary` is of the JSON type.
    Accessor {
        /// The `<value expression primary>`, VEP.
        primary: ValueExpression,
        /// The op chain, JC, as the path language writes it.
        chain: String,
        /// The SQL reading of the same text, if it has one.
        sql: Option<ValueExpression>,
    },
}

impl JsonTyped {
    /// Calls `f` with each value expression of it.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            JsonTyped::Parse { input, .. } => f(&input.value),
            JsonTyped::Scalar(value) | JsonTyped::Serialize { value, .. } => f(value),
            JsonTyped::Accessor { primary, .. } => f(primary),
        }
    }
}
