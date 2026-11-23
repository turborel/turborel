//! \[DESIGN\] The rows a join's left operand gives, as the probes of an
//! index join and the hash join they replace are weighed (RULES.md D890),
//! taken from the index range its plan reads where that is estimated at
//! fewer rows than the statistics estimate.
//!
//! A range past the greatest value a pass of the statistics saw, of a
//! column growing with insertion, keeps the share of the rows written
//! since (`stats/selectivity.rs`, RULES.md D381). STOCK LEVEL's lines of
//! a district's last twenty orders lie past it as soon as orders are
//! written, so between two passes their estimate grew with every order,
//! to many times their rows, and faster as more terminals wrote at once.
//! Times PLANNER.ESTIMATE_MARGIN, its probes of STOCK then cost more than
//! a hash join reading all of STOCK, which each run then read, a
//! hundredfold the rows of the probes. The index of the range has no
//! such lag, as it counts the entries of a small range and takes a larger
//! one's share of its tree as it is now (`index/blink.rs`). A filter
//! over the range keeps no more rows than the range holds, so the left's
//! rows are the lesser of the two estimates, and the margin still
//! weighs the probes against their rows being some times more.

use crate::physical::Plan;
use crate::physical::access::keyed;

/// The rows `estimated` of a join's left operand, or fewer where its
/// plan, `built`, reads a range of an index estimated at fewer, under
/// filters and projections.
pub(super) fn left_rows(estimated: f64, built: Option<&Plan>) -> f64 {
    built.and_then(ranged).map_or(estimated, |rows| estimated.min(rows))
}

/// The rows the index range `plan` reads, under filters and projections,
/// is estimated at, if it reads one.
fn ranged(plan: &Plan) -> Option<f64> {
    let mut at = plan;
    while let Plan::Filter { input, .. } | Plan::Project { input, .. } = at {
        at = input;
    }
    let Plan::IndexScan { probe, est, .. } = at else { return None };
    keyed(probe).then_some(*est)
}

// Unit tests: which plans give their range's estimate, and the lesser
// taken (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::index::Probe;
    use crate::logical::{Cond, Expr};
    use crate::value::{CompOp, Decimal, Value};

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// An index scan by `probe`, estimated at `est` rows.
    fn scan(probe: Probe, est: f64) -> Plan {
        Plan::IndexScan {
            table: QualifiedName { auth: "X".into(), ident: "T".into() },
            index: "I".into(),
            method: crate::index::BTREE_METHOD.into(),
            probe,
            slots: Vec::new(),
            reverse: false,
            columns: vec![0],
            detail: String::new(),
            est,
            cost: 1.0_f64,
            covering: false,
            located: false,
            decided: None,
        }
    }

    /// `input` under a filter of the statistics' estimate `est`.
    fn filtered(input: Plan, est: f64) -> Plan {
        let cond = Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Const(int(9)));
        Plan::Filter { cond, detail: String::new(), est, of_read: true, input: Box::new(input) }
    }

    /// Verifies: [DESIGN] RULES.md D890: a left operand reading a range of
    /// an index under a filter is estimated at the range's rows where they
    /// are fewer than the statistics', and at the statistics' where those
    /// are fewer; one reading every entry, or with no plan, at the
    /// statistics'. MC/DC of `keyed(probe)`: a range taken, a whole index
    /// not.
    #[test]
    fn a_left_reading_an_index_range_is_estimated_at_its_rows_if_fewer() {
        let range = || Probe::Prefix { equal: vec![int(1), int(3)], then: Some((CompOp::Ge, int(5_710))) };
        let left = filtered(scan(range(), 195.0_f64), 195.0_f64);
        assert!((left_rows(1_515.0_f64, Some(&left)) - 195.0_f64).abs() < f64::EPSILON, "the range's fewer rows");
        assert!((left_rows(120.0_f64, Some(&left)) - 120.0_f64).abs() < f64::EPSILON, "the statistics' fewer rows");
        let whole = filtered(scan(Probe::All, 195.0_f64), 195.0_f64);
        assert!((left_rows(1_515.0_f64, Some(&whole)) - 1_515.0_f64).abs() < f64::EPSILON, "no range");
        assert!((left_rows(1_515.0_f64, None) - 1_515.0_f64).abs() < f64::EPSILON, "no plan");
    }
}
