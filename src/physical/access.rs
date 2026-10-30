//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index probe, chosen by cost. Split from physical.rs (RULES.md
//! R-SIZE-2).

use super::estimate::{self, Known};
use super::{Built, PhysicalPlanner, Plan, TableRead};
use crate::index::{IndexMethod, Probe};
use crate::logical::{Cond, Expr, SortKey, and_all, pushed_conjuncts, render_cond};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType, Value};

/// A way to read a table: its cost, with and without the sort it may still
/// need, the rows it yields, whether in the order asked for (backward if
/// `reverse`), and the index probe, if it uses one.
struct Path {
    total: f64,
    cost: f64,
    est: f64,
    ordered: bool,
    reverse: bool,
    /// The index and method names, the probe, and the conjuncts it answers.
    index: Option<(String, String, Probe, Vec<usize>)>,
    /// Of a scan, the columns in whose order it reads the table, if any.
    order: Vec<usize>,
}

/// A probe an index could answer: its probe, the conjuncts it answers, how
/// many of the index's first columns it holds equal, and the rows it is
/// guessed to find from the table's statistics.
struct Candidate {
    probe: Probe,
    used: Vec<usize>,
    fixed: usize,
    guess: f64,
}

impl PhysicalPlanner<'_> {
    /// Choose how to read one table: a sequential scan or an index path.
    /// If an order is wanted, unordered paths are charged for a later sort.
    pub(super) fn access(&self, read: TableRead<'_>, mut conds: Vec<Cond>, want: &[SortKey]) -> Built {
        let Ok(storage) = self.db.storage_of(read.table) else {
            // [DESIGN] A bound table has storage. If not, it is scanned anyway,
            // and the executor reports the missing storage (SQLCODE -910).
            let plan =
                Plan::SeqScan { table: read.table.clone(), columns: read.needed.to_vec(), rows: 0, cost: 0.0_f64, order: Vec::new() };
            let cost = self.db.cost();
            return filtered(plan, conds, &Filtered { names: read.names, known: Known { stats: None, cost: &cost }, ordered: false });
        };
        let rows = storage.row_count();
        let best = self.best_path(read, &conds, want);
        let plan = match best.index {
            None => Plan::SeqScan { table: read.table.clone(), columns: read.needed.to_vec(), rows, cost: best.cost, order: best.order },
            Some((index, method, probe, mut used)) => {
                // `used` indexes `conds`, which `best_path` enumerated.
                used.sort_unstable();
                let mut answered = Vec::new();
                for &ci in used.iter().rev() {
                    if ci < conds.len() {
                        answered.push(conds.remove(ci));
                    }
                }
                answered.reverse();
                let detail = and_all(answered).map_or_else(|| "all keys, in order".to_string(), |c| render_cond(&c, read.names));
                Plan::IndexScan {
                    table: read.table.clone(),
                    index,
                    method,
                    probe,
                    reverse: best.reverse,
                    detail,
                    est: best.est,
                    cost: best.cost,
                }
            }
        };
        let (cost, stats) = (self.db.cost(), self.db.statistics(read.table));
        let known = Known { stats: stats.as_deref(), cost: &cost };
        filtered(plan, conds, &Filtered { names: read.names, known, ordered: best.ordered })
    }

    /// The cheapest way to read the table: a sequential scan, an index probe
    /// answering conjuncts of `conds`, or a full ordered index read.
    fn best_path(&self, read: TableRead<'_>, conds: &[Cond], want: &[SortKey]) -> Path {
        let cost = &self.db.cost();
        let sort_penalty = |est: f64, ordered: bool| if !want.is_empty() && !ordered { cost.sort(est) } else { 0.0_f64 };
        let unread = Path { total: 0.0_f64, cost: 0.0_f64, est: 0.0_f64, ordered: false, reverse: false, index: None, order: Vec::new() };
        let (Ok(storage), Some(desc)) = (self.db.storage_of(read.table), self.db.catalog.table(read.table)) else { return unread };
        let rows = storage.row_count();
        let descent = cost.index_descent * cost_of(rows.saturating_add(1)).log2();
        let (seq_cost, scanned) = scan_priced(&**storage, read.needed, conds);
        let stats = self.db.statistics(read.table);
        let all = cost_of(rows);
        let mut best =
            in_order(&**storage, want, Path { total: seq_cost + sort_penalty(scanned, false), cost: seq_cost, est: scanned, ..unread });

        // RULES.md D15: a probe reads only the rows its conjuncts select, so the
        // other conjuncts would not be evaluated on the rest. When one of them
        // can raise an error it must see every row: probe no index then.
        let probes_allowed = !conds.iter().any(Cond::can_raise);
        let compared: Vec<(usize, usize, CompOp, Value)> =
            conds.iter().enumerate().filter_map(|(ci, c)| probe_candidate(c).map(|(col, op, v)| (ci, col, op, v))).collect();
        // An index being built is not read until it is (`Database::create_index`).
        for ix in self.db.catalog.indexes(read.table).into_iter().filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (Some(method), Ok(index), Some(columns)) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), columns) else {
                continue;
            };
            let types: Vec<DataType> = columns.iter().map(|c| c.data_type).collect();
            let mut candidates = if probes_allowed {
                probes(&ix.columns, &types, &compared, all, Known { stats: stats.as_deref(), cost })
            } else {
                Vec::new()
            };
            // A full ordered read, only to provide order.
            candidates.push(Candidate { probe: Probe::All, used: Vec::new(), fixed: 0, guess: all });
            for c in candidates {
                let supported = supports(&**method, &c.probe, (types.len(), !want.is_empty()));
                // Indexes hold no key with a null, so the rows of a probe are
                // all its conjuncts select only if no column it does not test
                // may be null.
                let untested = columns.get(tested(&c.probe)..).unwrap_or_default();
                let order = if method.ordered() && !want.is_empty() { index_order(&ix.columns, c.fixed, want) } else { None };
                if !supported || untested.iter().any(|col| !col.not_null) || matches!(c.probe, Probe::All) && order.is_none() {
                    continue;
                }
                // The index's count if cheap; else, for an equality on all its
                // columns, the rows over its distinct keys; else the guess.
                let whole_key = c.fixed == types.len();
                let by_keys = || index.distinct_keys().filter(|_| whole_key).map(|d| all / d.max(1.0_f64));
                let est = index.estimate(&c.probe).map(cost_of).or_else(by_keys).unwrap_or(c.guess);
                // Its pages to find the entries, if it knows, else a tree's levels.
                let found = index.probe_pages(&c.probe).map_or(descent, |pages| cost.index_descent * pages);
                let path_cost = found + est * storage.fetch_cost();
                let total = path_cost + sort_penalty(est, order.is_some());
                if total < best.total {
                    let probed = Some((ix.name.clone(), ix.method.clone(), c.probe, c.used));
                    let ordered = (order.is_some(), order.unwrap_or(false));
                    best = Path { total, cost: path_cost, est, ordered: ordered.0, reverse: ordered.1, index: probed, order: Vec::new() };
                }
            }
        }
        best
    }
}

