//! \[DESIGN\] Whether a plan learned still fits the tables it reads
//! (RULES.md D45, D613). A plan learned is learned again once a table it
//! reads gains or loses rows as its statistics would be gathered again
//! for. A plan that reads every table only through index probes is not:
//! a probe finds the rows of its keys, which a table's growth seldom
//! changes, so the plan made by values would be the same. It is learned
//! again only once a probe's estimated rows per probe, its table's rows
//! over the distinct keys of the index columns it fixes, grows past
//! `OUTGROWN` times its figure as the plan was learned; until then each
//! drift of its tables moves the rows it is measured from. A figure taken
//! before its table had statistics is its whole table's rows, not its rows
//! per probe, so a plan learned so is learned again once they are known,
//! and its figures taken anew.

use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::engine::rows::{DEFAULT_REFRESH_FRACTION, DEFAULT_REFRESH_ROWS};
use crate::index::Probe;
use crate::physical::{JoinProbe, Plan, Preserved};
use crate::storage::cost_of;

/// \[DESIGN\] How many times its figure as its plan was learned a probe's
/// estimated rows per probe may grow before the plan is learned again:
/// ten, an order of magnitude, past which a probe's rows may well cost
/// more than another path's.
pub(super) const OUTGROWN: f64 = 10.0_f64;

/// What a plan learned is measured from: its tables' rows as it was
/// learned, or last drifted, and of a plan that reads only through index
/// probes, each probe's rows per probe as it was learned.
#[derive(Debug, Clone)]
pub(crate) struct Learnt {
    reads: Arc<[QualifiedName]>,
    rows: Arc<[usize]>,
    probes: Option<Arc<[Probed]>>,
}

/// A probe of a plan learned: its table, the columns it fixes, its
/// estimated rows per probe as the plan was learned, and whether its
/// table's statistics were known then (`figure`).
#[derive(Debug, Clone)]
struct Probed {
    table: QualifiedName,
    columns: Vec<usize>,
    figure: f64,
    known: bool,
}

/// How a plan learned fits its tables now (`Learnt::now`).
pub(crate) enum Fit {
    /// As it was: no table drifted.
    Still,
    /// Its tables drifted but its probes did not outgrow it: kept, measured
    /// from these rows on.
    Grown(Learnt),
    /// It is to be learned again.
    Moved,
}

impl Learnt {
    /// What `plan`, learned in `st`, reading `reads`, is measured from.
    pub(crate) fn of(st: &State, reads: &Arc<[QualifiedName]>, plan: &Plan) -> Learnt {
        let mut found = Vec::new();
        let probe = |(table, columns): (QualifiedName, Vec<usize>)| {
            let (figure, known) = figure(st, &table, &columns);
            Probed { table, columns, figure, known }
        };
        let probes = probed(plan, st, &mut found).then(|| found.into_iter().map(probe).collect());
        Learnt { reads: Arc::clone(reads), rows: rows_of(st, reads), probes }
    }

    /// The rows of `reads` in `st`, of a query that stopped learning: it
    /// learns again once one drifts.
    pub(crate) fn rows(st: &State, reads: &Arc<[QualifiedName]>) -> Learnt {
        Learnt { reads: Arc::clone(reads), rows: rows_of(st, reads), probes: None }
    }

    /// How the plan fits its tables in `st` now: moved, if a probe's
    /// table's statistics became known since it was learned; still, if no
    /// table drifted (`drifted`); grown, if its probes did not outgrow it;
    /// else moved.
    pub(crate) fn now(&self, st: &State) -> Fit {
        let reads = &self.reads;
        let probes = self.probes.as_deref().unwrap_or_default();
        if probes.iter().any(|p| !p.known && st.statistics(&p.table).is_some()) {
            return Fit::Moved;
        }
        if !reads.iter().zip(self.rows.iter()).any(|(t, &was)| drifted(was, st.row_count(t).unwrap_or(0))) {
            return Fit::Still;
        }
        let Some(kept) = &self.probes else { return Fit::Moved };
        if kept.iter().any(|p| outgrown(p.figure, figure(st, &p.table, &p.columns).0)) {
            return Fit::Moved;
        }
        Fit::Grown(Learnt { reads: Arc::clone(reads), rows: rows_of(st, reads), probes: Some(Arc::clone(kept)) })
    }
}

/// Whether a probe of `learned` rows per probe that now has `now` has
/// outgrown its plan: more than `OUTGROWN` times as many.
fn outgrown(learned: f64, now: f64) -> bool {
    now > learned.max(1.0_f64) * OUTGROWN
}

/// The rows of each table of `reads` in `st`.
fn rows_of(st: &State, reads: &[QualifiedName]) -> Arc<[usize]> {
    reads.iter().map(|t| st.row_count(t).unwrap_or(0)).collect()
}

