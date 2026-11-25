//! \[DESIGN\] An index join's probe bounding the key column after its
//! equalities by values of the outer row (RULES.md D776). STOCK-LEVEL's
//! join of DISTRICT and ORDER_LINE equates the warehouse and district
//! and keeps the lines of `OL_O_ID < D_NEXT_O_ID AND OL_O_ID >=
//! D_NEXT_O_ID - 20`, twenty orders' lines of the district's thousands. A
//! probe of the equalities alone read every line of the district, each
//! then judged by the bounds. Where the key's next column, after those
//! the equalities give, is compared by order with an outer column, or a
//! sum or difference of one and a constant that raises nothing by the
//! declared types (`logical/raises.rs`), the probe reads the keys between
//! the bounds alone, as `JoinProbe::Range` binds them of each outer row
//! (`physical/join_probe.rs`).
//!
//! The bounds are taken only of columns of exact numbers or datetimes, of
//! the outer value's kind, whose comparison the index's order answers
//! exactly [X3.135-1986 §5.11 GR2, GR5], so the conjuncts they answer are
//! judged of no pair again. A null bound matches no key, as its
//! comparison is unknown [GR1]. Each bound keeps the default share of a
//! range of the rows a probe of the equalities finds, which prices the
//! probes against the read they replace.

use super::Probing;
use crate::catalog::{ColumnDesc, IndexDesc, QualifiedName};
use crate::index::IndexMethod;
use crate::logical::{Cond, Expr, LogicalPlan, can_raise_typed, map_expr_columns, split_and};
use crate::physical::join::{Flip, JoinKey, equated};
use crate::physical::{OuterBound, PhysicalPlanner};
use crate::value::{ArithOp, CompOp, DataType, Value};

/// The declared type of a column of the outer rows, if known.
pub(in crate::physical) type TypeAt<'a> = &'a dyn Fn(usize) -> Option<DataType>;

/// Of index `ix` on `columns`, of a method of `kind`, the table column
/// after its first `used`, and its type, if the method answers a probe of
/// those and bounds of it, the index is not of sort keys, and the column
/// is of a kind whose order a bound is taken of.
fn next(ix: &IndexDesc, (columns, used): (&[ColumnDesc], usize), kind: &dyn IndexMethod) -> Option<(usize, DataType)> {
    let width = ix.columns.len();
    let column = *ix.columns.get(used)?;
    let data_type = columns.get(column)?.data_type.clone();
    let answers = kind.supports_range(width, used) && kind.supports_prefix(width, used, Some(CompOp::Lt));
    (answers && !ix.is_collated() && ordered_kind(&data_type)).then_some((column, data_type))
}

/// Whether a bound of a column of type `t` is taken: of an exact number
/// or a datetime.
fn ordered_kind(t: &DataType) -> bool {
    match t {
        DataType::Exact { .. } | DataType::Date | DataType::Timestamp { .. } => true,
        DataType::Distinct(d) => ordered_kind(&d.source),
        DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Time { .. }
        | DataType::Interval(_)
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => false,
    }
}

