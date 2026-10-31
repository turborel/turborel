//! \[DESIGN\] Index nested-loop joins (RULES.md D35): of a join on
//! equalities whose right operand reads a table, its left operand's rows
//! each probe an index of the table for the values of its first columns
//! the equalities give, rather than the table being read whole into a hash
//! table, when the left's rows are estimated few enough that their probes
//! cost less than that read. An operand reading its table through a
//! projection of its columns, as a derived table's does, is probed as the table,
//! the join under a projection giving the operand's columns; and a RIGHT
//! outer join as the LEFT one of its operands swapped, under a projection
//! putting the left's columns first. Split from physical/join.rs (RULES.md
//! R-SIZE-2).
//!
//! A probe reads only the rows its values select, so the rest of the
//! condition is evaluated only on the pairs it finds, and the table's own
//! filter only on its rows they are of: neither may raise an error (D15).
//! A filter that can is over a guard (`optimizer/raising.rs`), not a
//! table's scan, and is no such operand. An index holds no key with a
//! null, so its columns past those probed are NOT NULL, or a row with a
//! null there would be missed; and the left column's values are of the
//! kind of the indexed column's, which the index places among its keys
//! (`engine/key.rs` `anchor`), not compared with every key.

use super::estimate::{self, Known};
use super::join::Flip;
use super::{PhysicalPlanner, Plan};
use crate::catalog::{ColumnDesc, IndexDesc, QualifiedName};
use crate::index::{IndexMethod, Probe};
use crate::logical::{Cond, Expr, LogicalPlan, and_all, judged_by_sets, map_cond_columns, render_cond, render_expr, split_and};
use crate::storage::cost_of;
use crate::value::{CompOp, DataType, Decimal, Value};

/// An index of the table an operand reads that a join's equalities probe:
/// the table, its filter, the index, and of each of the index's first
/// columns the place among the equalities of the one that gives its
/// value; the rows a probe is estimated to find, and its cost.
pub(super) struct Probing {
    table: QualifiedName,
    filter: Option<Cond>,
    width: usize,
    index: String,
    method: String,
    used: Vec<usize>,
    pub(super) each: f64,
    pub(super) cost: f64,
    /// Of an operand that reads the table through a projection of its
    /// columns, as a derived table's does, the table's column each of its columns
    /// is, and the names of the table's columns.
    through: Option<(Vec<usize>, Vec<String>)>,
}

/// Whether values of a left column are of the kind of an indexed column's
/// type, so that the index places them among its keys.
pub(super) type OfKind<'a> = &'a dyn Fn(usize, &DataType) -> bool;

/// A join to plan by probes: its left operand's width, the pairs of a left
/// and a right column its equalities equate, the rest of its condition,
/// and whether it is a LEFT outer join.
pub(super) struct Equijoin<'a> {
    pub(super) width: usize,
    pub(super) keys: &'a [(usize, usize)],
    pub(super) rest: Vec<Cond>,
    pub(super) outer: bool,
}

/// `join` as an index nested-loop join of `left`, built, probing as
/// `probing` says, the probes of `rows` left rows; its condition shown
/// with the columns of both operands named `names`: the equalities probed,
/// then what is judged of the pairs found. Of an operand that reads the
/// table through a projection of its columns, the join is of the table's
/// rows, its condition renumbered so, under a projection that gives the
/// operand's columns.
pub(super) fn index_join(mut probing: Probing, join: Equijoin<'_>, (left, rows): (Plan, f64), names: &[String]) -> Plan {
    let Some((columns, table_names)) = probing.through.take() else { return table_join(probing, join, (left, rows), names) };
    let width = join.width;
    let of_table = |c: usize| c.checked_sub(width).map_or(Some(c), |at| columns.get(at).map(|&t| t.saturating_add(width)));
    let keys: Vec<(usize, usize)> = join.keys.iter().map(|&(l, r)| (l, columns.get(r).copied().unwrap_or(r))).collect();
    let rest = join.rest.into_iter().map(|c| map_cond_columns(c, &|i| of_table(i).unwrap_or(i))).collect();
    let mut joined_names: Vec<String> = names.iter().take(width).cloned().collect();
    joined_names.extend(table_names);
    let joined = table_join(probing, Equijoin { keys: &keys, rest, ..join }, (left, rows), &joined_names);
    let exprs: Vec<Expr> = (0..width).chain(columns.iter().map(|&t| t.saturating_add(width))).map(Expr::Column).collect();
    let detail = exprs.iter().map(|e| render_expr(e, &joined_names)).collect::<Vec<_>>().join(", ");
    Plan::Project { exprs, detail, input: Box::new(joined) }
}

