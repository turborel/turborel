//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.COLUMN_STATISTICS
//! (RULES.md D990, D991): what the planner's statistics hold of each column
//! of each base table, as the planner reads them (`State::statistics`,
//! `stats.rs`).
//!
//! A row is of a column. It shows the rows the last pass found and when
//! that pass was made, the share of the column's rows that is NULL, its
//! distinct values (an index whose key is the column alone counting them,
//! as the planner takes them), the average width of a row read for it,
//! its least and greatest values, its most common values with the share
//! of the rows each holds, and the bounds of its histogram. Each is NULL
//! while its table's engine knows none.
//!
//! A view's column has one declared type, the same in every row, so the
//! values of the table's columns are each shown as a character string,
//! its text as a client shows it, from which a CAST to the column's type
//! reads it back [ISO/IEC 9075-2:2023 §6.13], cut at `TEXT_LENGTH`
//! characters; the most common and the bounds are arrays of them.
//!
//! Reading the view reads the statistics as a plan does: a table's not
//! gathered yet are due, and the database's maintenance gathers them.
//!
//! The database's owner, and a user granted the role MONITOR the owner
//! created (`Seen::may_monitor`, RULES.md D520), see every column's row;
//! any other user those of the columns on which it holds a privilege, as
//! COLUMNS shows the columns "accessible to a given user" [ISO/IEC
//! 9075:1992 §21.2.9; ISO/IEC 9075-11:2006 draft §5.21].

use std::sync::Arc;

use super::statements::{TEXT_LENGTH, cut, time, time_column};
use super::{Seen, SystemTable, cardinal, catalog_name, identifier, number, text};
use crate::catalog::{Actions, ColumnDesc, TableDesc, Who, applicable};
use crate::error::Result;
use crate::stats::Statistics;
use crate::storage::Row;
use crate::value::cast::approximate_to_exact;
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::{BIGINT_PRECISION, CharUnits, DataType, Spelled, Value};

/// \[DESIGN\] The decimal places of a share or a width shown.
const PLACES: u32 = 4;
/// The columns of figures, past the five naming the column.
const FIGURES: usize = 10;

/// COLUMN_STATISTICS.
struct ColumnStatistics;

/// The table.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 1] {
    [Arc::new(ColumnStatistics)]
}

/// A column `name` of `data_type`, which may be null.
fn nullable(name: &str, data_type: DataType) -> ColumnDesc {
    ColumnDesc { data_type, ..identifier(name, false) }
}

/// The type of a count: BIGINT.
fn whole_type() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// The type of a share or a width: DECIMAL(18, 4).
fn share_type() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: PLACES, spelled: Spelled::Decimal }
}

/// The type of a value's text: CHARACTER VARYING(`TEXT_LENGTH`).
fn text_type() -> DataType {
    DataType::Varying { length: TEXT_LENGTH, units: CharUnits::Characters }
}

/// `x`, an estimate, rounded to a whole number; NULL if not known.
pub(super) fn whole(x: Option<f64>) -> Value {
    x.and_then(|x| approximate_to_exact(x, (BIGINT_PRECISION, 0), "[DESIGN] a statistic shown").ok()).unwrap_or(Value::Null)
}

/// `x` to `PLACES` places; NULL if not known.
fn share(x: Option<f64>) -> Value {
    x.and_then(|x| approximate_to_exact(x, (BIGINT_PRECISION, PLACES), "[DESIGN] a statistic shown").ok()).unwrap_or(Value::Null)
}

/// `value` as its text, cut.
fn as_text(value: &Value) -> Value {
    cut(&value.to_string())
}

/// An array of `values`, at most the most an array holds.
fn array(values: impl Iterator<Item = Value>) -> Value {
    Value::Array(values.take(usize::try_from(MAX_ARRAY_CARDINALITY).unwrap_or(usize::MAX)).collect())
}

