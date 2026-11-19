//! \[DESIGN\] The estimates a plan form carries (RULES.md D279): what the
//! planner has of each node, from the statistics the database keeps of
//! its tables (`stats.rs`) and the selectivities it prices conditions by
//! (`physical/estimate.rs`). A node the planner prices no estimate of,
//! or whose input is not known, is not known (negative).

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::logical::{Cond, LogicalPlan};
use crate::physical::estimate::{Known, selectivity};
use crate::stats::Statistics;

use super::ColumnStatistics;

/// A number not known, as the form writes one.
pub const UNKNOWN: f64 = -1.0;

/// Rows and a row's width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sized {
    /// Rows; negative if not known.
    pub rows: f64,
    /// Bytes of a row; negative if not known.
    pub width: f64,
}

impl Sized {
    /// Nothing known.
    pub const UNKNOWN: Sized = Sized { rows: UNKNOWN, width: UNKNOWN };

    /// Whether its rows are known.
    fn known(self) -> bool {
        self.rows >= 0.0_f64
    }
}

/// `x`, or `UNKNOWN` if none.
fn or_unknown(x: Option<f64>) -> f64 {
    x.filter(|n| n.is_finite() && *n >= 0.0_f64).unwrap_or(UNKNOWN)
}

/// What the database keeps of `table`: its rows and width, and each of
/// its `width` columns' distinct values, nulls and bounds.
pub fn scanned(st: &State, table: &QualifiedName, width: usize) -> (Sized, Vec<ColumnStatistics>) {
    let stats = st.statistics(table);
    let counted = st.row_count(table).and_then(|n| u32::try_from(n).ok()).map(f64::from);
    let rows = or_unknown(stats.as_ref().and_then(|s| s.rows()).or(counted));
    let all: Vec<usize> = (0..width).collect();
    let bytes = or_unknown(stats.as_ref().and_then(|s| s.width(&all)));
    let columns = match &stats {
        Some(s) => (0..width).map(|c| column(&**s, c)).collect(),
        None => Vec::new(),
    };
    (Sized { rows, width: bytes }, columns)
}

/// What `stats` keeps of `column`.
pub fn column(stats: &dyn Statistics, column: usize) -> ColumnStatistics {
    ColumnStatistics { distinct: or_unknown(stats.distinct(column)), nulls: or_unknown(stats.nulls(column)), bounds: stats.bounds(column) }
}

/// The share of rows `cond` keeps of `input`, by its table's statistics
/// if `input` is a base table's scan.
fn kept_share(st: &State, cond: &Cond, input: &LogicalPlan) -> f64 {
    let stats = if let LogicalPlan::Scan { table, .. } = input { st.statistics(table) } else { None };
    let cost = st.cost();
    selectivity(cond, Known { stats: stats.as_deref(), cost: &cost })
}

/// The estimate of `plan`, a node other than a scan, whose inputs are
/// estimated `inputs`.
pub fn of_node(st: &State, plan: &LogicalPlan, inputs: &[Sized]) -> Sized {
    let first = inputs.first().copied().unwrap_or(Sized::UNKNOWN);
    if !inputs.iter().all(|s| s.known()) {
        return Sized::UNKNOWN;
    }
    match plan {
        LogicalPlan::Filter { cond, input } => Sized { rows: first.rows * kept_share(st, cond, input), width: first.width },
        LogicalPlan::Product { .. } => {
            let second = inputs.get(1).copied().unwrap_or(Sized::UNKNOWN);
            Sized { rows: first.rows * second.rows, width: first.width + second.width }
        }
        LogicalPlan::Sort { .. } => first,
        LogicalPlan::Project { .. } => Sized { rows: first.rows, width: UNKNOWN },
        LogicalPlan::Limit { limit, .. } => {
            let most = limit.most().and_then(|n| u32::try_from(n).ok()).map_or(first.rows, f64::from);
            Sized { rows: first.rows.min(most), width: first.width }
        }
        LogicalPlan::Scan { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => Sized::UNKNOWN,
    }
}
