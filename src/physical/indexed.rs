//! \[DESIGN\] Index nested-loop joins (RULES.md D35): of a join on equalities
//! whose right operand reads a table, its left operand's rows each probe an
//! index of the table for the values of its first columns the equalities give,
//! rather than the table being read whole into a hash table, when the left's
//! rows are estimated few enough that their probes cost less than that read,
//! and would at the most rows the estimate may be wrong by (D386).
//! An operand reading its table through a projection of its columns, as a
//! derived table's does, is probed as the table, the join under a projection
//! giving the operand's columns; and a RIGHT outer join as the LEFT one of its
//! operands swapped, under a projection putting the left's columns first.
//! Independent-coordinate inner joins use the same executor with bounds
//! on several key columns (`indexed/coordinates.rs`). Geometry joins use
//! envelope candidates with a full shape recheck (`indexed/spatial.rs`).
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

use std::sync::Arc;

use super::cost::HASH_ROW;
use super::estimate::Known;
use super::robust::{Linear, choose, pessimistic};
use super::join::Flip;
use super::shown::Shown;
use super::{JoinComparison, JoinProbe, PhysicalPlanner, Plan, Preserved};

mod coordinates;
mod expand;
mod fan_out;
mod probed;
mod routed;
mod spatial;
use crate::catalog::{ColumnDesc, IndexDesc, QualifiedName};
use crate::index::IndexMethod;
use crate::logical::{Cond, Expr, LogicalPlan, and_all, judged_by_sets, map_cond_columns, render_cond, render_expr};
use crate::value::datetime::{Datetime, Moment};
use crate::value::interval::Interval;
use crate::value::{Binary, CompOp, DataType, Decimal, Value};
use routed::Candidate;

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
    /// The table's columns the query uses: the only ones a row found need
    /// be read of.
    needed: Vec<usize>,
    /// Whether each row found is followed by its id (`LogicalPlan::Scan`).
    located: bool,
    /// Of a probe of other columns than the key's first, each used key's
    /// place in the index's key (`indexed/expand.rs`).
    positions: Option<Vec<usize>>,
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
/// with the columns of both operands described as `shown` says: the equalities probed,
/// then what is judged of the pairs found. Of an operand that reads the
/// table through a projection of its columns, the join is of the table's
/// rows, its condition renumbered so, under a projection that gives the
/// operand's columns.
pub(super) fn index_join(mut probing: Probing, join: Equijoin<'_>, (left, rows): (Plan, f64), shown: Shown<'_>) -> Plan {
    let Some((columns, table_names)) = probing.through.take() else { return table_join(probing, join, (left, rows), shown) };
    let width = join.width;
    let of_table = |c: usize| c.checked_sub(width).map_or(Some(c), |at| columns.get(at).map(|&t| t.saturating_add(width)));
    let keys: Vec<(usize, usize)> = join.keys.iter().map(|&(l, r)| (l, columns.get(r).copied().unwrap_or(r))).collect();
    let rest = join.rest.into_iter().map(|c| map_cond_columns(c, &|i| of_table(i).unwrap_or(i))).collect();
    let joined_names = shown.names().map(|names| {
        let mut joined: Vec<String> = names.iter().take(width).cloned().collect();
        joined.extend(table_names);
        joined
    });
    let of_joined = Shown::of(joined_names.as_deref());
    let joined = table_join(probing, Equijoin { keys: &keys, rest, ..join }, (left, rows), of_joined);
    let exprs: Vec<Expr> = (0..width).chain(columns.iter().map(|&t| t.saturating_add(width))).map(Expr::Column).collect();
    let detail = of_joined.text(|names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
    Plan::Project { exprs, detail, input: Box::new(joined) }
}

/// The RIGHT outer join of a left operand that reads a table and a right
/// operand of `rows` rows, built, `right`, on the equalities `swapped`,
/// each a right column and a left column, and the rest of its condition
/// `rest`, over the left's `width` columns then the right's, `right_width`
/// of them, described as `shown` says: as the LEFT index join of `right`
/// probing the left's table as `probing` says, under a projection that
/// puts the left's columns first.
pub(super) fn right_index_join(
    probing: Probing,
    (swapped, rest): (&[(usize, usize)], Vec<Cond>),
    (right, rows): (Plan, f64),
    ((width, right_width), shown): ((usize, usize), Shown<'_>),
) -> Plan {
    let flip = Flip { width, right_width };
    let rest = rest.into_iter().map(|c| flip.cond(c)).collect();
    let flipped = flip.names(shown);
    let of_flipped = Shown::of(flipped.as_deref());
    let join = Equijoin { width: flip.right_width, keys: swapped, rest, outer: true };
    flip.restored(index_join(probing, join, (right, rows), of_flipped), of_flipped)
}

/// `index_join` of the table's own rows.
fn table_join(probing: Probing, join: Equijoin<'_>, (left, rows): (Plan, f64), shown: Shown<'_>) -> Plan {
    let width = join.width;
    let equal = |&(l, r): &(usize, usize)| Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(r.saturating_add(width)));
    let keys: Vec<usize> = probing.used.iter().filter_map(|&k| join.keys.get(k).map(|&(l, _)| l)).collect();
    // The equalities no probe answers, and the table's filter, are judged of the pairs.
    let unused = join.keys.iter().enumerate().filter(|(k, _)| !probing.used.contains(k)).map(|(_, key)| equal(key));
    let filter = probing.filter.map(|c| map_cond_columns(c, &|i| i.saturating_add(width)));
    let cond = and_all(join.rest.into_iter().chain(unused).chain(filter).collect()).map(judged_by_sets);
    let probed = probing.used.iter().filter_map(|&k| join.keys.get(k)).map(equal);
    let detail = shown.text(|names| and_all(probed.chain(cond.clone()).collect()).map(|c| render_cond(&c, names)).unwrap_or_default());
    Plan::IndexJoin {
        outer: if join.outer { Preserved::Outer } else { Preserved::Inner },
        spatial: None,
        left: Box::new(left),
        table: probing.table,
        index: probing.index.into(),
        method: probing.method.into(),
        probe: match probing.positions {
            Some(at) => {
                JoinProbe::Columns(keys.iter().zip(at).map(|(&outer, column)| JoinComparison { column, op: CompOp::Eq, outer }).collect())
            }
            None => JoinProbe::Prefix(keys.into_boxed_slice()),
        },
        cond,
        width: probing.width,
        columns: probing.needed.into_boxed_slice(),
        detail: detail.into(),
        est: probing.each,
        cost: rows * probing.cost,
        located: probing.located,
    }
}

impl PhysicalPlanner<'_> {
    /// How `right` is probed for the rows of `left` joined on the
    /// equalities `keys`, and `left`'s estimated rows, if it reads a table
    /// one of whose indexes they probe, and the probes are of less regret
    /// than the hash join they replace (`physical/robust.rs`, RULES.md
    /// D386): at `left`'s estimate and at its pessimistic rows, its bound
    /// or its estimate times PLANNER.ESTIMATE_MARGIN, each probe against
    /// `read`, the read of its rows the hash join's right operand would be
    /// (`access_cost`), and each row of either operand hashed (`HASH_ROW`).
    pub(super) fn probed_instead(
        &self,
        (left, right): (&LogicalPlan, &LogicalPlan),
        keys: &[(usize, usize)],
        read: &Plan,
    ) -> Option<(Probing, f64)> {
        let of_kind = |l: usize, t: &DataType| self.column_type(left, l).is_some_and(|lt| alike(&lt, t));
        let probing = self.probing(right, keys, &of_kind)?;
        let card = self.card(left);
        let hash = Linear { fixed: access_cost(read)? + HASH_ROW * self.rows_of(right), per_row: HASH_ROW };
        let index = Linear { fixed: 0.0_f64, per_row: probing.cost };
        let worst = pessimistic(card.rows, card.upper, self.weights().estimate_margin);
        let probes = choose(&[(false, hash), (true, index)], (card.rows, worst)).unwrap_or(false);
        probes.then_some((probing, card.rows))
    }

    /// The cheapest index of the table `right` reads, filtered by a
    /// condition that cannot raise an error or not, whose first columns
    /// the equalities `keys` give values of, of their kinds as `of_kind`
    /// says of a left column and an indexed column's type.
    pub(super) fn probing(&self, right: &LogicalPlan, keys: &[(usize, usize)], of_kind: OfKind<'_>) -> Option<Probing> {
        let Read { table, filter, through, needed, located } = read_table(right)?;
        // The equalities' right columns, of the table's.
        let keys: Vec<(usize, usize)> = match &through {
            Some((columns, _)) => keys.iter().map(|&(l, r)| columns.get(r).map(|&t| (l, t))).collect::<Option<_>>()?,
            None => keys.to_vec(),
        };
        let desc = self.db.catalog.table(table)?;
        // [EXTENSION] a partitioned table's are its partitions' and its
        // global ones (`indexed/routed.rs`, RULES.md D156).
        let (storage, candidates) = if self.db.catalog.partitioning().is_partitioned(table) {
            self.routed_candidates(table).filter(|_| !located)?
        } else {
            (Arc::clone(self.db.storage_of(table).ok()?), self.own_candidates(table))
        };
        let (model, stats) = (self.weights(), self.statistics(table));
        let known = Known { stats: stats.as_deref(), cost: &model };
        let (rows, columns) = (self.table_rows(table)?, expand::kept(&desc.columns, filter));
        let mut best: Option<Probing> = None;
        for Candidate { ix, found, routing } in &candidates {
            let Some(kind) = self.db.methods.get(&ix.method) else { continue };
            let Some(((used, at, types), reverse)) = expand::probed(ix, &columns, (&keys, of_kind), &**kind) else { continue };
            let whole = !reverse && used.len() == ix.columns.len();
            let by_columns = || {
                let distinct =
                    |c: usize| known.stats.and_then(|s| s.distinct(c)).map_or(1.0_f64 / model.equal_selectivity, |d| d.max(1.0_f64));
                at.iter().filter_map(|&k| ix.columns.get(k)).map(|&c| 1.0_f64 / distinct(c)).product::<f64>()
            };
            let probed: Vec<usize> = at.iter().filter_map(|&k| ix.columns.get(k).copied()).collect(); // `indexed/fan_out.rs`
            let fan_out = || self.edge_fan_out(table, &probed).unwrap_or_else(|| rows * by_columns());
            let keys_counted = (whole && routing.is_none()).then(|| self.index_keys(&ix.name, &**found)).flatten();
            let each = keys_counted.map_or_else(fan_out, |d| rows / d.max(1.0_f64));
            let pages = Candidate::pages(&**found, (&types, used.len()), reverse, rows);
            let missed = found.probe_missed().unwrap_or(0.0_f64) * storage.miss_cost();
            // Of each partition's index, each partition a probe is routed to probed.
            let (partitions, shown) = routing.as_ref().map_or((1.0_f64, String::new()), |r| r.probes(&probed));
            let cost = (model.index_descent * pages + missed) * partitions + each * storage.fetch_cost();
            if best.as_ref().is_none_or(|b| cost < b.cost) {
                best = Some(Probing {
                    table: table.clone(),
                    filter: filter.cloned(),
                    // A row's id follows its columns.
                    width: desc.columns.len().saturating_add(usize::from(located)),
                    index: ix.name.clone(),
                    method: format!("{}{shown}", expand::method_shown(ix, used.len(), reverse)),
                    used,
                    each,
                    cost,
                    through: through.clone(),
                    needed: needed.to_vec(),
                    located,
                    positions: reverse.then(|| at.clone()),
                });
            }
        }
        best
    }

    /// The indexes of `table`, not partitioned, a probe may read: those
    /// not being built.
    fn own_candidates(&self, table: &QualifiedName) -> Vec<Candidate> {
        let usable = self.db.catalog.indexes(table).into_iter().filter(|ix| !self.db.building.contains(&ix.name));
        usable
            .filter_map(|ix| Some(Candidate { ix: ix.clone(), found: Arc::clone(self.db.index_of(&ix.name).ok()?), routing: None }))
            .collect()
    }

    /// The rows `plan` is estimated to give (`physical/cardinality.rs`),
    /// known of every plan.
    pub(super) fn rows_of(&self, plan: &LogicalPlan) -> f64 {
        self.card(plan).rows
    }

    /// The distinct values of `plan`'s column `column`, through the nodes
    /// over its table (`cardinality/lineage.rs`): as its table's statistics
    /// count them, else its table's rows, as if a key.
    pub(super) fn distinct_of(&self, plan: &LogicalPlan, column: usize) -> f64 {
        self.estimating(|e| e.distinct(plan, column))
    }

    /// The type of `plan`'s column `column`, if it is a table's column.
    pub(super) fn column_type(&self, plan: &LogicalPlan, column: usize) -> Option<DataType> {
        match plan {
            LogicalPlan::Scan { table, .. } => self.db.catalog.table(table)?.columns.get(column).map(|c| c.data_type.clone()),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. } => self.column_type(input, column),
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
                Some(of_right) => self.column_type(right, of_right),
                None => self.column_type(left, column),
            },
            LogicalPlan::Project { exprs, input } => {
                let Some(Expr::Column(c)) = exprs.get(column) else { return None };
                self.column_type(input, *c)
            }
            LogicalPlan::Values { types, .. } => types.get(column).cloned(),
            LogicalPlan::Aggregate { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..)
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. } => None,
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
    // An equality of two columns' values is no probe of sort keys.
    if ix.is_collated() {
        return None;
    }
    let types: Vec<DataType> = ix.columns.iter().map(|&c| columns.get(c).map(|d| d.data_type.clone())).collect::<Option<_>>()?;
    let mut used = Vec::new();
    for (&column, data_type) in ix.columns.iter().zip(&types) {
        let Some(k) = keys.iter().position(|&(l, r)| r == column && of_kind(l, data_type)) else { break };
        used.push(k);
    }
    let probed = used.len();
    let supported = if probed == 1 && ix.columns.len() == 1 {
        kind.supports_compare(CompOp::Eq, &types)
    } else {
        kind.supports_prefix(ix.columns.len(), probed, None)
    };
    let untested = ix.columns.get(probed..).unwrap_or_default();
    let nullable = untested.iter().any(|&c| columns.get(c).is_none_or(|d| !d.not_null));
    (probed > 0 && supported && !nullable).then_some((used, types))
}

