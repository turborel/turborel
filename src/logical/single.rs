//! \[DESIGN\] Whether a plan gives one row at most by its shape, which a
//! semi join of a scalar subquery's rows needs (RULES.md D773).

use super::LogicalPlan;
use super::raises::MAX_JUDGED_DEPTH;

impl LogicalPlan {
    /// Whether it gives one row at most by its shape: an ungrouped set
    /// function's row [X3.135-1986 §5.25 GR1], a FETCH FIRST of one row or
    /// none of ONLY, a VALUES of one, or such rows filtered, projected,
    /// sorted or made distinct.
    pub fn one_row_at_most(&self) -> bool {
        one_row(self, 0)
    }
}

/// `LogicalPlan::one_row_at_most` of `plan`, `depth` nodes down; past
/// `MAX_JUDGED_DEPTH` taken as not (RULES.md R-CF-6).
fn one_row(plan: &LogicalPlan, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Aggregate { groups, seeds, .. } => groups.is_empty() && seeds.len() <= 1,
        LogicalPlan::Limit { limit, input } => limit.most().is_some_and(|n| n <= 1) || one_row(input, deeper),
        LogicalPlan::Values { rows, .. } => rows.len() <= 1,
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Project { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Distinct { input } => one_row(input, deeper),
        // A row of each operand's at most, as one of each is [X3.135-1986
        // §5.20 GR2b].
        LogicalPlan::Product { left, right } => one_row(left, deeper) && one_row(right, deeper),
        LogicalPlan::Scan { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => false,
    }
}

// Unit tests: which plans give one row at most (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::{Expr, Fetch, Limit, Search};
    use crate::value::{DataType, Decimal, Spelled, Value};

    fn values(n: usize) -> LogicalPlan {
        LogicalPlan::Values {
            rows: vec![vec![Expr::Const(Value::Exact(Decimal::new(1, 0)))]; n],
            types: vec![DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric }],
        }
    }

    fn fetched(input: LogicalPlan, count: i128, ties: bool) -> LogicalPlan {
        let fetch = Fetch {
            count: Expr::Const(Value::Exact(Decimal::new(count, 0))),
            percent: false,
            ties: ties.then(Vec::new),
            search: Search::Unstated,
            hints: Box::default(),
        };
        LogicalPlan::Limit { limit: Limit { offset: None, fetch: Some(fetch) }, input: Box::new(input) }
    }

    fn aggregate(groups: Vec<usize>, seeds: usize) -> LogicalPlan {
        let input = Box::new(values(3));
        LogicalPlan::Aggregate { groups, collations: Vec::new(), aggs: Vec::new(), seeds: vec![Vec::new(); seeds], input }
    }

    /// Verifies: [X3.135-1986 §5.25 GR1]; [ISO/IEC 9075-2:2023 §7.17 GR7];
    /// [DESIGN] RULES.md D773: an ungrouped set function's row is one, and
    /// a grouping's or two empty grouping sets' are not; a FETCH FIRST of
    /// one row of ONLY is one, of two or WITH TIES not, unless its input
    /// is; a VALUES of one row is, of two not; a filter, projection, sort
    /// or DISTINCT keeps its input's judgment; a scan's rows are not.
    #[test]
    fn plans_of_one_row_at_most() {
        assert!(aggregate(Vec::new(), 0).one_row_at_most());
        assert!(aggregate(Vec::new(), 1).one_row_at_most());
        assert!(!aggregate(Vec::new(), 2).one_row_at_most(), "two empty grouping sets");
        assert!(!aggregate(vec![0], 0).one_row_at_most(), "grouped");
        assert!(fetched(values(3), 1, false).one_row_at_most());
        assert!(fetched(values(3), 0, false).one_row_at_most());
        assert!(!fetched(values(3), 2, false).one_row_at_most());
        assert!(!fetched(values(3), 1, true).one_row_at_most(), "WITH TIES");
        assert!(fetched(values(1), 5, false).one_row_at_most(), "of an input of one row");
        assert!(values(1).one_row_at_most() && values(0).one_row_at_most() && !values(2).one_row_at_most());
        assert!(LogicalPlan::Distinct { input: Box::new(values(1)) }.one_row_at_most());
        assert!(!LogicalPlan::Distinct { input: Box::new(values(2)) }.one_row_at_most());
        let product = |a: usize, b: usize| LogicalPlan::Product { left: Box::new(values(a)), right: Box::new(values(b)) };
        assert!(product(1, 1).one_row_at_most());
        assert!(!product(2, 1).one_row_at_most() && !product(1, 2).one_row_at_most(), "of an operand of more");
    }
}
