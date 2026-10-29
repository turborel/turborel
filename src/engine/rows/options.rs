//! \[EXTENSION\] The row store's table options (RULES.md D41): how long a
//! character value may be before it goes out of line, which needs a
//! rebuild, and how its statistics are sampled and refreshed, which change
//! in place. A table's options are those its descriptor sets
//! (`TableDesc::options`), else those `RowStore::configure` gave it, else
//! the store's defaults. Split from engine/rows.rs (RULES.md R-SIZE-2).

use crate::catalog::TableDesc;
use crate::settings::Kind;
use crate::stats::Sampling;
use crate::storage::{Rebuild, TableOption};
use crate::value::{Decimal, Value};

/// \[DESIGN\] The default share of a table's rows changed after which its
/// statistics are gathered again: a tenth.
pub const DEFAULT_REFRESH_FRACTION: f64 = 0.1;
/// \[DESIGN\] The default changes past that share, so a small table's are
/// not gathered on every change.
pub const DEFAULT_REFRESH_ROWS: u64 = 50;

/// \[DESIGN\] The default of TRANSACTIONS.TRACKED_ROWS: the rows of a
/// transaction's reads, or writes, of a table tracked one by one, about
/// a megabyte of them.
pub const DEFAULT_TRACKED_ROWS: usize = 65_536;

/// \[DESIGN\] The default of TRANSACTIONS.HELD_BYTES: the bytes of a
/// transaction's changes to a table held in memory, a megabyte.
pub const DEFAULT_HELD_BYTES: usize = 1 << 20;

/// The names of the row store's table options.
const OUT_OF_LINE: &str = "OUT_OF_LINE";
const SAMPLE: &str = "STATISTICS.SAMPLE";
const BUCKETS: &str = "STATISTICS.BUCKETS";
const REFRESH_FRACTION: &str = "STATISTICS.REFRESH_FRACTION";
const REFRESH_ROWS: &str = "STATISTICS.REFRESH_ROWS";
const TRACKED_ROWS: &str = "TRANSACTIONS.TRACKED_ROWS";
const HELD_BYTES: &str = "TRANSACTIONS.HELD_BYTES";
const LAYOUT: &str = "LAYOUT";
const GROUP_ROWS: &str = "COLUMNS.GROUP_ROWS";
const FREEZE_ROWS: &str = "COLUMNS.FREEZE_ROWS";
const FREEZE_DELAY: &str = "COLUMNS.FREEZE_DELAY";
const REWRITE_FRACTION: &str = "COLUMNS.REWRITE_FRACTION";
/// \[DESIGN\] The default of COLUMNS.REWRITE_FRACTION: a fifth of a group's
/// rows changed, deleted or added to before it is written again, as
/// Vertica's mergeout purges and SQL Server's guidance has it.
pub const DEFAULT_REWRITE_FRACTION: f64 = 0.2;
/// The layouts a table may have.
const LAYOUTS: &[&str] = &["ROWS", "COLUMNS"];

/// \[DESIGN\] The default of COLUMNS.GROUP_ROWS: the rows of a frozen row
/// group, 2^16, 32 vectors of 2048 (RULES.md D31).
pub const DEFAULT_GROUP_ROWS: u32 = 1 << 16;
/// \[DESIGN\] The fewest rows of a frozen row group: one vector.
pub const MIN_GROUP_ROWS: u32 = 2048;
/// \[DESIGN\] The longest COLUMNS.FREEZE_DELAY, in seconds: a year.
const MAX_FREEZE_DELAY: u32 = 366 * 24 * 60 * 60;

/// \[DESIGN\] How a table of the row store keeps its rows (RULES.md D31), and
/// its statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TableOptions {
    /// Character values longer than this, in bytes, go out of line; none
    /// for a quarter of a page's capacity.
    pub out_of_line: Option<usize>,
    /// How its statistics are sampled and refreshed.
    pub statistics: Refresh,
    /// The rows of a transaction's reads, or writes, tracked one by one;
    /// none for `DEFAULT_TRACKED_ROWS`.
    pub tracked_rows: Option<usize>,
    /// The bytes of a transaction's changes held in memory; none for
    /// `DEFAULT_HELD_BYTES`.
    pub held_bytes: Option<usize>,
    /// How its rows are frozen into column groups, if its layout is
    /// COLUMNS; none if it is ROWS.
    pub columns: Option<Columns>,
}

/// \[DESIGN\] How a table of the COLUMNS layout freezes its rows into
/// column groups (RULES.md D31).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Columns {
    /// The rows of each group.
    pub group_rows: u32,
    /// The rows committed past the last group after which a freeze is
    /// due; 0 for `group_rows`.
    pub freeze_rows: u64,
    /// The seconds a row stays as it was written after its commit before
    /// it may be frozen.
    pub freeze_delay: u32,
    /// The share of a group's rows the tree has entries for, changed,
    /// deleted or added, past which the group is written again with them;
    /// 0 for never.
    pub rewrite_fraction: f64,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { group_rows: DEFAULT_GROUP_ROWS, freeze_rows: 0, freeze_delay: 0, rewrite_fraction: DEFAULT_REWRITE_FRACTION }
    }
}