/// The cost the planner gave the read of a table `plan` is: of its scan,
/// or of its index scans, filtered or not, and projected, as a derived table's
/// columns are, which costs nothing beside the read.
pub(super) fn access_cost(plan: &Plan) -> Option<f64> {
    match plan {
        Plan::SeqScan { cost, .. } | Plan::IndexScan { cost, .. } => Some(*cost),
        Plan::Filter { input, .. } | Plan::Project { input, .. } => access_cost(input),
        Plan::Append { left, right, .. } => Some(access_cost(left)? + access_cost(right)?),
        Plan::MergeAppend { inputs, .. } => inputs.iter().map(access_cost).sum(),
        Plan::Product { .. }
        | Plan::Nearest(_)
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..) => None,
    }
}

/// How an operand reads a table: the table, its filter, and of a
/// projection of its columns, each column's place among the table's and
/// their names (`Probing`).
struct Read<'a> {
    table: &'a QualifiedName,
    filter: Option<&'a Cond>,
    through: Option<(Vec<usize>, Vec<String>)>,
    needed: &'a [usize],
    located: bool,
}

/// Whether `plan` reads a table, as `read_table` says.
pub(super) fn reads_table(plan: &LogicalPlan) -> bool {
    read_table(plan).is_some()
}

/// How `plan` reads a table, if it is a table's scan, a filter of one that
/// cannot raise an error, or a projection of the columns of one of these,
/// as a derived table's is, or of such a projection. A projection of another
/// expression is not: of an outer join, its value of a row given nulls
/// would not be null; of an inner, it would be evaluated of fewer rows.
fn read_table(plan: &LogicalPlan) -> Option<Read<'_>> {
    match plan {
        LogicalPlan::Scan { table, needed, located, .. } => Some(Read { table, filter: None, through: None, needed, located: *located }),
        LogicalPlan::Filter { cond, input } => {
            let LogicalPlan::Scan { table, needed, located, .. } = &**input else { return None };
            (!cond.can_raise()).then_some(Read { table, filter: Some(cond), through: None, needed, located: *located })
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
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..)
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => None,
    }
}