/// \[DESIGN\] Whether `held` is a privilege on the column at `column`: a
/// column privilege, SELECT, INSERT, UPDATE or REFERENCES on it [ISO/IEC
/// 9075-2:2023 §4.42.2], SELECT on the table being SELECT on each of its
/// columns.
pub(super) fn on_column(held: &Actions, column: usize) -> bool {
    held.select || held.insert.contains(&column) || held.update.contains(&column) || held.references.contains(&column)
}

/// The figures of the column at `column` in `stats`, its table's last
/// pass made at `gathered`; each NULL if `stats` are none.
fn figures(stats: Option<&dyn Statistics>, column: usize, gathered: Option<i64>) -> [Value; FIGURES] {
    let Some(s) = stats else { return std::array::from_fn(|_| Value::Null) };
    let (least, greatest) = s.bounds(column).map_or((Value::Null, Value::Null), |(l, g)| (as_text(&l), as_text(&g)));
    let common = s.common(column);
    [
        whole(s.rows()),
        gathered.map_or(Value::Null, |at| time(crate::value::datetime::utc_of_unix(at))),
        share(s.nulls(column)),
        whole(s.distinct(column)),
        share(s.width(&[column])),
        least,
        greatest,
        common.map_or(Value::Null, |c| array(c.iter().map(|(v, _)| as_text(v)))),
        common.map_or(Value::Null, |c| array(c.iter().map(|&(_, f)| share(Some(f))))),
        s.histogram(column).map_or(Value::Null, |h| array(h.iter().map(as_text))),
    ]
}

/// The rows of `t`'s columns `columns`, as `seen` reads its statistics.
fn rows_of(seen: &Seen<'_>, t: &TableDesc, columns: &[usize]) -> Vec<Row> {
    let stats = seen.state.statistics(&t.name);
    let gathered = seen.state.lock_of(&t.name).ok().and_then(|lock| lock.gathered(None));
    let mut rows = Vec::new();
    for &at in columns {
        let Some(c) = t.columns.get(at) else { continue };
        let mut row = vec![catalog_name(seen), text(&t.name.auth), text(&t.name.ident), text(&c.name), number(at.saturating_add(1))];
        row.extend(figures(stats.as_deref(), at, gathered));
        rows.push(row);
    }
    rows
}

impl SystemTable for ColumnStatistics {
    fn name(&self) -> &'static str {
        "COLUMN_STATISTICS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut columns: Vec<ColumnDesc> =
            ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"].iter().map(|n| identifier(n, true)).collect();
        columns.push(cardinal("ORDINAL_POSITION", true));
        columns.extend([nullable("ROW_COUNT", whole_type()), time_column("GATHERED"), nullable("NULL_FRACTION", share_type())]);
        columns.extend([nullable("DISTINCT_VALUES", whole_type()), nullable("AVERAGE_WIDTH", share_type())]);
        columns.extend(["MIN_VALUE", "MAX_VALUE"].map(|n| nullable(n, text_type())));
        let arrays = |element: DataType| DataType::array(element, MAX_ARRAY_CARDINALITY);
        columns.push(nullable("MOST_COMMON_VALUES", arrays(text_type())));
        columns.push(nullable("MOST_COMMON_FREQUENCIES", arrays(share_type())));
        columns.push(nullable("HISTOGRAM_BOUNDS", arrays(text_type())));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let every = seen.may_monitor();
        let who = Who::sole(seen.state.catalog.roles(), seen.auth);
        let mut tables: Vec<&TableDesc> = seen.state.catalog.tables();
        tables.sort_by(|a, b| (&a.name.auth, &a.name.ident).cmp(&(&b.name.auth, &b.name.ident)));
        let mut rows = Vec::new();
        for t in tables {
            let held = applicable(&*seen.state.catalog, who, &t.name).0;
            let shown: Vec<usize> = (0..t.columns.len()).filter(|&c| every || on_column(&held, c)).collect();
            // A table none of whose columns are shown is not asked for its statistics.
            if !shown.is_empty() {
                rows.extend(rows_of(seen, t, &shown));
            }
        }
        Ok(rows)
    }
}
