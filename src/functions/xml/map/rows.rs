//! A mapping's rows written as XML text [ISO/IEC 9075-14:2006 draft §9.10
//! GR6]: a function of each row's XML visible columns' values, the row's
//! element of an element of each value (`XmlMapRow`); and the set function
//! of the rows of all the tables a mapping maps, one table's after
//! another's, each the index of its table and its text, which writes the
//! mapping's data as they come (`XmlMapRows`, `map/data.rs`). A row's text
//! and the data are each at most `MAX_INPUT` octets, so a mapping of any
//! number of rows and tables is read as its rows stream, and one longer is
//! refused, SQLCODE -900, before memory is (RULES.md D18, D106).

use std::fmt::Write;
use std::sync::Arc;

use super::super::super::{Accumulator, AggregateFunction, Mapping, ScalarFunction, xml_mapped};
use super::super::{Input, input, too_long};
use super::data::{DataShape, DataWriter};
use super::documents::{ROW, attribute, xml_name};
use super::{Choices, Nulls, Table, XMLMAP};
use crate::error::{Code, Result, SqlError};
use crate::value::lob::{Lob, LobKind};
use crate::value::{CharUnits, DataType, Value};
use crate::xml::{MAX_INPUT, XSI_NAMESPACE};

/// [§9.10 GR6c] How each row of a table is written: its element, the
/// attributes of that element, and the element's name and the data type
/// of each of its columns; how its values are mapped [§9.8].
#[derive(Debug)]
pub struct RowShape {
    /// `row`, or XMLTN mapped as a forest.
    element: String,
    /// XSI and the target namespace, mapped as a forest.
    attributes: String,
    /// XMLCNj of each column, and its data type.
    columns: Vec<(String, DataType)>,
    /// NULLS, ENCODING, CHARMAPPING True [§9.10 GR6b v 3 A] and the
    /// SQL-session's time zone displacement.
    mapping: Mapping,
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
        let columns = table.columns.iter().map(|c| (xml_name(&c.name), c.declared.data_type().clone())).collect();
        let mapping = Mapping { encoding: choices.encoding, charmapping: true, zone: choices.zone, nil: choices.nulls == Nulls::Nil };
        RowShape { element, attributes, columns, mapping }
    }

    /// [§9.10 GR6b v] XMLCj, of the value `v` of the column named `name`
    /// of the data type `t`: of the null value nothing, NULLS absent, or an
    /// element nilled, NULLS nil; else the element of the value mapped by
    /// §9.8 \[GR6b v 3 A\]: of a distinct type its source's \[GR1a\], of a
    /// row, an array or a multiset an element of each field or element
    /// \[GR9i, GR9j\], an XML value serialized as CONTENT \[GR9k\].
    fn column(&self, out: &mut String, (name, t): (&str, &DataType), v: &Value) -> Result<()> {
        // Writing to a String cannot fail.
        if v.is_null() {
            if self.mapping.nil {
                let _ = write!(out, r#"<{name} xsi:nil="true"/>"#);
            }
            return Ok(());
        }
        let _ = write!(out, "<{name}>{}</{name}>", xml_mapped(v, t, self.mapping)?);
        Ok(())
    }
}

/// The type of a row's text and of a mapping's data: a CHARACTER LARGE
/// OBJECT of the longest text XMLPARSE reads.
#[must_use]
pub fn rows_type() -> DataType {
    DataType::Lob { kind: LobKind::Character, length: u64::try_from(MAX_INPUT).unwrap_or(u64::MAX), units: CharUnits::Characters }
}

/// \[EXTENSION\] The function of a row of a table a mapping maps, of
/// its XML visible columns' values: the row's text [§9.10 GR6c], written
/// as `.0` says (RULES.md D106).
#[derive(Debug)]
pub struct XmlMapRow(pub Arc<RowShape>);

impl ScalarFunction for XmlMapRow {
    fn name(&self) -> &str {
        XMLMAP
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(rows_type())
    }

    /// [§9.10 GR6] XMLRi of the row of the values `args`; past
    /// `MAX_INPUT` octets, SQLCODE -900.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let shape = &*self.0;
        let mut text = String::new();
        // Writing to a String cannot fail.
        let _ = write!(text, "<{}{}>", shape.element, shape.attributes);
        for (value, (name, t)) in args.iter().zip(&shape.columns) {
            shape.column(&mut text, (name, t), value)?;
            if text.len() > MAX_INPUT {
                return Err(too_long());
            }
        }
        let _ = write!(text, "</{}>", shape.element);
        Ok(Value::Lob(Lob::of_text(&text)))
    }
}

/// \[EXTENSION\] The set function of the rows of a mapping's tables, each
/// the ROW of its table's index and its text, which writes the mapping's
/// data as `.0` says (RULES.md D106).
#[derive(Debug)]
pub struct XmlMapRows(pub Arc<DataShape>);

impl AggregateFunction for XmlMapRows {
    fn name(&self) -> &str {
        XMLMAP
    }

    fn result_type(&self, _: Option<DataType>) -> Result<DataType> {
        Ok(rows_type())
    }

    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        let shape = Arc::clone(&self.0);
        let writer = DataWriter::new(&shape);
        Box::new(Rows { shape, writer })
    }
}

/// The data a mapping has been given rows of, or the error its root was.
struct Rows {
    shape: Arc<DataShape>,
    writer: Result<DataWriter>,
}

impl Accumulator for Rows {
    /// The row `v`, the ROW of its table's index and its text, after those
    /// before it.
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(fields) = v else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] XMLMAP's rows were given {v}, not a row")));
        };
        let (Some(Value::Exact(index)), Some(text)) = (fields.first(), fields.get(1)) else {
            return Err(SqlError::new(Code::Api, "[DESIGN] XMLMAP's rows were given a row of no table and text"));
        };
        let index = usize::try_from(index.units).map_err(|_| SqlError::new(Code::Api, "[DESIGN] XMLMAP was given no table's index"))?;
        let Some(Input::Text(row)) = input(text)? else {
            return Err(SqlError::new(Code::Api, "[DESIGN] XMLMAP was given a row of no text"));
        };
        let writer = self.writer.as_mut().map_err(|e| e.clone())?;
        writer.row(&self.shape, index, &row)
    }

    /// The data's text, each table not given a row of an element of none.
    fn finish(&self) -> Result<Value> {
        let writer = self.writer.as_ref().map_err(Clone::clone)?.clone();
        Ok(Value::Lob(Lob::of_text(&writer.finish(&self.shape)?)))
    }
}