impl PhysicalPlanner<'_> {
    /// `probing`, a probe of a join's equalities whose left operand is
    /// `width` columns wide, its outer columns of the types `type_at`
    /// gives, bounding the key's next column by the conjuncts of `rest`,
    /// over the join's columns, that compare it with an outer value, a
    /// lower bound and an upper at most, its rows and cost made so; and the
    /// conjuncts of `rest` left.
    pub(in crate::physical) fn bounded(
        &self,
        mut probing: Probing,
        rest: Vec<Cond>,
        (width, type_at): (usize, TypeAt<'_>),
    ) -> (Probing, Vec<Cond>) {
        let Some((column, data_type)) = self.next_bounded(&probing) else { return (probing, rest) };
        let fetch = self.db.storage_of(&probing.table).map_or(1.0_f64, |s| s.fetch_cost(probing.needed.len()));
        let of_table = |c: usize| {
            let at = c.checked_sub(width)?;
            probing.through.as_ref().map_or(Some(at), |(columns, _)| columns.get(at).copied())
        };
        let (mut low, mut high, mut left) = (None, None, Vec::new());
        for c in rest {
            let found = bound_of(&c, width, &|r| of_table(r) == Some(column)).filter(|(b, e)| outer_fits((b, e), &data_type, type_at));
            let Some((b, _)) = found else {
                left.push(c);
                continue;
            };
            let place = if matches!(b.op, CompOp::Gt | CompOp::Ge) { &mut low } else { &mut high };
            if place.is_none() {
                *place = Some((b, c));
            } else {
                left.push(c);
            }
        }
        let taken: Vec<(OuterBound, Cond)> = low.into_iter().chain(high).collect();
        let share = self.kept_share(&probing.table, column, &taken);
        let each = probing.each * share;
        probing.cost = (probing.cost - (probing.each - each) * fetch).max(0.0_f64);
        probing.each = each;
        (probing.bounds, probing.bounded) = taken.into_iter().unzip();
        (probing, left)
    }

    /// The cost of a join of `left` and `right` on `cond` whose left rows
    /// each probe an index of the table `right` reads by the equalities of
    /// `cond` and bounds of the key's next column, if it may be one; or of
    /// the right's rows probing the left's table, the cheaper. A range join
    /// is weighed against it (`physical/range/planned.rs`).
    pub(in crate::physical) fn bounded_join_cost(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan)) -> Option<f64> {
        let flipped = Flip { width: left.width(), right_width: right.width() }.cond(cond.clone());
        let normal = self.bounded_probes(cond, (left, right));
        let swapped = self.bounded_probes(&flipped, (right, left));
        [normal, swapped].into_iter().flatten().reduce(f64::min)
    }

    /// The cost of `outer`'s rows each probing an index of the table
    /// `inner` reads by the equalities of `cond` and bounds, if it has an
    /// index the equalities give a prefix of and a bound is taken; the
    /// columns `inner`'s own filter fixes taken as equalities too, as the
    /// join made of them would be (`indexed/fixed.rs`). It is net of the
    /// inner table's read the probes replace.
    fn bounded_probes(&self, cond: &Cond, (outer, inner): (&LogicalPlan, &LogicalPlan)) -> Option<f64> {
        let fixed = self.fixed_prefix(cond, outer, inner);
        let (cond, outer) = fixed.as_ref().map_or((cond, outer), |f| (&f.cond, &f.left));
        let width = outer.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut pairs, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            match equated(&c, width) {
                Some(JoinKey { left, right, nulls: false }) => pairs.push((left, right)),
                Some(JoinKey { nulls: true, .. }) | None => rest.push(c),
            }
        }
        let type_at = |c: usize| self.column_type(outer, c);
        let of_kind = |c: usize, t: &DataType| self.column_type(outer, c).is_some_and(|ct| super::alike(&ct, t));
        let (probing, _) = self.bounded(self.probing(inner, &pairs, &of_kind)?, rest, (width, &type_at));
        // The probes replace the inner table's read, which the range join
        // they are weighed against prices apart from its search.
        let read = super::access_cost(&self.whole().build_node(inner, &[]).plan).unwrap_or(0.0_f64);
        (!probing.bounds.is_empty()).then(|| (self.rows_of(outer) * probing.cost - read).max(0.0_f64))
    }

    /// The share of a probe's rows the bounds `taken` of `table`'s column
    /// `column` keep: of a lower and an upper bound of one outer column, as
    /// `D_NEXT_O_ID - 20` and `D_NEXT_O_ID` are, their constants'
    /// difference over the column's distinct values, as its values are
    /// spread evenly; else the default share of a range for each bound.
    fn kept_share(&self, table: &QualifiedName, column: usize, taken: &[(OuterBound, Cond)]) -> f64 {
        let default = self.weights().range_selectivity.powi(i32::try_from(taken.len()).unwrap_or(i32::MAX));
        let [(low, _), (high, _)] = taken else { return default };
        let distinct = self.statistics(table).and_then(|s| s.distinct(column)).filter(|_| low.outer == high.outer);
        let (Some(distinct), Some(from), Some(to)) = (distinct, offset(low), offset(high)) else { return default };
        ((to - from).max(1.0_f64) / distinct.max(1.0_f64)).min(1.0_f64)
    }

    /// Of the index `probing` reads in its key's order, the key column
    /// after those its equalities give, and its type, as `next` says.
    fn next_bounded(&self, probing: &Probing) -> Option<(usize, DataType)> {
        let ix = self.db.catalog.index(&probing.index)?;
        let kind = self.db.methods.get(&ix.method)?;
        let desc = self.db.catalog.table(&probing.table)?;
        next(ix, (&desc.columns, probing.used.len()), &**kind).filter(|_| probing.positions.is_none())
    }
}