impl Columns {
    /// The rows after which a freeze is due.
    pub(super) fn due(&self) -> u64 {
        if self.freeze_rows == 0 { u64::from(self.group_rows) } else { self.freeze_rows }
    }
}

/// What a table keeps of its transactions in memory: the rows of their
/// reads or writes tracked one by one, and the bytes of their changes
/// held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Bounds {
    pub(super) tracked: usize,
    pub(super) held: usize,
}

impl TableOptions {
    /// What a table of these options keeps of its transactions.
    pub(super) fn bounds(&self) -> Bounds {
        Bounds { tracked: self.tracked_rows.unwrap_or(DEFAULT_TRACKED_ROWS), held: self.held_bytes.unwrap_or(DEFAULT_HELD_BYTES) }
    }
}

/// How a table's statistics are sampled, and when they are gathered again:
/// once the changes since the last pass are more than `fraction` of the
/// rows it found, and `rows` more.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Refresh {
    /// How each column's values are sampled.
    pub sampling: Sampling,
    /// The share of the rows.
    pub fraction: f64,
    /// The changes past it.
    pub rows: u64,
}

impl Default for Refresh {
    fn default() -> Self {
        Refresh { sampling: Sampling::default(), fraction: DEFAULT_REFRESH_FRACTION, rows: DEFAULT_REFRESH_ROWS }
    }
}

fn whole(n: impl TryInto<i128>) -> Value {
    Value::Exact(Decimal::new(n.try_into().unwrap_or(i128::MAX), 0))
}

/// The row store's table options, each defaulting to `defaults`'.
pub(super) fn declared(defaults: TableOptions) -> Vec<TableOption> {
    let stats = defaults.statistics;
    vec![
        TableOption::new(
            (OUT_OF_LINE, Kind::Integer { min: 0, max: 1_i128 << 30_i32 }, whole(defaults.out_of_line.unwrap_or(0))),
            Rebuild::Yes,
            "Character values longer than this, in bytes, are kept out of their row's record; 0 for a quarter of a page.",
        ),
        TableOption::new(
            (SAMPLE, Kind::Integer { min: 1, max: 1_000_000 }, whole(stats.sampling.values)),
            Rebuild::No,
            "The most values of each column the statistics sample.",
        ),
        TableOption::new(
            (BUCKETS, Kind::Integer { min: 1, max: 10_000 }, whole(stats.sampling.buckets)),
            Rebuild::No,
            "The buckets of each column's histogram.",
        ),
        TableOption::new(
            (REFRESH_FRACTION, Kind::Number { min: 0.0_f64, max: 1_000.0_f64 }, Value::Approx(stats.fraction)),
            Rebuild::No,
            "The share of the table's rows changed, and STATISTICS.REFRESH_ROWS more, after which its statistics are gathered again.",
        ),
        TableOption::new(
            (REFRESH_ROWS, Kind::Integer { min: 0, max: i128::from(i64::MAX) }, whole(stats.rows)),
            Rebuild::No,
            "The changes past STATISTICS.REFRESH_FRACTION of the rows after which the statistics are gathered again.",
        ),
        TableOption::new(
            (
                TRACKED_ROWS,
                Kind::Integer { min: 1, max: i128::from(i64::MAX) },
                whole(defaults.tracked_rows.unwrap_or(DEFAULT_TRACKED_ROWS)),
            ),
            Rebuild::No,
            "The rows of a transaction's reads, or writes, of the table tracked one by one to check its commit; past them, every row is counted, which may refuse a commit that need not be.",
        ),
        TableOption::new(
            (HELD_BYTES, Kind::Integer { min: 4 << 10, max: 1_i128 << 40_i32 }, whole(defaults.held_bytes.unwrap_or(DEFAULT_HELD_BYTES))),
            Rebuild::No,
            "The bytes of a transaction's changes to the table held in memory, to undo them and check others' reads by; past them, changes are written to a spill area, and a committed one's rows kept as each column's least and greatest value.",
        ),
    ]
    .into_iter()
    .chain(layout_declared(defaults))
    .collect()
}