/// \[DESIGN\] Whether a table of `was` rows that now has `now` gained or
/// lost more than its statistics are gathered again for by default
/// (`STATISTICS.REFRESH_FRACTION` of them and `REFRESH_ROWS`), so a plan
/// learned follows what the statistics follow.
fn drifted(was: usize, now: usize) -> bool {
    let least = u32::try_from(DEFAULT_REFRESH_ROWS).map_or(f64::MAX, f64::from);
    cost_of(now.abs_diff(was)) > cost_of(was) * DEFAULT_REFRESH_FRACTION + least
}

/// The estimated rows per probe of `table` fixing `columns`: its rows over
/// the distinct values its statistics count of them together, or of each
/// apart, multiplied; its rows if it fixes none, or nothing is known.
/// And whether its statistics are known.
fn figure(st: &State, table: &QualifiedName, columns: &[usize]) -> (f64, bool) {
    let rows = cost_of(st.row_count(table).unwrap_or(0));
    let Some(stats) = st.statistics(table) else { return (rows, false) };
    if columns.is_empty() {
        return (rows, true);
    }
    let apart = || columns.iter().map(|&c| stats.distinct(c)).product::<Option<f64>>();
    let distinct = stats.group_distinct(columns).or_else(apart).unwrap_or(1.0_f64);
    (rows / distinct.max(1.0_f64), true)
}

/// Whether `plan` reads every table through index probes alone, each
/// probe's table and the columns it fixes added to `found`.
fn probed(plan: &Plan, st: &State, found: &mut Vec<(QualifiedName, Vec<usize>)>) -> bool {
    match plan {
        Plan::IndexScan { table, index, probe, .. } => {
            let Some(fixed) = fixed_of(probe) else { return false };
            let Some(ix) = st.catalog.index(index) else { return false };
            found.push((table.clone(), ix.columns.iter().copied().take(fixed).collect()));
            true
        }
        Plan::IndexJoin { outer: Preserved::Inner, spatial: None, left, table, index, probe: JoinProbe::Prefix(columns), .. } => {
            let Some(ix) = st.catalog.index(index) else { return false };
            found.push((table.clone(), ix.columns.iter().copied().take(columns.len()).collect()));
            probed(left, st, found)
        }
        Plan::Filter { input, .. }
        | Plan::Project { input, .. }
        | Plan::Aggregate { input, .. }
        | Plan::Distinct { input, .. }
        | Plan::Sort { input, .. }
        | Plan::Limit { input, .. } => probed(input, st, found),
        Plan::Product { left, right, .. } | Plan::HashJoin { left, right, .. } | Plan::Append { left, right, .. } => {
            probed(left, st, found) && probed(right, st, found)
        }
        Plan::Values { .. } => true,
        Plan::SeqScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::Nearest(_)
        | Plan::MergeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::RangeJoin(_)
        | Plan::OuterJoin { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::MergeAppend { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::Counted(_) => false,
    }
}

/// How many of an index's first columns `probe` fixes equal, if it is a
/// probe of keys: of a comparison, one if an equality, else none.
fn fixed_of(probe: &Probe) -> Option<usize> {
    match probe {
        Probe::Compare { op, .. } => Some(usize::from(*op == crate::value::CompOp::Eq)),
        Probe::Prefix { equal, .. } | Probe::Range { equal, .. } => Some(equal.len()),
        Probe::Columns(_) | Probe::All | Probe::Predicate { .. } | Probe::Xml(_) => None,
    }
}

// Unit tests: when a table drifts and when a probe outgrows its plan
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D45: a table has drifted from a plan
    /// learned once it gains or loses more than a tenth of its rows and
    /// 50 more, as its statistics are gathered again by default; at the
    /// bound, not.
    #[test]
    fn a_table_drifts_past_its_statistics_refresh() {
        assert!(!drifted(1000, 1150) && drifted(1000, 1151));
        assert!(!drifted(1000, 850) && drifted(1000, 849));
        assert!(!drifted(0, 50) && drifted(0, 51));
    }

    /// Verifies: [DESIGN] RULES.md D613: a probe outgrows its plan once
    /// its rows per probe pass ten times its figure as learned, a figure
    /// below one taken as one; at ten times, not.
    #[test]
    fn a_probe_outgrows_its_plan_past_ten_times_its_rows() {
        assert!(!outgrown(4.0_f64, 40.0_f64) && outgrown(4.0_f64, 40.5_f64));
        assert!(!outgrown(0.25_f64, 10.0_f64) && outgrown(0.25_f64, 10.5_f64), "below one row, as of one");
        assert!(!outgrown(4.0_f64, 1.0_f64), "fewer rows");
    }
}
