//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] The catalog of VECTOR (design
//! document §16, RULES.md D93), objects "associated with
//! INFORMATION_SCHEMA" this implementation adds. VECTOR_COLUMNS has a row
//! for each column of a VECTOR type, with the dimension count, format and
//! layout it fixes, null of one it leaves flexible. VECTOR_DISTANCE_FUNCTIONS
//! has a row for each distance function of SQL, with its language, the
//! properties it declares, the formats it measures and its owner.
//!
//! A user sees the columns of each table on which it holds a privilege,
//! and each distance function on which it holds EXECUTE, as the views of
//! the Information Schema show what is "accessible to" it.

use std::sync::Arc;

use super::{Seen, SystemTable, cardinal, catalog_name, identifier, number, text, text_column, yes};
use crate::catalog::{ColumnDesc, Who, applicable, owner_of};
use crate::error::Result;
use crate::storage::Row;
use crate::value::Value;
use crate::vector::{Element, shape_of};

/// The longest list of formats: each of the five, with commas.
const FORMATS_LENGTH: u32 = 64;
/// The longest name of a format or layout.
const WORD_LENGTH: u32 = 16;
/// The longest body of a distance function shown; one longer is null, as
/// a routine's definition longer than CHARACTER_DATA holds is.
const DEFINITION_LENGTH: u32 = 4096;
/// The one language built in.
const SQL: &str = "SQL";

/// VECTOR_COLUMNS.
struct Columns;

/// VECTOR_DISTANCE_FUNCTIONS.
struct DistanceFunctions;

/// The tables.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 2] {
    [Arc::new(Columns), Arc::new(DistanceFunctions)]
}

/// The user of `seen`, as privileges know it.
fn who<'a>(seen: &'a Seen<'_>) -> Who<'a> {
    Who::sole(seen.state.catalog.roles(), seen.auth)
}

/// A text value, or null.
fn or_null(s: Option<&str>) -> Value {
    s.map_or(Value::Null, text)
}

impl SystemTable for Columns {
    fn name(&self) -> &'static str {
        "VECTOR_COLUMNS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let names = ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"];
        let mut columns: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
        columns.push(cardinal("ORDINAL_POSITION", true));
        columns.push(cardinal("VECTOR_DIMENSIONS", false));
        columns.push(text_column(("VECTOR_FORMAT", WORD_LENGTH), false));
        columns.push(text_column(("VECTOR_LAYOUT", WORD_LENGTH), false));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let catalog = catalog_name(seen);
        let mut rows = Vec::new();
        for table in seen.state.catalog.tables() {
            if applicable(&*seen.state.catalog, who(seen), &table.name).0.is_empty() {
                continue;
            }
            for (at, column) in table.columns.iter().enumerate() {
                let Some(shape) = shape_of(&column.data_type) else { continue };
                rows.push(vec![
                    catalog.clone(),
                    text(&table.name.auth),
                    text(&table.name.ident),
                    text(&column.name),
                    number(at.saturating_add(1)),
                    shape.dims.map_or(Value::Null, |d| number(usize::try_from(d).unwrap_or(usize::MAX))),
                    or_null(shape.element.map(Element::name)),
                    or_null(shape.layout.map(crate::vector::Layout::name)),
                ]);
            }
        }
        Ok(rows)
    }
}

impl SystemTable for DistanceFunctions {
    fn name(&self) -> &'static str {
        "VECTOR_DISTANCE_FUNCTIONS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let names = ["FUNCTION_CATALOG", "FUNCTION_SCHEMA", "FUNCTION_NAME", "SPECIFIC_NAME"];
        let mut columns: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
        columns.push(text_column(("LANGUAGE", WORD_LENGTH), true));
        for flag in ["IS_DETERMINISTIC", "SYMMETRIC", "NON_NEGATIVE", "TRIANGLE_INEQUALITY"] {
            columns.push(text_column((flag, 3), true));
        }
        columns.push(text_column(("FORMATS", FORMATS_LENGTH), true));
        columns.push(identifier("FUNCTION_OWNER", true));
        columns.push(text_column(("DEFINITION", DEFINITION_LENGTH), false));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let catalog = catalog_name(seen);
        let mut rows = Vec::new();
        for r in seen.state.catalog.routines() {
            let Some(d) = &r.distance else { continue };
            if !r.execute_of(who(seen)).0 {
                continue;
            }
            let formats: Vec<Element> =
                if d.formats.is_empty() { Element::ALL.to_vec() } else { d.formats.iter().filter_map(|c| Element::of_code(*c)).collect() };
            let formats = formats.iter().map(|e| e.name()).collect::<Vec<_>>().join(",");
            let shown = usize::try_from(DEFINITION_LENGTH).is_ok_and(|most| d.expression.chars().count() <= most);
            rows.push(vec![
                catalog.clone(),
                text(&r.name.auth),
                text(&r.name.ident),
                text(&r.specific.ident),
                text(SQL),
                yes(r.deterministic),
                yes(d.symmetric),
                yes(d.non_negative),
                yes(d.triangle),
                text(&formats),
                text(owner_of(&*seen.state.catalog, &r.name.auth)),
                if shown { text(&d.expression) } else { Value::Null },
            ]);
        }
        Ok(rows)
    }
}
