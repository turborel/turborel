//! \[DESIGN\] (RULES.md D651) What an operator holding rows costs past its
//! work grant: a hash join's built rows, a hashed grouping's groups and a
//! sort's rows. The grant is what `spill/grant.rs` would give the
//! operator as the statement is planned (RULES.md D298): half of what the
//! grants' half of the budget has free, at most MEMORY.WORK_BYTES, at
//! least the floor every operator holds. Rows are held at their width,
//! the bytes `Statistics::width` says the columns read take, and an
//! entry's bytes more of the table holding them.
//!
//! An operator whose rows outgrow the grant writes them to the spill area
//! and reads them back once more. A hash join past its grant partitions
//! both its operands, as a Grace hash join does (`exec/join/partitioned.rs`),
//! so each row of either is written and read once more; a grouping writes
//! its partial groups and its rows past the grant; a sort writes runs and
//! merges them. Each such row costs `SPILLED_ROW`, and each byte of it
//! `SPILLED_BYTE`, in rows of a scan, as `bench/analytics/costs.rs`
//! measures them. So a merge join of inputs already in order, or a
//! grouping streamed in its keys' order, wins where holding the rows
//! would spill, and a wide build fills its grant sooner than a narrow one.

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::physical::PhysicalPlanner;
use crate::settings::MEMORY_WORK_BYTES;
use crate::spill::WORK_FLOOR;
use crate::storage::cost_of;

/// \[DESIGN\] The cost of a row written to the spill area and read back
/// once more, in rows of a scan (`bench/analytics/costs.rs`, its spilled
/// hash join, grouping and sort).
pub(in crate::physical) const SPILLED_ROW: f64 = 1.0;
/// \[DESIGN\] The cost more of each byte of such a row, in rows of a scan:
/// a spilled row of 100 bytes costs a scanned row more than one of none.
pub(in crate::physical) const SPILLED_BYTE: f64 = 0.01;
/// \[DESIGN\] The bytes an operator's table holds for each row besides its
/// values: its hash, its link and its slot.
pub(in crate::physical) const ENTRY_BYTES: f64 = 16.0;
/// \[DESIGN\] The bytes of a column's value held when its width is not
/// known: an integer's.
pub(in crate::physical) const COLUMN_BYTES: f64 = 8.0;

/// The bytes an operator would be granted if it started as the statement
/// of `db` is planned: half of the grants' share of the budget free, at
/// most MEMORY.WORK_BYTES, at least the floor, or the ceiling itself
/// below the floor (`spill/grant.rs`).
pub(crate) fn grant_bytes(db: &State) -> f64 {
    let ceiling = crate::settings::work_bytes(db.settings.get(MEMORY_WORK_BYTES));
    let limit = db.memory.limit();
    let free = limit.saturating_sub(db.memory.reserved()).saturating_sub(limit >> 1_u32);
    let granted = if ceiling <= WORK_FLOOR { ceiling } else { ceiling.min(free >> 1_u32).max(WORK_FLOOR) };
    cost_of(granted)
}

/// The bytes a row of `columns` columns held by an operator takes, of
/// `width` bytes of values if known.
pub(in crate::physical) fn row_bytes(columns: usize, width: Option<f64>) -> f64 {
    width.unwrap_or_else(|| cost_of(columns) * COLUMN_BYTES) + ENTRY_BYTES
}

/// The cost more of an operator holding `held` rows of `bytes` bytes
/// each, past a grant of `grant` bytes, which writes and reads back
/// `moved` rows once more if they outgrow it: none if they fit.
pub(in crate::physical) fn spilled((held, bytes): (f64, f64), moved: f64, grant: f64) -> f64 {
    if held * bytes <= grant {
        return 0.0_f64;
    }
    moved * (SPILLED_ROW + SPILLED_BYTE * bytes)
}

/// The cost more of a hash join building `built` rows of `bytes` bytes
/// and probing with `probing` rows, past a grant of `grant` bytes: both
/// operands partitioned, each row written and read once more.
pub(in crate::physical) fn hash_spilled((built, bytes): (f64, f64), probing: f64, grant: f64) -> f64 {
    spilled((built, bytes), built + probing, grant)
}

/// The cost more of a sort of `rows` rows of `bytes` bytes past a grant
/// of `grant` bytes: its runs written and merged once.
pub(in crate::physical) fn sort_spilled((rows, bytes): (f64, f64), grant: f64) -> f64 {
    spilled((rows, bytes), rows, grant)
}

impl PhysicalPlanner<'_> {
    /// The bytes a row of `columns` columns of `table` holds: its share of
    /// the table's average row, as the statistics say, else each column's
    /// an integer's.
    pub(in crate::physical) fn row_bytes_of(&self, table: &QualifiedName, columns: usize) -> f64 {
        let all = self.db.catalog.table(table).map_or(columns, |t| t.columns.len());
        let share = (cost_of(columns) / cost_of(all).max(1.0_f64)).min(1.0_f64);
        row_bytes(columns, self.statistics(table).and_then(|s| s.width(&[])).map(|w| w * share))
    }
}
