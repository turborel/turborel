//! \[DESIGN\] A table's rows in a range of an index between two bounds a
//! constant apart, at most the range's width times the rows of a value of
//! its column under the index's first columns (RULES.md D892).
//!
//! A plan made with slots and no values asks no range of its index, so
//! the cap of a range past the statistics (`past.rs`, D891) has nothing
//! to ask. A stock level's `OL_O_ID < ? AND OL_O_ID >= ? - 20` of one
//! slot still says the range holds twenty values of the column, whatever
//! the slot's value. The rows of a value under the index's first columns
//! held equal are the rows the pass saw over the distinct combinations
//! of those columns and it, as the pass counted them, so they do not grow
//! with the rows written since. Their product with the width bounds the
//! rows the conjuncts keep. Two integers bound a width as well: the
//! index's estimate of a range of more entries than it counts is its
//! share of the tree, coarse by some times for one of a few hundred. It
//! is asked only of an exact numeric column of scale 0 bounded by
//! integers, as only there a width counts values.

use super::Estimates;
use crate::catalog::QualifiedName;
use crate::logical::{Cond, Expr};
use crate::stats::Statistics;
use crate::value::{ArithOp, CompOp, DataType, Value};

/// A bound of a column by a slot plus an integer, or by an integer: the
/// comparison, the slot's place if a slot's, and the integer.
type SlotBound = (CompOp, Option<usize>, f64);

impl Estimates<'_> {
    /// `share`, the share of `table`'s rows the conjuncts `conds`, of its
    /// columns, are estimated to keep, at most the share a range of an
    /// index between two bounds a constant apart holds by its width.
    pub(super) fn width_capped(&self, table: &QualifiedName, conds: &[Cond], share: f64) -> f64 {
        let (Some(stats), Some(all)) = (self.statistics(table), self.table_rows(table)) else { return share };
        let db = self.db();
        let Some(desc) = db.catalog.table(table) else { return share };
        let integral = |column: usize| desc.columns.get(column).is_some_and(|c| matches!(c.data_type, DataType::Exact { scale: 0, .. }));
        let mut least = share;
        for ix in db.catalog.indexes(table) {
            let Some((held, column, width)) = slot_range(&ix.columns, conds) else { continue };
            let Some(rows) = per_value(&*stats, &held, column).filter(|_| integral(column)) else { continue };
            least = least.min(width * rows / all.max(1.0_f64));
        }
        least
    }
}

/// The index's first columns `conds` hold equal, the next column, and
/// the values between its two bounds of one slot, if they bound it so.
fn slot_range(columns: &[usize], conds: &[Cond]) -> Option<(Vec<usize>, usize, f64)> {
    let held: Vec<usize> = columns.iter().copied().take_while(|&c| conds.iter().any(|cond| equal(cond, c))).collect();
    let column = *columns.get(held.len())?;
    if held.is_empty() {
        return None;
    }
    let low = conds.iter().find_map(|c| slot_bound(c, column).filter(|b| matches!(b.0, CompOp::Gt | CompOp::Ge)))?;
    let high = conds.iter().find_map(|c| slot_bound(c, column).filter(|b| matches!(b.0, CompOp::Lt | CompOp::Le)))?;
    if low.1 != high.1 {
        return None;
    }
    let open = |op: CompOp, edge: CompOp| if op == edge { 1.0_f64 } else { 0.0_f64 };
    let width = high.2 - low.2 - open(low.0, CompOp::Gt) + open(high.0, CompOp::Le);
    Some((held, column, width.max(1.0_f64)))
}

/// The rows of one value of `column` under the columns `held`, as the
/// pass saw them: its rows over the distinct combinations of them all.
fn per_value(stats: &dyn Statistics, held: &[usize], column: usize) -> Option<f64> {
    let rows = stats.rows()?;
    let combinations = held.iter().chain(std::iter::once(&column)).try_fold(1.0_f64, |n, &c| stats.distinct(c).map(|d| n * d))?;
    Some(rows / combinations.clamp(1.0_f64, rows.max(1.0_f64)))
}

/// Whether `c` holds `column` equal to a constant or a slot.
fn equal(c: &Cond, column: usize) -> bool {
    compared(c, column).is_some_and(|(op, other)| op == CompOp::Eq && matches!(other, Expr::Const(_) | Expr::Dynamic(_)))
}

/// The bound `c` puts on `column` by a slot plus or less an integer, the
/// slot alone, or an integer, written either way round.
fn slot_bound(c: &Cond, column: usize) -> Option<SlotBound> {
    let (op, other) = compared(c, column)?;
    if let Expr::Dynamic(slot) = other {
        return Some((op, Some(slot.index), 0.0_f64));
    }
    if let Expr::Const(Value::Exact(n)) = other {
        let n = n.to_f64();
        return (n.fract().abs() < f64::EPSILON).then_some((op, None, n));
    }
    let Expr::Arith(left, by, right) = other else { return None };
    let (Expr::Dynamic(slot), Expr::Const(Value::Exact(n))) = (&**left, &**right) else { return None };
    let n = n.to_f64();
    let signed = match by {
        ArithOp::Add => n,
        ArithOp::Sub => -n,
        ArithOp::Mul | ArithOp::Div => return None,
    };
    (n.fract().abs() < f64::EPSILON).then_some((op, Some(slot.index), signed))
}

