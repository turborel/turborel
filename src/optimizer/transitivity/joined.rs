//! \[DESIGN\] Columns of two tables equated by one value (RULES.md D404):
//! where a filter over a product holds `a = v` and `b = v`, `a` and `b`
//! of different tables of it and `v` one exact number or one `?`, it
//! holds `a = b` as well, so the join may probe an index of `b`'s table
//! by `a`'s value, as it may by any other equality of its columns.

use crate::logical::{Cond, Expr, LogicalPlan};
use crate::value::{CompOp, DataType, NormKey, Value};

/// The value two equalities may equate their columns with: an exact
/// number, in its form under which identical numbers are equal, or the
/// place of a `?` of exact numeric type, which has one value in an
/// execution.
#[derive(PartialEq)]
enum Held {
    Number(NormKey),
    Slot(usize),
}

/// The equalities of two columns of different tables of `input` the
/// conjuncts' equalities with one value imply, but those `equated`
/// holds already. A column compared with an exact number is of a numeric
/// type [X3.135-1986 §5.11 SR2], so two such columns compare without
/// error, and the equality added keeps the rows the two it is of keep.
pub(super) fn equated_by_values(conjuncts: &[Cond], equated: &[(usize, usize)], input: &LogicalPlan) -> Vec<Cond> {
    let LogicalPlan::Product { .. } = input else { return Vec::new() };
    let mut ends = Vec::new();
    table_ends(input, 0, &mut ends);
    let held: Vec<(usize, Held)> = conjuncts.iter().filter_map(held).collect();
    let mut known: Vec<(usize, usize)> = equated.to_vec();
    let mut added = Vec::new();
    for (i, (a, value)) in held.iter().enumerate() {
        for (b, other) in held.iter().skip(i.saturating_add(1)) {
            let apart = table_of(&ends, *a) != table_of(&ends, *b);
            let new = !known.iter().any(|&(x, y)| (x, y) == (*a, *b) || (y, x) == (*a, *b));
            if value == other && apart && new {
                known.push((*a, *b));
                added.push(Cond::Compare(Expr::Column(*a), CompOp::Eq, Expr::Column(*b)));
            }
        }
    }
    added
}

/// The column "column = value" compares, and the value, if it is one an
/// equality of two columns may be derived of (`Held`).
fn held(c: &Cond) -> Option<(usize, Held)> {
    let Cond::Compare(left, CompOp::Eq, right) = c else { return None };
    let (column, value) = if let Expr::Column(at) = left { (*at, right) } else { (column_of(right)?, left) };
    if let Expr::Const(v @ Value::Exact(_)) = value {
        return Some((column, Held::Number(v.norm())));
    }
    let Expr::Dynamic(slot) = value else { return None };
    matches!(slot.data_type, DataType::Exact { .. }).then_some((column, Held::Slot(slot.index)))
}

/// The column `e` is, if one.
fn column_of(e: &Expr) -> Option<usize> {
    if let Expr::Column(at) = e { Some(*at) } else { None }
}

/// The first column past each table a product of `plan`, from column
/// `from`, reads, into `ends`, in order.
fn table_ends(plan: &LogicalPlan, from: usize, ends: &mut Vec<usize>) {
    if let LogicalPlan::Product { left, right } = plan {
        table_ends(left, from, ends);
        table_ends(right, from.saturating_add(left.width()), ends);
        return;
    }
    ends.push(from.saturating_add(plan.width()));
}

/// Which of the tables ending at `ends` the column `at` is of.
fn table_of(ends: &[usize], at: usize) -> usize {
    ends.iter().position(|&end| at < end).unwrap_or(ends.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical::Slot;
    use crate::value::{CharUnits, Spelled};

    /// Verifies: [DESIGN] RULES.md D404, a column equal to a `?` of exact
    /// numeric type is held by the `?`'s place, written either way round;
    /// one of a character type, and one compared otherwise, are not.
    #[test]
    fn a_column_equal_to_an_exact_parameter_is_held_by_its_place() {
        let exact = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let text = DataType::Varying { length: 4, units: CharUnits::Characters };
        let slot = |data_type: &DataType| Expr::Dynamic(Slot { index: 1, data_type: data_type.clone(), at: 0 });
        let held_by = |c: Cond| held(&c).map(|(column, h)| (column, h == Held::Slot(1)));
        assert_eq!(held_by(Cond::Compare(Expr::Column(2), CompOp::Eq, slot(&exact))), Some((2, true)));
        assert_eq!(held_by(Cond::Compare(slot(&exact), CompOp::Eq, Expr::Column(3))), Some((3, true)));
        assert_eq!(held_by(Cond::Compare(Expr::Column(2), CompOp::Eq, slot(&text))), None, "of a character type");
        assert_eq!(held_by(Cond::Compare(Expr::Column(2), CompOp::Lt, slot(&exact))), None, "not an equality");
    }
}
