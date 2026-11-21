//! \[DESIGN\] The filter over a table access: the conjuncts the access
//! does not answer itself, and the rows they are estimated to keep, a
//! spatial probe's estimate standing for the windowed spatial conjuncts
//! (RULES.md D137).

use super::super::cardinality::at_least_one;
use super::super::cost::conjuncts::ranked;
use super::super::estimate::Known;
use super::super::shown::Shown;
use super::super::{Built, CostModel, Plan, TableRead};
use super::geometric;
use crate::logical::{Cond, and_all, judged_by_sets, render_cond};

/// A table without storage scanned anyway under a filter on `conds`, of
/// no rows, with the guesses `cost`.
pub(super) fn unstored(read: TableRead<'_>, conds: Vec<Cond>, cost: &CostModel) -> Built {
    let plan = Plan::SeqScan {
        table: read.table.clone(),
        columns: read.needed.to_vec(),
        rows: 0,
        cost: 0.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: read.located,
    };
    let known = Known { stats: None, cost };
    filtered(plan, conds, &Filtered { shown: read.shown, known, ordered: false, read: 0.0, most: 0.0, note: "", windowed: None })
}

/// What a filter over a table access is planned with.
pub(super) struct Filtered<'a> {
    /// How the filter is described (`physical/shown.rs`).
    pub(super) shown: Shown<'a>,
    /// The table's statistics, if its engine gives any, and the guesses.
    pub(super) known: Known<'a>,
    /// Whether the access yields rows in the order asked for.
    pub(super) ordered: bool,
    /// The row count used to estimate the fraction a filter keeps: the
    /// whole table for a scan or function lookup, else comparison matches.
    pub(super) read: f64,
    /// The most rows the filter may keep: those the access reads, a scan
    /// only those of the zones or chunks its conditions may match
    /// (`TableStorage::scan_share`).
    pub(super) most: f64,
    /// What EXPLAIN shows of a spatial probe priced against the scan
    /// (RULES.md D137), if any, after the condition.
    pub(super) note: &'a str,
    /// The share of the rows read the windowed spatial conjuncts keep, if
    /// a spatial probe was priced.
    pub(super) windowed: Option<f64>,
}

/// A table access `plan` under a filter for the conjuncts `conds` it does not
/// answer itself, estimated to keep the fraction of the rows that the
/// table's statistics give, no more than the access reads, its conjuncts
/// judged in the order of their ranks (`cost/conjuncts.rs`): a scan that
/// skips the rows its conditions cannot match keeps the same rows of the
/// fewer it reads, not a fraction of them again. Its long IN lists are
/// judged by sets (`logical::judged_by_sets`). Of no conjunct left, an
/// index scan deciding its conjunct of each entry (RULES.md D85) shows
/// the spatial note itself.
pub(super) fn filtered(plan: Plan, conds: Vec<Cond>, with: &Filtered<'_>) -> Built {
    let kept = geometric::kept(&conds, with.known, with.windowed);
    let plan = match and_all(ranked(conds, with.known)) {
        Some(c) => {
            // At least a row of a read of one, as the estimator's card is
            // (RULES.md D655): a share of a small table, an equality on 25
            // rows with no statistics yet, is otherwise a fraction of a
            // row, which a join above takes as none, its other operand
            // then free to probe or to read last however large.
            let est = at_least_one(with.read * kept, with.read).min(with.most);
            let c = judged_by_sets(c);
            let shown = with.shown.text(|names| render_cond(&c, names));
            let detail = if with.note.is_empty() { shown } else { format!("{shown}  {}", with.note) };
            Plan::Filter { detail, cond: c, est, of_read: true, input: Box::new(plan) }
        }
        None => noted(plan, with.note),
    };
    Built { plan, ordered: with.ordered }
}

/// `plan` shown with `note`, what EXPLAIN shows of the spatial probe
/// priced, after its detail, if it is an index scan and there is a note.
fn noted(mut plan: Plan, note: &str) -> Plan {
    if let Plan::IndexScan { detail, .. } = &mut plan
        && !note.is_empty()
    {
        detail.push_str("  ");
        detail.push_str(note);
    }
    plan
}

// Unit tests: a filter's estimate floored at a row of a read of one or
// more (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::logical::Expr;
    use crate::stats::StatsBuilder;
    use crate::value::{CompOp, Decimal, Value};

    /// The estimate of a filter of `cond` over a scan of `read` rows, of
    /// which it keeps at most `most`, by the statistics `stats`, if any.
    fn est(cond: Cond, (read, most): (f64, f64), stats: Option<&dyn crate::stats::Statistics>) -> f64 {
        let plan = Plan::SeqScan {
            table: QualifiedName { auth: "E".into(), ident: "T".into() },
            columns: vec![0],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        };
        let cost = CostModel::default();
        let known = Known { stats, cost: &cost };
        let with = Filtered { shown: Shown::of(None), known, ordered: false, read, most, note: "", windowed: None };
        let Plan::Filter { est, .. } = filtered(plan, vec![cond], &with).plan else { panic!("a filter") };
        est
    }

    fn equal(n: i128) -> Cond {
        Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(n, 0))))
    }

    /// Verifies: [DESIGN] RULES.md D655: a filter of a read of one row or
    /// more is estimated to keep at least one, as the estimator's card is,
    /// whether its share is a guess (an equality on 25 rows with no
    /// statistics, a quarter of a row) or the statistics say none (a value
    /// past the greatest); of a read of less than a row, no more than it
    /// reads; and never more than the access may keep. Regression test
    /// (R-TEST-7): the one nation of TPC-H's query 21 was estimated to
    /// keep no rows with no statistics, and its join put last. MC/DC of
    /// `at_least_one`'s floor: a share above a row kept as it is, one below
    /// raised, a read below a row its own floor.
    #[test]
    fn a_filter_of_rows_keeps_at_least_one() {
        assert!((est(equal(3), (25.0_f64, 25.0_f64), None) - 1.0_f64).abs() < 1e-9_f64, "a guessed quarter row");
        assert!((est(equal(3), (1_000.0_f64, 1_000.0_f64), None) - 10.0_f64).abs() < 1e-9_f64, "a share above a row");
        assert!((est(equal(3), (0.5_f64, 0.5_f64), None) - 0.5_f64).abs() < 1e-9_f64, "a read below a row");
        assert!(est(equal(3), (0.0_f64, 0.0_f64), None).abs() < 1e-9_f64, "of no rows none");
        assert!(est(equal(3), (25.0_f64, 0.0_f64), None).abs() < 1e-9_f64, "no more than the access keeps");
        let mut builder = StatsBuilder::new(1);
        for k in 0..25_i128 {
            builder.add(&vec![Value::Exact(Decimal::new(k, 0))], 4);
        }
        let stats = builder.finish();
        assert!((est(equal(99), (25.0_f64, 25.0_f64), Some(&stats)) - 1.0_f64).abs() < 1e-9_f64, "the statistics' none");
    }
}
