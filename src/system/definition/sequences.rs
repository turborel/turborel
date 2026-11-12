//! \[EXTENSION\] The table of sequence generators [ISO/IEC 9075-2:2023
//! §4.29.1], DEFINITION_SCHEMA.SEQUENCES, which the view
//! INFORMATION_SCHEMA.SEQUENCES reads. ISO/IEC 9075:1992 has no sequence
//! generators, so §21.3 defines no table of them. Each row is a
//! generator's name, its data type, and its start, minimum, maximum and
//! increment, each as text, as a value of NUMERIC(38) may not be held by
//! CARDINAL_NUMBER; and YES if it cycles.

use std::sync::Arc;

use super::{Columns, catalog_name, defined, ids, number, text, yes};
use crate::catalog::ColumnDesc;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::Value;

/// The table.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![defined("SEQUENCES", sequences_columns, Rows::Infallible(sequences))]
}

fn sequences_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("SEQUENCE", true)
        .data("DATA_TYPE", true)
        .number("NUMERIC_PRECISION", false)
        .number("NUMERIC_PRECISION_RADIX", false)
        .number("NUMERIC_SCALE", false)
        .data("START_VALUE", true)
        .data("MINIMUM_VALUE", true)
        .data("MAXIMUM_VALUE", true)
        .data("INCREMENT", true)
        .data("CYCLE_OPTION", true)
        .done()
}

/// Each sequence generator of the catalog.
fn sequences(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let optional = |n: Option<u32>| n.and_then(|n| usize::try_from(n).ok()).map_or(Value::Null, number);
    seen.state
        .catalog
        .sequences()
        .into_iter()
        .map(|s| {
            let d = s.data_type.kind().descriptor(&s.data_type);
            let described = [text(&d.name), optional(d.precision), optional(d.radix), optional(d.scale)];
            let values = [s.start, s.minimum, s.maximum, s.increment].map(|v| text(&v.to_string()));
            [ids(&catalog, &s.name).to_vec(), described.to_vec(), values.to_vec(), vec![yes(s.cycle)]].concat()
        })
        .collect()
}