/// Whether `method`, of an index of `width` columns, answers `probe`: a
/// read of all its keys only to give an order, if `ordering`.
fn supports(method: &dyn IndexMethod, probe: &Probe, (width, ordering): (usize, bool)) -> bool {
    match probe {
        Probe::All => method.ordered() && ordering,
        Probe::Compare { op, .. } => method.supports(*op),
        Probe::Prefix { equal, then } => method.supports_prefix(width, equal.len(), then.as_ref().map(|(op, _)| *op)),
        Probe::Range { equal, .. } => method.supports_range(width, equal.len()),
    }
}

/// `scan`, a scan of `storage`, read in the order of the columns it keeps
/// its rows sorted by, if `want` asks for the first of them, all ascending
/// (`TableStorage::scan_ordered`): no sort, its rows merged as they are
/// read, so its cost is the scan's.
fn in_order(storage: &dyn TableStorage, want: &[SortKey], scan: Path) -> Path {
    let keys: Vec<usize> = want.iter().map(|k| k.column).collect();
    if want.is_empty() || want.iter().any(|k| k.descending) || !storage.ordered_by().starts_with(&keys) {
        return scan;
    }
    Path { total: scan.cost, ordered: true, order: keys, ..scan }
}

/// The cost of a scan of `storage` reading `needed` under `conds`, and the
/// rows it reads: only the share of them the conditions it is told may
/// match (`TableStorage::scan_share`).
fn scan_priced(storage: &dyn TableStorage, needed: &[usize], conds: &[Cond]) -> (f64, f64) {
    let share = and_all(conds.to_vec()).as_ref().and_then(pushed_conjuncts).map_or(1.0_f64, |c| storage.scan_share(&c));
    (storage.scan_cost(needed) * share, cost_of(storage.row_count()) * share)
}