/// The RIGHT outer join of a left operand that reads a table and a right
/// operand of `rows` rows, built, `right`, on the equalities `swapped`,
/// each a right column and a left column, and the rest of its condition
/// `rest`, over the left's `width` columns then the right's, named
/// `names`: as the LEFT index join of `right` probing the left's table as
/// `probing` says, under a projection that puts the left's columns first.
pub(super) fn right_index_join(
    probing: Probing,
    (swapped, rest): (&[(usize, usize)], Vec<Cond>),
    (right, rows): (Plan, f64),
    (width, names): (usize, &[String]),
) -> Plan {
    let flip = Flip { width, right_width: names.len().saturating_sub(width) };
    let rest = rest.into_iter().map(|c| flip.cond(c)).collect();
    let flipped = flip.names(names);
    let join = Equijoin { width: flip.right_width, keys: swapped, rest, outer: true };
    flip.restored(index_join(probing, join, (right, rows), &flipped), &flipped)
}

/// `index_join` of the table's own rows.
fn table_join(probing: Probing, join: Equijoin<'_>, (left, rows): (Plan, f64), names: &[String]) -> Plan {
    let width = join.width;
    let equal = |&(l, r): &(usize, usize)| Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(r.saturating_add(width)));
    let keys: Vec<usize> = probing.used.iter().filter_map(|&k| join.keys.get(k).map(|&(l, _)| l)).collect();
    // The equalities no probe answers, and the table's filter, are judged of the pairs.
    let unused = join.keys.iter().enumerate().filter(|(k, _)| !probing.used.contains(k)).map(|(_, key)| equal(key));
    let filter = probing.filter.map(|c| map_cond_columns(c, &|i| i.saturating_add(width)));
    let cond = and_all(join.rest.into_iter().chain(unused).chain(filter).collect()).map(judged_by_sets);
    let probed = probing.used.iter().filter_map(|&k| join.keys.get(k)).map(equal);
    let detail = and_all(probed.chain(cond.clone()).collect()).map(|c| render_cond(&c, names)).unwrap_or_default();
    Plan::IndexJoin {
        outer: join.outer,
        left: Box::new(left),
        table: probing.table,
        index: probing.index,
        method: probing.method,
        keys,
        cond,
        width: probing.width,
        detail,
        est: probing.each,
        cost: rows * probing.cost,
    }
}

