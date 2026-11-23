//! \[DESIGN\] A table's rows in a range of an index past the greatest
//! value its statistics saw, at most as many as the index estimates the
//! range to hold (RULES.md D891).
//!
//! A range past the greatest value a pass of the statistics saw has no
//! share in them. It keeps an equality's, or, of a column growing with
//! insertion, the share of every row written since the pass
//! (`stats/selectivity.rs`, RULES.md D381), which grows with each write
//! until the next pass. A stock level's lines of its district's last
//! twenty orders, or a delivery's oldest new orders once the oldest of the
//! pass are delivered, lie there. Every choice fed by the estimate swung
//! with it, the join's method (D890), its order, its hash table's size,
//! and whether a grouping over it runs on several threads. An index whose
//! first columns the conjuncts hold equal, and whose next they bound past
//! that value, estimates the range as its tree is now (`index/blink.rs`),
//! counting the entries of a small one. So the rows the conjuncts keep
//! are at most that estimate, which holds every row they keep. A range
//! the statistics saw is left to them, as they weigh the conjuncts the
//! index does not answer too. A plan made with slots reads each as the
//! value of the execution it is planned for, its bound a slot less a
//! constant folded, as the statistics read them (`estimate/valued.rs`).

use super::Estimates;
use crate::catalog::QualifiedName;
use crate::index::Probe;
use crate::logical::{Cond, Expr};
use crate::physical::estimate::filled;
use crate::stats::Statistics;
use crate::storage::cost_of;
use crate::value::{CompOp, Value, compare};

/// The bounds the conjuncts put on one column: each a comparison and its
/// value, the lower one above, the upper below.
#[derive(Default)]
struct Bounds {
    low: Option<(CompOp, Value)>,
    high: Option<(CompOp, Value)>,
}

impl Estimates<'_> {
    /// `share`, the share of `table`'s rows the conjuncts `conds`, of its
    /// columns, are estimated to keep, at most the share an index's
    /// estimate of a range they ask past the statistics' greatest value
    /// holds.
    pub(super) fn past_capped(&self, table: &QualifiedName, conds: &[Cond], share: f64) -> f64 {
        let (Some(stats), Some(all)) = (self.statistics(table), self.table_rows(table)) else { return share };
        let db = self.db();
        let valued: Vec<Cond> = conds.iter().map(|c| filled(c).unwrap_or_else(|| self.valued(c).into_owned())).collect();
        let mut least = share;
        for ix in db.catalog.indexes(table) {
            let Some(probe) = past_probe(&ix.columns, &valued, &*stats) else { continue };
            let Some(rows) = db.index_of(&ix.name).ok().and_then(|found| found.estimate(&probe)) else { continue };
            least = least.min(cost_of(rows) / all.max(1.0_f64));
        }
        least
    }
}

/// The probe of an index on `columns` that the conjuncts `conds` ask,
/// if they hold its first columns equal to constants and bound its next
/// from below past the greatest value `stats` saw of it.
fn past_probe(columns: &[usize], conds: &[Cond], stats: &dyn Statistics) -> Option<Probe> {
    let mut equal = Vec::new();
    let mut next = None;
    for &column in columns {
        if let Some(value) = conds.iter().find_map(|c| constant(c, column, CompOp::Eq)) {
            equal.push(value);
            continue;
        }
        next = Some(column);
        break;
    }
    let column = next.filter(|_| !equal.is_empty())?;
    let bounds = bounds_of(conds, column);
    let (op, low) = bounds.low?;
    let (_, greatest) = stats.bounds(column)?;
    if compare(&low, &greatest) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    Some(match bounds.high {
        Some(high) => Probe::Range { equal, low: Box::new((op, low)), high: Box::new(high) },
        None => Probe::Prefix { equal, then: Some((op, low)) },
    })
}

