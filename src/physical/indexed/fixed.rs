//! \[DESIGN\] A join's probe of the inner table's index whose first columns
//! the inner table's own filter fixes (RULES.md D761). A join of
//! ORDER_LINE and STOCK on `S_I_ID = OL_I_ID`, STOCK filtered by
//! `S_W_ID = ?`, probes STOCK's key (S_W_ID, S_I_ID) only if S_W_ID's
//! value comes with each left row. A filter's equality with a constant
//! gets it by transitivity, as the left is filtered by the same constant,
//! but a slot of its own does not, nor a constant the left is not
//! filtered by. So the value the filter fixes is made a column of the
//! left operand, equated with the inner table's column, and a projection
//! takes it away after: the join's keys then give each column of the
//! index's prefix, and it is planned as any. The plan made with values
//! takes the same way, so a plan with slots keeps one shape with it
//! (D45).
//!
//! A column is so fixed only where it makes a prefix of an index with the
//! join's keys: each column of the prefix fixed or keyed, at least one of
//! each, or fixed alone where the join bounds the index's next column by
//! the left's values, as STOCK-LEVEL's lines are bounded by the district's
//! next order (RULES.md D776). A column the left already gives as a
//! constant or slot of its own is not fixed again, so the plan made so is
//! not rewritten twice.

use super::read_table;
use crate::logical::{Cond, Expr, LogicalPlan, and_all, expr_columns, map_cond_columns, split_and};
use crate::physical::PhysicalPlanner;
use crate::physical::join::equated;
use crate::value::CompOp;

/// A join whose inner table's fixed columns are columns of its left
/// operand too.
pub(in crate::physical) struct Fixed {
    /// Its condition, over the operands below.
    pub(in crate::physical) cond: Cond,
    /// Its left operand, the values fixed after its columns.
    pub(in crate::physical) left: LogicalPlan,
    /// What gives the join's own columns of its pairs.
    pub(in crate::physical) kept: Vec<Expr>,
}

/// `c`, a column equal to a constant or a slot: the column and the value.
fn fixing(c: &Cond) -> Option<(usize, Expr)> {
    let Cond::Compare(a, CompOp::Eq, b) = c else { return None };
    let ((Expr::Column(column), value) | (value, Expr::Column(column))) = (a, b) else { return None };
    matches!(value, Expr::Const(_) | Expr::Dynamic(_)).then(|| (*column, value.clone()))
}

/// The right's columns a join equates, those of them the left gives as
/// constants or slots, and those it compares by order with the left's
/// values.
type Columns<'a> = (&'a [usize], &'a [usize], &'a [usize]);

/// Of `c`, a comparison by order of a column of the right, past the
/// left's `width`, with a value of the left's columns alone, the right's
/// column.
fn ordered_with(c: &Cond, width: usize) -> Option<usize> {
    let Cond::Compare(a, op, b) = c else { return None };
    let ((Expr::Column(r), value) | (value, Expr::Column(r))) = (a, b) else { return None };
    let mut read = Vec::new();
    expr_columns(value, &mut read);
    let of_left = !read.is_empty() && read.iter().all(|&l| l < width);
    (of_left && *r >= width && !matches!(op, CompOp::Eq | CompOp::Ne)).then(|| r.saturating_sub(width))
}

/// Whether column `l` of `left` is a constant or slot its projection gives.
fn given(left: &LogicalPlan, l: usize) -> bool {
    let LogicalPlan::Project { exprs, .. } = left else { return false };
    matches!(exprs.get(l), Some(Expr::Const(_) | Expr::Dynamic(_)))
}

/// Of `ix_columns`, an index's, the fixes among `fixes` that make its
/// prefix with the right columns `keyed` the join equates, each column of
/// the prefix one or the other, `given` ones as keyed; none unless at
/// least one of each, or the column after the prefix is one of `ordered`,
/// those the join compares by order with the left's values.
fn prefix_fixes(ix_columns: &[usize], fixes: &[(usize, Expr)], (keyed, given, ordered): Columns<'_>) -> Option<Vec<(usize, Expr)>> {
    let (mut used, mut by_key) = (Vec::new(), false);
    for column in ix_columns {
        if given.contains(column) {
            by_key = true;
        } else if let Some(fix) = fixes.iter().find(|(c, _)| c == column) {
            used.push(fix.clone());
        } else if keyed.contains(column) {
            by_key = true;
        } else {
            by_key = by_key || ordered.contains(column);
            break;
        }
    }
    (by_key && !used.is_empty()).then_some(used)
}

impl PhysicalPlanner<'_> {
    /// The join of `left` and the table `right` reads on `cond` as `Fixed`,
    /// if the table's filter fixes columns that make an index's prefix
    /// with the columns `cond` equates, as the module says.
    pub(in crate::physical) fn fixed_prefix(&self, cond: &Cond, left: &LogicalPlan, right: &LogicalPlan) -> Option<Fixed> {
        let read = read_table(right).filter(|r| r.through.is_none())?;
        let mut filtered = Vec::new();
        split_and(read.filter?.clone(), &mut filtered);
        let fixes: Vec<(usize, Expr)> = filtered.iter().filter_map(fixing).collect();
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let keys: Vec<_> = conjuncts.iter().filter_map(|c| equated(c, width)).filter(|k| !k.nulls).collect();
        let keyed: Vec<usize> = keys.iter().map(|k| k.right).collect();
        let given: Vec<usize> = keys.iter().filter(|k| given(left, k.left)).map(|k| k.right).collect();
        let ordered: Vec<usize> = conjuncts.iter().filter_map(|c| ordered_with(c, width)).collect();
        let building = |name: &_| self.db.building.contains(name);
        let mut indexes = self.db.catalog.indexes(read.table).into_iter().filter(|ix| !building(&ix.name));
        let used = indexes.find_map(|ix| prefix_fixes(&ix.columns, &fixes, (&keyed, &given, &ordered)))?;
        fixed(conjuncts, (left, right.width()), used)
    }
}

/// The join of `left` and a right operand `right_width` columns wide on
/// `conjuncts`, the values of `used` made columns of the left, each
/// equated with the right's column it fixes.
fn fixed(conjuncts: Vec<Cond>, (left, right_width): (&LogicalPlan, usize), used: Vec<(usize, Expr)>) -> Option<Fixed> {
    let (width, count) = (left.width(), used.len());
    let wider = width.saturating_add(count);
    let moved = move |c: usize| if c < width { c } else { c.saturating_add(count) };
    let equal = used.iter().enumerate().map(|(k, (column, _))| {
        Cond::Compare(Expr::Column(width.saturating_add(k)), CompOp::Eq, Expr::Column(wider.saturating_add(*column)))
    });
    let conds: Vec<Cond> = equal.chain(conjuncts.into_iter().map(|c| map_cond_columns(c, &moved))).collect();
    let exprs = (0..width).map(Expr::Column).chain(used.into_iter().map(|(_, value)| value)).collect();
    let kept = (0..width).chain(wider..wider.saturating_add(right_width)).map(Expr::Column).collect();
    Some(Fixed { cond: and_all(conds)?, left: LogicalPlan::Project { exprs, input: Box::new(left.clone()) }, kept })
}