impl PhysicalPlanner<'_> {
    /// How `right` is probed for the rows of `left` joined on the
    /// equalities `keys`, and `left`'s estimated rows, if it reads a table
    /// one of whose indexes they probe, and those rows' probes are
    /// estimated to cost less than `read`, the read of its rows the hash
    /// join's right operand would be (`access_cost`).
    pub(super) fn probed_instead(
        &self,
        (left, right): (&LogicalPlan, &LogicalPlan),
        keys: &[(usize, usize)],
        read: &Plan,
    ) -> Option<(Probing, f64)> {
        let of_kind = |l: usize, t: &DataType| self.column_type(left, l).is_some_and(|lt| alike(&lt, t));
        let probing = self.probing(right, keys, &of_kind)?;
        let rows = self.rows_of(left)?;
        (rows * probing.cost < access_cost(read)?).then_some((probing, rows))
    }

    /// The cheapest index of the table `right` reads, filtered by a
    /// condition that cannot raise an error or not, whose first columns
    /// the equalities `keys` give values of, of their kinds as `of_kind`
    /// says of a left column and an indexed column's type.
    pub(super) fn probing(&self, right: &LogicalPlan, keys: &[(usize, usize)], of_kind: OfKind<'_>) -> Option<Probing> {
        let Read { table, filter, through } = read_table(right)?;
        // The equalities' right columns, of the table's.
        let keys: Vec<(usize, usize)> = match &through {
            Some((columns, _)) => keys.iter().map(|&(l, r)| columns.get(r).map(|&t| (l, t))).collect::<Option<_>>()?,
            None => keys.to_vec(),
        };
        let (storage, desc) = (self.db.storage_of(table).ok()?, self.db.catalog.table(table)?);
        let model = self.db.cost();
        let stats = self.db.statistics(table);
        let known = Known { stats: stats.as_deref(), cost: &model };
        let rows = cost_of(storage.row_count());
        let mut best: Option<Probing> = None;
        for ix in self.db.catalog.indexes(table).into_iter().filter(|ix| !self.db.building.contains(&ix.name)) {
            let (Some(kind), Ok(found)) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name)) else { continue };
            let Some((used, types)) = probed_columns(ix, &desc.columns, (&keys, of_kind), &**kind) else { continue };
            let whole = used.len() == ix.columns.len();
            let by_columns = || {
                let distinct =
                    |c: usize| known.stats.and_then(|s| s.distinct(c)).map_or(1.0_f64 / model.equal_selectivity, |d| d.max(1.0_f64));
                ix.columns.iter().take(used.len()).map(|&c| 1.0_f64 / distinct(c)).product::<f64>()
            };
            let each = found.distinct_keys().filter(|_| whole).map_or_else(|| rows * by_columns(), |d| rows / d.max(1.0_f64));
            let sample: Option<Vec<Value>> = types.iter().take(used.len()).map(sample).collect();
            let pages = sample
                .and_then(|values| found.probe_pages(&Probe::Prefix { equal: values, then: None }))
                .unwrap_or_else(|| (rows + 1.0_f64).log2());
            let cost = model.index_descent * pages + each * storage.fetch_cost();
            if best.as_ref().is_none_or(|b| cost < b.cost) {
                let (index, method, width) = (ix.name.clone(), ix.method.clone(), desc.columns.len());
                let through = through.clone();
                best = Some(Probing { table: table.clone(), filter: filter.cloned(), width, index, method, used, each, cost, through });
            }
        }
        best
    }

    /// The rows `plan` is estimated to give, if it reads tables, filters,
    /// products and projections of them, or a list of rows. A filter of a
    /// product, as a join is, keeps of an equality of two columns one pair
    /// in as many as the more distinct of them has values (`distinct_of`),
    /// as join ordering takes it (`physical/order.rs`).
    pub(super) fn rows_of(&self, plan: &LogicalPlan) -> Option<f64> {
        match plan {
            LogicalPlan::Scan { table, .. } => self.db.storage_of(table).ok().map(|s| cost_of(s.row_count())),
            LogicalPlan::Filter { cond, input } => {
                let stats = if let LogicalPlan::Scan { table, .. } = &**input { self.db.statistics(table) } else { None };
                let model = self.db.cost();
                let known = Known { stats: stats.as_deref(), cost: &model };
                let mut conjuncts = Vec::new();
                split_and(cond.clone(), &mut conjuncts);
                let equal = |c: &Cond| if stats.is_some() { None } else { self.equality_share(input, c) };
                let share = conjuncts.iter().map(|c| equal(c).unwrap_or_else(|| estimate::selectivity(c, known))).product::<f64>();
                Some(self.rows_of(input)? * share)
            }
            LogicalPlan::Product { left, right } => Some(self.rows_of(left)? * self.rows_of(right)?),
            LogicalPlan::Project { input, .. } | LogicalPlan::Distinct { input } | LogicalPlan::Sort { input, .. } => self.rows_of(input),
            LogicalPlan::Values { rows, .. } => Some(cost_of(rows.len())),
            LogicalPlan::OuterJoin { .. } | LogicalPlan::Aggregate { .. } | LogicalPlan::SetOp { .. } | LogicalPlan::System { .. } => None,
        }
    }

    /// The share of `plan`'s rows the equality of two of its columns `c`
    /// keeps, if it is one and their distinct values are known.
    fn equality_share(&self, plan: &LogicalPlan, c: &Cond) -> Option<f64> {
        let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
        let distinct = self.distinct_of(plan, *a)?.max(self.distinct_of(plan, *b)?).max(1.0_f64);
        Some(1.0_f64 / distinct)
    }

    /// The distinct values of `plan`'s column `column`, if it is a table's
    /// column: as its table's statistics count them, else its table's rows,
    /// as if a key; a filter's as its input's, so no estimate is made
    /// again of each level of a chain of joins.
    fn distinct_of(&self, plan: &LogicalPlan, column: usize) -> Option<f64> {
        match plan {
            LogicalPlan::Scan { table, .. } => self.db.statistics(table).and_then(|s| s.distinct(column)).or_else(|| self.rows_of(plan)),
            LogicalPlan::Filter { input, .. } | LogicalPlan::Distinct { input } | LogicalPlan::Sort { input, .. } => {
                self.distinct_of(input, column)
            }
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
                Some(of_right) => self.distinct_of(right, of_right),
                None => self.distinct_of(left, column),
            },
            LogicalPlan::Project { exprs, input } => {
                let Some(Expr::Column(c)) = exprs.get(column) else { return None };
                self.distinct_of(input, *c)
            }
            LogicalPlan::Aggregate { .. } | LogicalPlan::SetOp { .. } | LogicalPlan::Values { .. } | LogicalPlan::System { .. } => None,
        }
    }

    /// The type of `plan`'s column `column`, if it is a table's column.
    fn column_type(&self, plan: &LogicalPlan, column: usize) -> Option<DataType> {
        match plan {
            LogicalPlan::Scan { table, .. } => self.db.catalog.table(table)?.columns.get(column).map(|c| c.data_type),
            LogicalPlan::Filter { input, .. } | LogicalPlan::Distinct { input } | LogicalPlan::Sort { input, .. } => {
                self.column_type(input, column)
            }
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
                Some(of_right) => self.column_type(right, of_right),
                None => self.column_type(left, column),
            },
            LogicalPlan::Project { exprs, input } => {
                let Some(Expr::Column(c)) = exprs.get(column) else { return None };
                self.column_type(input, *c)
            }
            LogicalPlan::Aggregate { .. } | LogicalPlan::SetOp { .. } | LogicalPlan::Values { .. } | LogicalPlan::System { .. } => None,
        }
    }
}