/// Whether values of type `a` are placed among the keys of a column of
/// type `b` (`engine/key.rs` `anchor`): of one kind, character strings of
/// either length, and binary strings, alike; an exact number among
/// DECFLOAT keys too, which hold it exactly.
pub(super) fn alike(a: &DataType, b: &DataType) -> bool {
    family(a) == family(b) || (matches!(a, DataType::Exact { .. }) && matches!(b, DataType::DecFloat { .. }))
}

/// The name of the kind of `t`, the same of fixed and varying strings.
fn family(t: &DataType) -> &'static str {
    match t {
        DataType::Character { .. } | DataType::Varying { .. } => "CHARACTER",
        DataType::Binary { .. } | DataType::VarBinary { .. } => "BINARY",
        DataType::Distinct(d) => family(&d.source),
        DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => t.kind().name(),
    }
}

/// A value of type `t`, to ask an index the pages a probe of it reads,
/// which do not depend on which value: of each type built in; none of an
/// extension's.
fn sample(t: &DataType) -> Option<Value> {
    let moment = |moment: Moment, zoned: bool| Value::Datetime(Datetime { moment, utc: 0, zone: 0, zoned, precision: 0 });
    match t {
        DataType::Exact { .. } => Some(Value::Exact(Decimal::new(0, 0))),
        DataType::Approximate { .. } => Some(Value::Approx(0.0_f64)),
        DataType::DecFloat { .. } => Some(Value::DecFloat(crate::value::DecFloat::ZERO)),
        DataType::Character { .. } | DataType::Varying { .. } => Some(Value::Char(String::new())),
        DataType::Binary { .. } | DataType::VarBinary { .. } => Some(Value::Binary(Binary::new(vec![0]))),
        DataType::Boolean => Some(Value::Bool(false)),
        DataType::Date => Some(moment(Moment::Date, false)),
        DataType::Time { zoned, .. } => Some(moment(Moment::Time, *zoned)),
        DataType::Timestamp { zoned, .. } => Some(moment(Moment::Timestamp, *zoned)),
        DataType::Interval(qualifier) => Some(Value::Interval(Interval { units: 0, qualifier: *qualifier })),
        DataType::Distinct(d) => sample(&d.source),
        // No index is of a large object [ISO/IEC 9075-2:2023 §9.14 SR3].
        DataType::Lob { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
    }
}

// Unit tests: the kinds a probe compares, its sample values, and the cost
// of a read (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::estimate;
    use crate::value::Spelled;

    /// Verifies: [DESIGN] RULES.md D35: character strings of either length
    /// are of one kind, as are binary strings, but exact and approximate
    /// numbers are not; a probe's pages are asked with a value of each type
    /// built in; and only a table's read has a cost to set the probes
    /// against.
    #[test]
    fn kinds_samples_and_reads() {
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
        assert!(alike(&DataType::character(1), &DataType::varying(2)));
        assert!(alike(&DataType::Binary { length: 1 }, &DataType::VarBinary { length: 8 }));
        assert!(!alike(&integer, &double) && alike(&double, &double));
        assert!(matches!(sample(&double), Some(Value::Approx(_))) && matches!(sample(&integer), Some(Value::Exact(_))));
        assert!(matches!(sample(&DataType::Date), Some(Value::Datetime(_))));
        assert!(matches!(sample(&DataType::Binary { length: 8 }), Some(Value::Binary(_))));
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