/// The bounds `conds` put on `column` by comparisons with constants, the
/// first of each side.
fn bounds_of(conds: &[Cond], column: usize) -> Bounds {
    let mut bounds = Bounds::default();
    for op in [CompOp::Gt, CompOp::Ge] {
        bounds.low = bounds.low.or_else(|| conds.iter().find_map(|c| constant(c, column, op)).map(|v| (op, v)));
    }
    for op in [CompOp::Lt, CompOp::Le] {
        bounds.high = bounds.high.or_else(|| conds.iter().find_map(|c| constant(c, column, op)).map(|v| (op, v)));
    }
    bounds
}

/// The constant `c` compares `column` with by `op`, written either way
/// round, if it is such a comparison and the constant not null.
fn constant(c: &Cond, column: usize, op: CompOp) -> Option<Value> {
    let Cond::Compare(left, written, right) = c else { return None };
    let (at, value, turned) = if let (Expr::Column(at), Expr::Const(v)) = (left, right) {
        (*at, v, *written)
    } else if let (Expr::Const(v), Expr::Column(at)) = (left, right) {
        (*at, v, written.flip())
    } else {
        return None;
    };
    (at == column && turned == op && !value.is_null()).then(|| value.clone())
}

// Unit tests: which conjuncts ask a range past the statistics, and its
// probe (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// Statistics whose every column's values lay from 1 to 3,000.
    #[derive(Debug)]
    struct Saw;

    impl Statistics for Saw {
        fn bounds(&self, _column: usize) -> Option<(Value, Value)> {
            Some((int(1), int(3_000)))
        }
    }

    fn compared(column: usize, op: CompOp, n: i128) -> Cond {
        Cond::Compare(Expr::Column(column), op, Expr::Const(int(n)))
    }

    /// Verifies: [DESIGN] RULES.md D891: conjuncts holding an index's first
    /// columns equal and bounding its next past the greatest value the
    /// statistics saw ask that range of it, a bound written either way
    /// round; a range within what they saw, one with no lower bound, and
    /// one of no equal column first ask none. MC/DC of the conjunct's
    /// three conditions: its column, its operator, and a value not null.
    #[test]
    fn a_range_past_the_statistics_is_probed_of_its_index() {
        let (key, saw) = ([0_usize, 1, 2], Saw);
        let ranged =
            [compared(0, CompOp::Eq, 1), compared(1, CompOp::Eq, 3), compared(2, CompOp::Ge, 5_710), compared(2, CompOp::Lt, 5_730)];
        let Some(Probe::Range { equal, low, high }) = past_probe(&key, &ranged, &saw) else { panic!("no range") };
        assert_eq!((equal.len(), low.0, high.0), (2, CompOp::Ge, CompOp::Lt));
        let turned = [compared(0, CompOp::Eq, 1), Cond::Compare(Expr::Const(int(5_710)), CompOp::Lt, Expr::Column(1))];
        let Some(Probe::Prefix { then: Some((op, _)), .. }) = past_probe(&key, &turned, &saw) else { panic!("no prefix") };
        assert_eq!(op, CompOp::Gt, "a bound written the other way round");
        let within = [compared(0, CompOp::Eq, 1), compared(1, CompOp::Ge, 2_000)];
        assert!(past_probe(&key, &within, &saw).is_none(), "within what the statistics saw");
        let below = [compared(0, CompOp::Eq, 1), compared(1, CompOp::Lt, 5_000)];
        assert!(past_probe(&key, &below, &saw).is_none(), "no lower bound");
        assert!(past_probe(&key, &[compared(1, CompOp::Ge, 5_000)], &saw).is_none(), "no equal column first");
        let null = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Null));
        assert!(constant(&null, 0, CompOp::Eq).is_none(), "a null");
        assert!(constant(&compared(0, CompOp::Eq, 1), 1, CompOp::Eq).is_none(), "another column");
        assert!(constant(&compared(0, CompOp::Eq, 1), 0, CompOp::Ge).is_none(), "another operator");
    }
}