/// The options of a table's layout, each defaulting to `defaults`'.
fn layout_declared(defaults: TableOptions) -> Vec<TableOption> {
    let cols = defaults.columns.unwrap_or_default();
    vec![
        TableOption::new(
            (LAYOUT, Kind::Choice(LAYOUTS), Value::Char(if defaults.columns.is_some() { "COLUMNS" } else { "ROWS" }.into())),
            Rebuild::Yes,
            "How the table keeps its rows: ROWS, each row's values together; COLUMNS, rows written kept so until they are frozen into groups, each column's values together.",
        ),
        columns_option(
            GROUP_ROWS,
            (MIN_GROUP_ROWS.into(), DEFAULT_GROUP_ROWS.into(), whole(cols.group_rows)),
            "The rows of each group a table of the COLUMNS layout freezes; groups frozen before keep theirs.",
        ),
        columns_option(
            FREEZE_ROWS,
            (0, i128::from(i64::MAX), whole(cols.freeze_rows)),
            "The rows committed past the last group after which a table of the COLUMNS layout freezes them; 0 for COLUMNS.GROUP_ROWS.",
        ),
        columns_option(
            FREEZE_DELAY,
            (0, MAX_FREEZE_DELAY.into(), whole(cols.freeze_delay)),
            "The seconds a row of a table of the COLUMNS layout stays as it was written after its commit before it may be frozen, so rows changed soon after are not.",
        ),
        TableOption::new(
            (REWRITE_FRACTION, Kind::Number { min: 0.0_f64, max: 1.0_f64 }, Value::Approx(cols.rewrite_fraction)),
            Rebuild::No,
            "The share of a frozen group's rows changed, deleted or added to, past which a table of the COLUMNS layout writes the group again with them; 0 for never.",
        ),
    ]
}

/// An option of the COLUMNS layout, a whole number from `min` to `max`,
/// `default` until set, in place.
fn columns_option(name: &str, (min, max, default): (i128, i128, Value), doc: &str) -> TableOption {
    TableOption::new((name, Kind::Integer { min, max }, default), Rebuild::No, doc)
}

/// The encodings, by commas, that `desc` sets its rows to pass through
/// (the database's option `ENCODINGS`).
pub(super) fn encodings_of(desc: &TableDesc) -> String {
    crate::storage::encodings_of(&desc.options).to_owned()
}

/// `base`, how a table's statistics are kept, with the options `desc` sets.
pub(super) fn refresh_of(desc: &TableDesc, base: Refresh) -> Refresh {
    resolved(desc, TableOptions { statistics: base, ..TableOptions::default() }).statistics
}

/// What a table keeps of its transactions, as `desc` sets it, else `base`.
pub(super) fn bounds_of(desc: &TableDesc, base: Bounds) -> Bounds {
    resolved(desc, TableOptions { tracked_rows: Some(base.tracked), held_bytes: Some(base.held), ..TableOptions::default() }).bounds()
}

/// How a table freezes its rows, as `desc` sets it, else `base`.
pub(super) fn columns_of(desc: &TableDesc, base: Option<Columns>) -> Option<Columns> {
    resolved(desc, TableOptions { columns: base, ..TableOptions::default() }).columns
}

/// `base` with the options `desc` sets.
pub(super) fn resolved(desc: &TableDesc, base: TableOptions) -> TableOptions {
    let set = |name: &str| desc.options.iter().find(|(n, _)| n == name).map(|(_, v)| v);
    let count = |name: &str| set(name).and_then(|v| if let Value::Exact(d) = v { u64::try_from(d.units).ok() } else { None });
    let size = |name: &str| count(name).and_then(|n| usize::try_from(n).ok());
    let mut out = base;
    if let Some(n) = size(OUT_OF_LINE) {
        out.out_of_line = (n > 0).then_some(n);
    }
    out.statistics.sampling.values = size(SAMPLE).unwrap_or(base.statistics.sampling.values);
    out.statistics.sampling.buckets = size(BUCKETS).unwrap_or(base.statistics.sampling.buckets);
    out.statistics.fraction = set(REFRESH_FRACTION).and_then(Value::as_f64).unwrap_or(base.statistics.fraction);
    out.statistics.rows = count(REFRESH_ROWS).unwrap_or(base.statistics.rows);
    out.tracked_rows = size(TRACKED_ROWS).or(base.tracked_rows);
    out.held_bytes = size(HELD_BYTES).or(base.held_bytes);
    let layout = set(LAYOUT).map(ToString::to_string);
    let columns = match layout.as_deref().map(str::trim_end) {
        Some("COLUMNS") => Some(base.columns.unwrap_or_default()),
        Some(_) => None,
        None => base.columns,
    };
    out.columns = columns.map(|c| Columns {
        group_rows: count(GROUP_ROWS).and_then(|n| u32::try_from(n).ok()).unwrap_or(c.group_rows),
        freeze_rows: count(FREEZE_ROWS).unwrap_or(c.freeze_rows),
        freeze_delay: count(FREEZE_DELAY).and_then(|n| u32::try_from(n).ok()).unwrap_or(c.freeze_delay),
        rewrite_fraction: set(REWRITE_FRACTION).and_then(Value::as_f64).unwrap_or(c.rewrite_fraction),
    });
    out
}