/// The probes of an index on `columns`, of types `types`, that the
/// comparisons `compared` (conjunct, column, operator, constant) answer: the
/// equalities of its first columns, as many as there are, then each
/// comparison of the next column, or none. Values are of the column's type
/// (`coerce_probe`). An index on one column is asked with
/// `Probe::Compare`, which every method answers that supports the operator.
fn probes(columns: &[usize], types: &[DataType], compared: &[(usize, usize, CompOp, Value)], all: f64, known: Known<'_>) -> Vec<Candidate> {
    let on = |at: usize| {
        let (col, data_type) = (columns.get(at).copied(), types.get(at));
        compared.iter().filter_map(move |(ci, c, op, v)| {
            (Some(*c) == col).then_some(())?;
            Some((*ci, *c, *op, coerce_probe(v, data_type?)?))
        })
    };
    let (mut equal, mut used, mut guess) = (Vec::new(), Vec::new(), all);
    while let Some((ci, col, _, value)) = on(equal.len()).find(|(_, _, op, _)| *op == CompOp::Eq) {
        guess *= estimate::compare((col, CompOp::Eq, &value), known);
        equal.push(value);
        used.push(ci);
    }
    let fixed = equal.len();
    let mut out = Vec::new();
    for (ci, col, op, value) in on(fixed).filter(|(_, _, op, _)| *op != CompOp::Eq || fixed == 0) {
        let guess = guess * estimate::compare((col, op, &value), known);
        let used = [used.as_slice(), &[ci]].concat();
        let probe = if fixed == 0 { Probe::Compare { op, value } } else { Probe::Prefix { equal: equal.clone(), then: Some((op, value)) } };
        out.push(Candidate { probe, used, fixed, guess });
    }
    // [ISO/IEC 9075:1992 §8.3 GR1] a lower and an upper bound of the next
    // column, as of BETWEEN: one range, the keys past the upper not read.
    let bound = |below: bool| {
        on(fixed).find(|(_, _, op, _)| matches!(op, CompOp::Lt | CompOp::Le) == below && *op != CompOp::Eq && *op != CompOp::Ne)
    };
    if let (Some((lo_ci, col, lo_op, lo)), Some((hi_ci, _, hi_op, hi))) = (bound(false), bound(true)) {
        let (a, b) = (estimate::compare((col, lo_op, &lo), known), estimate::compare((col, hi_op, &hi), known));
        // Of one column, both hold where either does not fail: a + b - 1,
        // if the statistics know; else as if independent.
        let guess = guess * (a + b - 1.0_f64).max(a * b);
        let used = [used.as_slice(), &[lo_ci, hi_ci]].concat();
        out.push(Candidate {
            probe: Probe::Range { equal: equal.clone(), low: Box::new((lo_op, lo)), high: Box::new((hi_op, hi)) },
            used,
            fixed,
            guess,
        });
    }
    if fixed > 0 {
        let probe = match (equal.as_slice(), columns.len()) {
            ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
            _ => Probe::Prefix { equal, then: None },
        };
        out.push(Candidate { probe, used, fixed, guess });
    }
    out
}

/// How many of an index's first columns `probe` tests.
fn tested(probe: &Probe) -> usize {
    match probe {
        Probe::All => 0,
        Probe::Compare { .. } => 1,
        Probe::Prefix { equal, then } => equal.len().saturating_add(usize::from(then.is_some())),
        Probe::Range { equal, .. } => equal.len().saturating_add(1),
    }
}

/// Whether reading an index on `columns` in key order, its first `fixed`
/// held equal by its probe, gives rows in the order of `want`: once the
/// keys of the columns held equal are left out, the rest are the index's
/// next columns, in order, all ascending, or all descending, when it is
/// read backward. Some(backward) if so.
fn index_order(columns: &[usize], fixed: usize, want: &[SortKey]) -> Option<bool> {
    let (held, free) = columns.split_at(fixed.min(columns.len()));
    let keys: Vec<&SortKey> = want.iter().filter(|k| !held.contains(&k.column)).collect();
    let Some(first) = keys.first() else { return Some(false) };
    let in_order = keys.len() <= free.len() && keys.iter().zip(free).all(|(k, c)| k.column == *c && k.descending == first.descending);
    in_order.then_some(first.descending)
}

/// What a filter over a table access is planned with.
struct Filtered<'a> {
    /// The table's column names.
    names: &'a [String],
    /// The table's statistics, if its engine gives any, and the guesses.
    known: Known<'a>,
    /// Whether the access yields rows in the order asked for.
    ordered: bool,
}

/// A table access `plan` under a filter for the conjuncts `conds` it does not
/// answer itself, estimated to keep the fraction of the rows read that the
/// table's statistics give.
fn filtered(plan: Plan, conds: Vec<Cond>, with: &Filtered<'_>) -> Built {
    let read = match &plan {
        Plan::SeqScan { rows, .. } => Some(cost_of(*rows)),
        Plan::IndexScan { est, .. } => Some(*est),
        Plan::Filter { .. }
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Distinct { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. } => None,
    };
    let plan = match and_all(conds) {
        Some(c) => {
            let est = read.map(|rows| rows * estimate::selectivity(&c, with.known));
            Plan::Filter { detail: render_cond(&c, with.names), cond: c, est, input: Box::new(plan) }
        }
        None => plan,
    };
    Built { plan, ordered: with.ordered }
}

/// "column `<op>` constant" or "constant `<op>` column".
pub(super) fn probe_candidate(c: &Cond) -> Option<(usize, CompOp, Value)> {
    match c {
        Cond::Compare(Expr::Column(i), op, Expr::Const(v)) => Some((*i, *op, v.clone())),
        Cond::Compare(Expr::Const(v), op, Expr::Column(i)) => Some((*i, op.flip(), v.clone())),
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => None,
    }
}

/// An index on a column of type T can be probed with a constant only when
/// comparing them in the index's key order gives the [X3.135-1986 §5.11] result.
fn coerce_probe(v: &Value, column: &DataType) -> Option<Value> {
    match (column, v) {
        (DataType::Character { .. } | DataType::Varying { .. }, Value::Char(_))
        | (DataType::Exact { .. }, Value::Exact(_))
        | (DataType::Approximate { .. }, Value::Approx(_)) => Some(v.clone()),
        (DataType::Approximate { .. }, Value::Exact(d)) => Some(Value::Approx(d.to_f64())),
        _ => None,
    }
}