/// The comparison `c` makes of `column` and the other side, turned so
/// the column is on the left, if `c` compares the column.
fn compared(c: &Cond, column: usize) -> Option<(CompOp, &Expr)> {
    let Cond::Compare(left, op, right) = c else { return None };
    if let Expr::Column(at) = left
        && *at == column
    {
        return Some((*op, right));
    }
    let Expr::Column(at) = right else { return None };
    (*at == column).then_some((op.flip(), left))
}

// Unit tests: which conjuncts bound a range by one slot, its width, and
// a value's rows (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::Slot;
    use crate::value::{Decimal, Spelled};

    fn int(n: i128) -> Expr {
        Expr::Const(Value::Exact(Decimal::new(n, 0)))
    }

    fn slot(index: usize) -> Expr {
        Expr::Dynamic(Slot { index, data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, at: 0 })
    }

    fn by(index: usize, op: ArithOp, n: i128) -> Expr {
        Expr::Arith(Box::new(slot(index)), op, Box::new(int(n)))
    }

    fn compare(column: usize, op: CompOp, e: Expr) -> Cond {
        Cond::Compare(Expr::Column(column), op, e)
    }

    /// Statistics of 3,000 rows, 10 values of column 0 and 300 of 1.
    #[derive(Debug)]
    struct Saw;

    impl Statistics for Saw {
        fn rows(&self) -> Option<f64> {
            Some(3_000.0_f64)
        }
        fn distinct(&self, column: usize) -> Option<f64> {
            [10.0_f64, 300.0_f64].get(column).copied()
        }
    }

    /// Verifies: [DESIGN] RULES.md D892: a range between a slot less a
    /// constant and the slot, or the slot and it plus one, written either
    /// way round, its first column held equal to a constant or a slot,
    /// holds the values between its bounds, an inclusive bound one more
    /// and an exclusive lower one less; two slots, a product, a missing
    /// bound and no column held equal give none; two integers give their
    /// width, an integer and a slot none. A value's rows are the pass's
    /// rows over its columns' combinations.
    #[test]
    fn a_range_of_one_slot_is_as_wide_as_its_bounds() {
        let key = [0_usize, 1];
        let width = |conds: &[Cond]| slot_range(&key, conds).map(|(_, _, w)| w);
        let held = compare(0, CompOp::Eq, int(3));
        let ranged = [held.clone(), compare(1, CompOp::Ge, by(0, ArithOp::Sub, 20)), compare(1, CompOp::Lt, slot(0))];
        assert_eq!(slot_range(&key, &ranged), Some((vec![0], 1, 20.0_f64)));
        let turned = [
            compare(0, CompOp::Eq, slot(1)),
            Cond::Compare(slot(0), CompOp::Lt, Expr::Column(1)),
            compare(1, CompOp::Le, by(0, ArithOp::Add, 5)),
        ];
        assert_eq!(width(&turned), Some(5.0_f64), "open below, closed above, held by a slot");
        let closed = [held.clone(), compare(1, CompOp::Ge, slot(0)), compare(1, CompOp::Le, by(0, ArithOp::Add, 5))];
        assert_eq!(width(&closed), Some(6.0_f64), "closed at both");
        let two = [held.clone(), compare(1, CompOp::Ge, slot(1)), compare(1, CompOp::Lt, slot(0))];
        assert_eq!(width(&two), None, "two slots");
        let product = [held.clone(), compare(1, CompOp::Ge, by(0, ArithOp::Mul, 2)), compare(1, CompOp::Lt, slot(0))];
        assert_eq!(width(&product), None, "a product");
        assert_eq!(width(&[held.clone(), compare(1, CompOp::Lt, slot(0))]), None, "no lower bound");
        assert_eq!(width(&[compare(1, CompOp::Ge, slot(0)), compare(1, CompOp::Lt, by(0, ArithOp::Add, 3))]), None, "none held");
        let constants = [held.clone(), compare(1, CompOp::Ge, int(6_081)), compare(1, CompOp::Lt, int(6_101))];
        assert_eq!(width(&constants), Some(20.0_f64), "two integers");
        let mixed = [held, compare(1, CompOp::Ge, int(6_081)), compare(1, CompOp::Lt, slot(0))];
        assert_eq!(width(&mixed), None, "an integer and a slot");
        assert_eq!(per_value(&Saw, &[0], 1), Some(1.0_f64));
    }
}
