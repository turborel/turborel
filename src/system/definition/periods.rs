//! \[EXTENSION\] The table of period descriptors [ISO/IEC 9075-2:2023
//! §4.16.1], DEFINITION_SCHEMA.PERIODS, which the view
//! INFORMATION_SCHEMA.PERIODS reads. ISO/IEC 9075-11:2006 has no periods,
//! so it defines no table of them. Each row is a period's table, its name,
//! its start and end columns, and the name of its period constraint.

use std::sync::Arc;

use super::{Columns, catalog_name, defined, ids, text};
use crate::catalog::ColumnDesc;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::Value;

/// The table.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![defined("PERIODS", periods_columns, Rows::Infallible(periods))]
}

fn periods_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("PERIOD_NAME", true)
        .id("START_COLUMN_NAME", true)
        .id("END_COLUMN_NAME", true)
        .ids("CONSTRAINT", false)
        .done()
}

/// Each period of each table of the catalog.
fn periods(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.tables() {
        for p in &t.periods {
            let named = [text(&p.name), text(&p.start), text(&p.end)];
            let constraint = p.constraint.as_ref().map_or([Value::Null, Value::Null, Value::Null], |c| ids(&catalog, c));
            rows.push([ids(&catalog, &t.name).to_vec(), named.to_vec(), constraint.to_vec()].concat());
        }
    }
    rows
}