/// Whether the outer value `expr` of the bound `bound` is of the kind of
/// the key column's type `data_type`, as `type_at` gives the outer
/// columns' types, and raises nothing.
fn outer_fits((bound, expr): (&OuterBound, &Expr), data_type: &DataType, type_at: TypeAt<'_>) -> bool {
    let of_kind = type_at(bound.outer).is_some_and(|t| super::alike(&t, data_type) && ordered_kind(&t));
    of_kind && !can_raise_typed(expr, type_at)
}

/// Of `cond`, a comparison by order of a column the join's right operand
/// gives, which `keyed` says is the key's next, with an outer value, the
/// bound it makes and the outer value's expression.
fn bound_of(cond: &Cond, width: usize, keyed: &dyn Fn(usize) -> bool) -> Option<(OuterBound, Expr)> {
    let Cond::Compare(first, op, second) = cond else { return None };
    let (op, e) = match (first, second) {
        (Expr::Column(key), value) => (keyed(*key).then_some(*op)?, value),
        (value, Expr::Column(key)) => (keyed(*key).then(|| op.flip())?, value),
        _ => return None,
    };
    if op == CompOp::Eq || op == CompOp::Ne {
        return None;
    }
    let (outer, shift) = outer_value(e, width)?;
    Some((OuterBound { op, outer, shift }, e.clone()))
}

/// Of `e`, an outer column of the `width` the left gives, or a sum or
/// difference of one and a constant: the column and, of the latter, the
/// expression over its value alone, as column 0.
fn outer_value(e: &Expr, width: usize) -> Option<(usize, Option<Box<Expr>>)> {
    let outer = |c: usize| (c < width).then_some(c);
    if let Expr::Column(c) = e {
        return Some((outer(*c)?, None));
    }
    let Expr::Arith(x, ArithOp::Add | ArithOp::Sub, y) = e else { return None };
    let column = match (&**x, &**y) {
        (Expr::Column(c), Expr::Const(_)) | (Expr::Const(_), Expr::Column(c)) => outer(*c)?,
        _ => return None,
    };
    Some((column, Some(Box::new(map_expr_columns(e.clone(), &|_| 0)))))
}

/// The constant the bound `b` adds to its outer column: none of a column
/// alone, of a sum the constant, of a difference its negation; none of a
/// constant less the column.
fn offset(b: &OuterBound) -> Option<f64> {
    let Some(shift) = &b.shift else { return Some(0.0_f64) };
    let Expr::Arith(x, op, y) = &**shift else { return None };
    let (constant, sign) = match (&**x, op, &**y) {
        (Expr::Column(_), ArithOp::Add, Expr::Const(c)) | (Expr::Const(c), ArithOp::Add, Expr::Column(_)) => (c, 1.0_f64),
        (Expr::Column(_), ArithOp::Sub, Expr::Const(c)) => (c, -1.0_f64),
        _ => return None,
    };
    let Value::Exact(d) = constant else { return None };
    Some(sign * d.to_f64())
}
