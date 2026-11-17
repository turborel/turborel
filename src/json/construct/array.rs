//! JSON_ARRAY [ISO/IEC 9075-2:2023 §6.34], `<JSON array constructor>`:
//! a JSON array of its `<JSON input expression>`s, by enumeration, or of
//! the rows of a `<query expression>`, by query; and the elements of an
//! array being made, which JSON_ARRAYAGG gathers too [§10.11 GR5].

use super::{Input, MAX_ELEMENTS, OnNull, input_item, within};
use crate::error::{Result, SqlState};
use crate::json::serialize::string_type;
use crate::json::{IMPLICIT_RETURNING, Item, Keys, serialize_as};
use crate::value::{DataType, Value};

/// \[DESIGN\] The implementation-defined `<JSON key uniqueness
/// constraint>` an element FORMAT JSON is parsed under (IV156, IV178)
/// [§6.34 GR3c ii 1 B III, GR4b ii 1 B III, §10.11 GR5f ii 2 C]: WITHOUT
/// UNIQUE KEYS, as the SQL/JSON query operators parse (IV185).
const ELEMENT_KEYS: Keys = Keys::Any;

/// [§6.34, §10.11] What follows the elements of a JSON_ARRAY or a
/// JSON_ARRAYAGG: each clause explicit or implicit, as its Syntax Rules
/// make one (`ArrayOptions::IMPLICIT`).
#[derive(Debug, Clone, Copy)]
pub struct ArrayOptions<'a> {
    /// The `<JSON constructor null clause>`; without one, ABSENT ON NULL,
    /// `OnNull::Absent` [§6.34 SR7a, §10.11 SR5a].
    pub on_null: OnNull,
    /// The `<data type>` of the `<JSON output clause>`, FORMAT JSON, a
    /// character string type, which is the declared type [§6.34 SR7b,
    /// §10.11 SR5b]; without one, `IMPLICIT_RETURNING` (ID098, ID134)
    /// [§6.34 SR2b, §10.11 SR1b].
    pub returning: &'a DataType,
}

impl ArrayOptions<'static> {
    /// Every clause implicit: ABSENT ON NULL, RETURNING
    /// `IMPLICIT_RETURNING` FORMAT JSON.
    pub const IMPLICIT: ArrayOptions<'static> = ArrayOptions { on_null: OnNull::Absent, returning: &IMPLICIT_RETURNING };
}

/// The elements of a JSON array being made, one at a time, in order
/// [§6.34 GR3c ii, GR4b ii, §10.11 GR5f ii]. `count` counts those kept, of
/// which at most `MAX_ELEMENTS` are held, so what it holds is bounded.
#[derive(Debug, Clone)]
pub(super) struct Elements {
    on_null: OnNull,
    kept: Vec<Item>,
    count: usize,
}

impl Elements {
    /// No elements yet, of the null clause `on_null`.
    pub(super) fn new(on_null: OnNull) -> Elements {
        Elements { on_null, kept: Vec::new(), count: 0 }
    }

    /// [§6.34 GR3c ii 1 B, GR4b ii 1 B, §10.11 GR5f ii 2] JEi, the item
    /// `input` is; left out if it is the SQL/JSON null and the null clause
    /// ABSENT ON NULL [§6.34 GR3c ii 2 B].
    ///
    /// # Errors
    ///
    /// As `input_item`.
    pub(super) fn add(&mut self, input: &Input<'_>) -> Result<()> {
        let element = input_item(input, ELEMENT_KEYS)?;
        if self.on_null == OnNull::Absent && matches!(element, Item::Null) {
            return Ok(());
        }
        self.count = self.count.saturating_add(1);
        if self.count <= MAX_ELEMENTS {
            self.kept.push(element);
        }
        Ok(())
    }

    /// [§6.34 GR3c ii 2, GR4b ii 2, §10.11 GR5g] The elements kept, if they
    /// are not too many.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2203D if more than `MAX_ELEMENTS` are kept.
    pub(super) fn checked(&self) -> Result<&[Item]> {
        within(
            self.count,
            MAX_ELEMENTS,
            SqlState::TOO_MANY_JSON_ARRAY_ELEMENTS,
            "[ISO/IEC 9075-2:2023 §6.34 GR3c ii 2] the elements of a JSON array are too many",
        )?;
        Ok(&self.kept)
    }
}

/// [§6.34 GR3, GR5a] JSON_ARRAY by enumeration of the `<JSON input
/// expression>`s `elements`, in order, and the clauses `options`: the
/// JSON text of the array whose elements are the items they are, in
/// order \[GR3c ii\], with no elements of none \[GR3c i\], the SQL/JSON
/// nulls left out if `options.on_null` is ABSENT ON NULL, serialized to
/// `options.returning` [§9.43]. \[SR7d\] That a single element is no
/// `<scalar subquery>`, which makes it a JSON_ARRAY by query, is the SQL
/// layer's.
///
/// # Errors
///
/// SQLCODE -103 if `options.returning` is no string type [§9.43 SR2a];
/// -900 if it is a binary string type (RULES.md D18); of each element in
/// turn, as `input_item` (SQLSTATE 22032 of a FORMAT JSON one that is no
/// JSON text); then 2203D if the elements are more than `MAX_ELEMENTS`
/// \[GR3c ii 2\]; as `serialize_as`, 22001 if the text is longer than
/// `options.returning` [§9.43 GR2a iii].
pub fn json_array(elements: &[Input<'_>], options: &ArrayOptions<'_>) -> Result<Value> {
    string_type(options.returning)?;
    let mut made = Elements::new(options.on_null);
    for element in elements {
        made.add(element)?;
    }
    made.checked()?;
    serialize_as(&Item::Array(made.kept), options.returning)
}

/// [§6.34 GR4, GR5a] JSON_ARRAY by query of `column`, the values of the
/// one column \[SR7c\] of the rows of the table T its `<query expression>`
/// gives, in the order T has them, `format_json` whether its `<JSON input
/// clause>` FORMAT JSON is written, and the clauses `options`: as
/// `json_array` of an element of each value \[GR4b\], none of no rows
/// \[GR4b i\]. That the `<query expression>` is of degree 1 is the SQL
/// layer's \[SR7c\].
///
/// # Errors
///
/// As `json_array` \[GR4b ii 2\].
pub fn json_array_by_query(column: &[Value], format_json: bool, options: &ArrayOptions<'_>) -> Result<Value> {
    let inputs: Vec<Input<'_>> = column.iter().map(|value| Input { value, format_json }).collect();
    json_array(&inputs, options)
}

#[cfg(test)]
mod tests;
