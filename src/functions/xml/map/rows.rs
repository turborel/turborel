//! The set function that writes a table's rows as XML text [ISO/IEC
//! 9075-14:2006 draft §9.10 GR6]: of each row, the ROW of its XML visible
//! columns' values, an element of an element of each value. It holds the
//! text of the rows given it, at most `MAX_INPUT` octets, so a table of
//! any number of rows is read as its rows stream, and one whose text is
//! longer is refused, SQLCODE -900, before memory is (RULES.md D18, D86).

use std::fmt::Write;
use std::sync::Arc;

use super::super::super::{Accumulator, AggregateFunction, BinaryEncoding, xml_lexical};
use super::super::too_long;
use super::documents::{ROW, attribute, xml_name};
use super::{Choices, Nulls, Table, XMLMAP};
use crate::error::{Code, Result, SqlError};
use crate::value::lob::{Lob, LobKind};
use crate::value::{CharUnits, DataType, Value};
use crate::xml::{MAX_INPUT, Parameters, Version, XSI_NAMESPACE, serialize};

/// [§9.10 GR6c] How each row of a table is written: its element, the
/// attributes of that element, and the element's name of each of its
/// columns; the nulls and binary encoding of the mapping.
#[derive(Debug)]
pub struct RowShape {
    /// `row`, or XMLTN mapped as a forest.
    element: String,
    /// XSI and the target namespace, mapped as a forest.
    attributes: String,
    /// XMLCNj of each column.
    columns: Vec<String>,
    /// NULLS.
    nulls: Nulls,
    /// ENCODING.
    encoding: BinaryEncoding,
}

impl RowShape {
    /// [§9.10 GR6c] How each row of `table` is written by `choices`, as
    /// the mapping of a table itself if `alone`, else as one of a schema,
    /// TARGETNS then the zero-length string [§9.13 GR5b].
    #[must_use]
    pub fn of(table: &Table, choices: &Choices, alone: bool) -> RowShape {
        let element = if choices.forest { xml_name(&table.name) } else { ROW.to_owned() };
        let mut attributes = String::new();
        // Writing to a String cannot fail.
        if choices.forest && choices.nulls == Nulls::Nil {
            let _ = write!(attributes, r#" xmlns:xsi="{XSI_NAMESPACE}""#);
        }
        if choices.forest && alone && !choices.target.is_empty() {
            let _ = write!(attributes, r#" xmlns="{}""#, attribute(&choices.target));
        }
        let columns = table.columns.iter().map(|c| xml_name(&c.name)).collect();
        RowShape { element, attributes, columns, nulls: choices.nulls, encoding: choices.encoding }
    }

    /// [§9.10 GR6b v] XMLCj, of the value `v` of the column named `name`:
    /// of the null value nothing, NULLS absent, or an element nilled, NULLS
    /// nil; else the element of the value mapped by §9.8, characters
    /// mapped \[GR6b v 3 A\], an XML value serialized as CONTENT [§9.8
    /// GR9k].
    fn column(&self, out: &mut String, name: &str, v: &Value) -> Result<()> {
        // Writing to a String cannot fail.
        if v.is_null() {
            if self.nulls == Nulls::Nil {
                let _ = write!(out, r#"<{name} xsi:nil="true"/>"#);
            }
            return Ok(());
        }
        let text = if let Value::Xml(x) = v { serialize(&*x.sequence()?, &CONTENT)? } else { xml_lexical(v, self.encoding, true)? };
        let _ = write!(out, "<{name}>{text}</{name}>");
        Ok(())
    }
}

/// [§9.8 GR9k] XMLSERIALIZE(CONTENT ... AS CLOB): of XML 1.0, the
/// version XMLSERIALIZE writes of none [§6.8 SR2d], without a
/// declaration.
const CONTENT: Parameters = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };

/// The type of a table's rows' text: a CHARACTER LARGE OBJECT of the
/// longest text XMLPARSE reads.
#[must_use]
pub fn rows_type() -> DataType {
    DataType::Lob { kind: LobKind::Character, length: u64::try_from(MAX_INPUT).unwrap_or(u64::MAX), units: CharUnits::Characters }
}

/// \[EXTENSION\] The set function of a table's rows, written as `.0`
/// says (RULES.md D86).
#[derive(Debug)]
pub struct XmlMapRows(pub Arc<RowShape>);

impl AggregateFunction for XmlMapRows {
    fn name(&self) -> &str {
        XMLMAP
    }

    fn result_type(&self, _: Option<DataType>) -> Result<DataType> {
        Ok(rows_type())
    }

    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(Rows { shape: Arc::clone(&self.0), text: String::new() })
    }
}

/// The text of the rows a table's mapping has been given.
struct Rows {
    shape: Arc<RowShape>,
    text: String,
}

impl Accumulator for Rows {
    /// [§9.10 GR6] XMLRi of the row `v`, the ROW of its XML visible
    /// columns' values, after those before it; past `MAX_INPUT` octets,
    /// SQLCODE -900.
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(fields) = v else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] XMLMAP's rows were given {v}, not a row")));
        };
        let shape = &*self.shape;
        let text = &mut self.text;
        // Writing to a String cannot fail.
        let _ = write!(text, "<{}{}>", shape.element, shape.attributes);
        for (value, name) in fields.iter().zip(&shape.columns) {
            shape.column(text, name, value)?;
        }
        let _ = write!(text, "</{}>", shape.element);
        if text.len() > MAX_INPUT {
            return Err(too_long());
        }
        Ok(())
    }

    /// The text of the rows, of none the zero-length string.
    fn finish(&self) -> Result<Value> {
        Ok(Value::Lob(Lob::of_text(&self.text)))
    }
}