/// Of the index `ix`, on columns of `columns`, of a method of `kind`, the
/// place among `keys` of the equality giving each of its first columns
/// values of its kind, as `of_kind` says, and the types of its columns: if
/// it gives some, the method answers a probe of them, and the columns past
/// them are NOT NULL, as an index holds no key with a null.
fn probed_columns(
    ix: &IndexDesc,
    columns: &[ColumnDesc],
    (keys, of_kind): (&[(usize, usize)], OfKind<'_>),
    kind: &dyn IndexMethod,
) -> Option<(Vec<usize>, Vec<DataType>)> {
    let types: Vec<DataType> = ix.columns.iter().map(|&c| columns.get(c).map(|d| d.data_type)).collect::<Option<_>>()?;
    let mut used = Vec::new();
    for (&column, data_type) in ix.columns.iter().zip(&types) {
        let Some(k) = keys.iter().position(|&(l, r)| r == column && of_kind(l, data_type)) else { break };
        used.push(k);
    }
    let probed = used.len();
    let supported =
        if probed == 1 && ix.columns.len() == 1 { kind.supports(CompOp::Eq) } else { kind.supports_prefix(ix.columns.len(), probed, None) };
    let untested = ix.columns.get(probed..).unwrap_or_default();
    let nullable = untested.iter().any(|&c| columns.get(c).is_none_or(|d| !d.not_null));
    (probed > 0 && supported && !nullable).then_some((used, types))
}

/// The cost the planner gave the read of a table `plan` is: of its scan,
/// or of its index scans, filtered or not, and projected, as a derived table's
/// columns are, which costs nothing beside the read.
fn access_cost(plan: &Plan) -> Option<f64> {
    match plan {
        Plan::SeqScan { cost, .. } | Plan::IndexScan { cost, .. } => Some(*cost),
        Plan::Filter { input, .. } | Plan::Project { input, .. } => access_cost(input),
        Plan::Append { left, right } => Some(access_cost(left)? + access_cost(right)?),
        Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Sort { .. } => None,
    }
}

/// How an operand reads a table: the table, its filter, and of a
/// projection of its columns, each column's place among the table's and
/// their names (`Probing`).
struct Read<'a> {
    table: &'a QualifiedName,
    filter: Option<&'a Cond>,
    through: Option<(Vec<usize>, Vec<String>)>,
}

