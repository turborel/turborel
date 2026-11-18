//! JSON_OBJECTAGG [ISO/IEC 9075-2:2023 §10.11], a `<JSON aggregate
//! function>`: a JSON object of the rows of a group, as an accumulator
//! the aggregate is computed with. JSON_ARRAYAGG holds its rows in the
//! SQL layer, to sort them, and is JSON_ARRAY by query of them
//! (`json_array_by_query`).
//!
//! \[DESIGN\] Each is a state given the rows of T1, its argument source
//! with its `<filter clause>` applied [GR2, GR3], one at a time by `add`,
//! and its result by `finish`, which takes `&self`, so a state may be read
//! before more rows are given, as the executor's `Accumulator` is. What it
//! is given is the row's evaluated values, nulls too, which it needs
//! [GR4b ii 2 D I]: the SQL layer gives them as they are, not with the
//! null values a `<general set function>` eliminates.

use super::object::Members;
use super::{Input, ObjectOptions};
use crate::error::Result;
use crate::json::serialize::string_type;
use crate::json::{Item, serialize_as};
use crate::value::{DataType, Value};

/// [§10.11 GR4] The state of a JSON_OBJECTAGG: the members of the rows
/// given, and whether any row was.
#[derive(Debug, Clone)]
pub struct JsonObjectAgg {
    members: Members,
    rows: bool,
    returning: DataType,
}

impl JsonObjectAgg {
    /// No rows yet, of the clauses `options` [§10.11 SR4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `options.returning` is no string type [§9.43
    /// SR2a]; -900 if it is a binary string type (RULES.md D18).
    pub fn new(options: &ObjectOptions<'_>) -> Result<JsonObjectAgg> {
        string_type(options.returning)?;
        Ok(JsonObjectAgg { members: Members::new(options.on_null, options.keys), rows: false, returning: options.returning.clone() })
    }

    /// [§10.11 GR4b ii 2] The row Ri: `name`, VJNi, the value of the
    /// `<JSON name>`, and `value`, the `<JSON input expression>`, its
    /// value VJVEi; Mi is kept as JSON_OBJECT keeps a member.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22004 if `name` is the null value \[GR4b ii 2 B\]; SQLCODE
    /// -103 if it is no character string; SQLSTATE 22032 if `value` is
    /// FORMAT JSON and no JSON text, 22037 if an object of it has two
    /// members of one key under WITH UNIQUE KEYS \[GR4b ii 2 D III\]; as
    /// JSON_OBJECT's, of a value cast or read.
    pub fn add(&mut self, name: &Value, value: &Input<'_>) -> Result<()> {
        self.rows = true;
        self.members.add(name, value)
    }

    /// [§10.11 GR4b, GR6b] The result of the rows given: the null value if
    /// none was \[GR4b i\]; else the JSON text of the object of their
    /// members, those bound to the SQL/JSON null left out ABSENT ON NULL
    /// \[GR4b ii 4\], serialized to the type returned [§9.43].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22030 if two rows' names are one under WITH UNIQUE KEYS
    /// \[GR4b ii 3\]; 2203E if the members are more than `MAX_MEMBERS`
    /// \[GR4b ii 4\]; as `serialize_as`, 22001 if the text is longer than
    /// the type returned [§9.43 GR2a iii].
    pub fn finish(&self) -> Result<Value> {
        if !self.rows {
            return Ok(Value::Null);
        }
        let members = self.members.checked()?;
        serialize_as(&Item::Object(members.to_vec()), &self.returning)
    }
}

#[cfg(test)]
mod tests;