/// How `plan` reads a table, if it is a table's scan, a filter of one that
/// cannot raise an error, or a projection of the columns of one of these,
/// as a derived table's is, or of such a projection. A projection of another
/// expression is not: of an outer join, its value of a row given nulls
/// would not be null; of an inner, it would be evaluated of fewer rows.
fn read_table(plan: &LogicalPlan) -> Option<Read<'_>> {
    match plan {
        LogicalPlan::Scan { table, .. } => Some(Read { table, filter: None, through: None }),
        LogicalPlan::Filter { cond, input } => {
            let LogicalPlan::Scan { table, .. } = &**input else { return None };
            (!cond.can_raise()).then_some(Read { table, filter: Some(cond), through: None })
        }
        LogicalPlan::Project { exprs, input } => {
            let columns: Vec<usize> = exprs.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect::<Option<_>>()?;
            let read = read_table(input)?;
            let through = match read.through {
                Some((inner, names)) => (columns.iter().map(|&c| inner.get(c).copied()).collect::<Option<_>>()?, names),
                None => (columns, input.names()),
            };
            Some(Read { through: Some(through), ..read })
        }
        LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. } => None,
    }
}

/// Whether values of type `a` are placed among the keys of a column of
/// type `b` (`engine/key.rs` `anchor`): of one kind, character strings of
/// either length, and bit strings, alike.
fn alike(a: &DataType, b: &DataType) -> bool {
    family(a) == family(b)
}

/// The name of the kind of `t`, the same of fixed and varying strings.
fn family(t: &DataType) -> &'static str {
    match t {
        DataType::Character { .. } | DataType::Varying { .. } => "CHARACTER",
        DataType::Bit { .. } | DataType::BitVarying { .. } => "BIT",
        DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Extension(_) => t.kind().name(),
    }
}

/// A value of type `t`, to ask an index the pages a probe of it reads,
/// which do not depend on which value: of a number or a string; none of
/// another type.
fn sample(t: &DataType) -> Option<Value> {
    match t {
        DataType::Exact { .. } => Some(Value::Exact(Decimal::new(0, 0))),
        DataType::Approximate { .. } => Some(Value::Approx(0.0_f64)),
        DataType::Character { .. } | DataType::Varying { .. } => Some(Value::Char(String::new())),
        DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => None,
    }
}

// Unit tests: the kinds a probe compares, its sample values, and the cost
// of a read (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Spelled;

    /// Verifies: [DESIGN] RULES.md D35: character strings of either length
    /// are of one kind, as are bit strings, but exact and approximate
    /// numbers are not; a probe's pages are asked with a value of a number
    /// or a string, of either kind; and only a table's read has a cost to
    /// set the probes against.
    #[test]
    fn kinds_samples_and_reads() {
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
        assert!(alike(&DataType::Character { length: 1 }, &DataType::Varying { length: 2 }));
        assert!(alike(&DataType::Bit { length: 1 }, &DataType::BitVarying { length: 8 }));
        assert!(!alike(&integer, &double) && alike(&double, &double));
        assert!(matches!(sample(&double), Some(Value::Approx(_))) && matches!(sample(&integer), Some(Value::Exact(_))));
        assert!(sample(&DataType::Date).is_none());
        assert_eq!(access_cost(&Plan::Values { rows: Vec::new(), detail: String::new() }), None);
    }

    /// Verifies: [DESIGN] RULES.md D31: an IN list judged by a set keeps,
    /// by the planner's estimate, the share each of its equalities keeps,
    /// summed, of a column or of another expression.
    #[test]
    fn a_set_is_estimated_as_its_equalities() {
        use std::sync::Arc;

        use crate::logical::InSet;
        let model = super::super::CostModel::default();
        let known = Known { stats: None, cost: &model };
        let set = Arc::new(InSet::of(vec![Value::Exact(Decimal::new(1, 0)), Value::Exact(Decimal::new(2, 0))]));
        let twice = 2.0_f64 * model.equal_selectivity;
        for of in [Expr::Column(0), Expr::Const(Value::Exact(Decimal::new(1, 0)))] {
            let share = estimate::selectivity(&Cond::In(of, Arc::clone(&set)), known);
            assert!((share - twice).abs() < f64::EPSILON, "{share}");
        }
    }
}
