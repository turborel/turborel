//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index's probes (`physical/probe.rs`), one, or one of each range of
//! an OR, chosen by cost. Split from physical.rs (RULES.md R-SIZE-2).

use super::estimate::{self, Known};
use super::probe::{Alternatives, Compared, coerce, comparisons, probes_of_any, render_probe};
use super::{Built, PhysicalPlanner, Plan, TableRead};
use crate::index::{Index, IndexMethod, Probe};
use crate::logical::{Cond, SortKey, and_all, pushed_conjuncts, render_cond};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType};

/// A way to read a table: its cost, with and without the sort it may still
/// need, the rows it yields, whether in the order asked for (backward if
/// `reverse`), and the index probe, if it uses one.
struct Path {
    total: f64,
    cost: f64,
    est: f64,
    ordered: bool,
    reverse: bool,
    /// The index probed, if any.
    index: Option<Probed>,
    /// Of a scan, the columns in whose order it reads the table, if any.
    order: Vec<usize>,
}

/// An index probed: its name, its method's, its columns, its probes, one
/// for each range of an OR, the conjuncts they answer exactly, and whether
/// they only bound the rows of others, which a filter still judges.
struct Probed {
    index: String,
    method: String,
    columns: Vec<usize>,
    probes: Vec<Probe>,
    used: Vec<usize>,
    loose: bool,
}

/// Probes an index could answer: its probes, of one range each, the
/// conjuncts they answer exactly, how many of the index's first columns
/// they hold equal, the rows they are guessed to find from the table's
/// statistics, and whether they only bound the rows of a conjunct, which a
/// filter still judges (a LIKE's).
struct Candidate {
    probes: Vec<Probe>,
    used: Vec<usize>,
    fixed: usize,
    guess: f64,
    loose: bool,
}

impl Candidate {
    /// One probe.
    fn one(probe: Probe, (used, fixed, guess, loose): (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate { probes: vec![probe], used, fixed, guess, loose }
    }
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
            let with =
                Filtered { names: read.names, known: Known { stats: None, cost: &cost }, ordered: false, read: 0.0_f64, most: 0.0_f64 };
            return filtered(plan, conds, &with);
        };
        let rows = storage.row_count();
        let best = self.best_path(read, &conds, want);
        // A scan's filter keeps its share of the table's rows, but no more
        // than the scan reads: the rows its zones or chunks may match.
        let (plan, (found, most)) = match best.index {
            None => {
                let order = best.order;
                let plan = Plan::SeqScan { table: read.table.clone(), columns: read.needed.to_vec(), rows, cost: best.cost, order };
                (plan, (cost_of(rows), best.est))
            }
            Some(mut probed) => {
                // `used` indexes `conds`, which `best_path` enumerated; a
                // conjunct two comparisons of it answer is there twice.
                probed.used.sort_unstable();
                probed.used.dedup();
                let mut answered = Vec::new();
                for &ci in probed.used.iter().rev() {
                    if ci < conds.len() {
                        answered.push(conds.remove(ci));
                    }
                }
                answered.reverse();
                (index_scans(read, probed, answered, (best.est, best.cost, best.reverse)), (best.est, best.est))
            }
        };
        let (cost, stats) = (self.db.cost(), self.db.statistics(read.table));
        let known = Known { stats: stats.as_deref(), cost: &cost };
        filtered(plan, conds, &Filtered { names: read.names, known, ordered: best.ordered, read: found, most })
    }

    /// The cheapest way to read the table: a sequential scan, index probes
    /// answering conjuncts of `conds`, or a full ordered index read.
    fn best_path(&self, read: TableRead<'_>, conds: &[Cond], want: &[SortKey]) -> Path {
        let cost = &self.db.cost();
        let sort_penalty = |est: f64, ordered: bool| if !want.is_empty() && !ordered { cost.sort(est) } else { 0.0_f64 };
        let unread = Path { total: 0.0_f64, cost: 0.0_f64, est: 0.0_f64, ordered: false, reverse: false, index: None, order: Vec::new() };
        let (Ok(storage), Some(desc)) = (self.db.storage_of(read.table), self.db.catalog.table(read.table)) else { return unread };
        let (seq_cost, scanned) = scan_priced(&**storage, read.needed, conds);
        let stats = self.db.statistics(read.table);
        let known = Known { stats: stats.as_deref(), cost };
        let all = cost_of(storage.row_count());
        let mut best =
            in_order(&**storage, want, Path { total: seq_cost + sort_penalty(scanned, false), cost: seq_cost, est: scanned, ..unread });

        // RULES.md D15: a probe reads only the rows its conjuncts select, so the
        // other conjuncts would not be evaluated on the rest. When one of them
        // can raise an error it must see every row: probe no index then.
        let (compared, ors) = if conds.iter().any(Cond::can_raise) { (Vec::new(), Vec::new()) } else { comparisons(conds, known) };
        // An index being built is not read until it is (`Database::create_index`).
        for ix in self.db.catalog.indexes(read.table).into_iter().filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (Some(kind), Ok(found), Some(columns)) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), columns) else {
                continue;
            };
            let types: Vec<DataType> = columns.iter().map(|c| c.data_type).collect();
            let mut candidates = probes(&ix.columns, &types, (&compared, &ors), all, known);
            // A full ordered read, only to provide order.
            candidates.push(Candidate::one(Probe::All, (Vec::new(), 0, all, false)));
            for c in candidates {
                let supported = c.probes.iter().all(|p| supports(&**kind, p, (types.len(), !want.is_empty())));
                // Indexes hold no key with a null, so the rows of a probe are
                // all its conjuncts select only if no column it does not test
                // may be null.
                let untested = columns.get(c.probes.first().map_or(0, tested)..).unwrap_or_default();
                // The rows of probes of several ranges come a range at a time.
                let one = c.probes.len() == 1;
                let order = if kind.ordered() && !want.is_empty() && one { index_order(&ix.columns, c.fixed, want) } else { None };
                let all_keys = matches!(c.probes.first(), Some(Probe::All));
                if !supported || untested.iter().any(|col| !col.not_null) || all_keys && order.is_none() {
                    continue;
                }
                let distinct = || self.db.distinct_keys(read.table, &**found);
                let (est, path_cost) = priced((&**found, &distinct), &c, (all, types.len(), cost.index_descent), &**storage);
                let total = path_cost + sort_penalty(est, order.is_some());
                if total < best.total {
                    let (index, method, keys) = (ix.name.clone(), ix.method.clone(), ix.columns.clone());
                    let probed = Some(Probed { index, method, columns: keys, probes: c.probes, used: c.used, loose: c.loose });
                    let ordered = (order.is_some(), order.unwrap_or(false));
                    best = Path { total, cost: path_cost, est, ordered: ordered.0, reverse: ordered.1, index: probed, order: Vec::new() };
                }
            }
        }
        best
    }
}

/// The rows `c`'s probes of `index` are estimated to find, and their cost,
/// of a table of `all` rows in `storage`, the index `width` columns wide,
/// a level of a tree costing `descent`: the index's count of each probe's
/// if cheap; else, for an equality on all its columns, the rows over its
/// distinct keys, `distinct` asks the index for (`State::distinct_keys`);
/// else the guess, shared by the probes. Its pages to find the entries, if
/// it knows, else a tree's levels.
fn priced(
    (index, distinct): (&dyn Index, &dyn Fn() -> Option<f64>),
    c: &Candidate,
    (all, width, descent): (f64, usize, f64),
    storage: &dyn TableStorage,
) -> (f64, f64) {
    let levels = descent * cost_of(storage.row_count().saturating_add(1)).log2();
    let share = c.guess / cost_of(c.probes.len());
    let (mut est, mut found) = (0.0_f64, 0.0_f64);
    for probe in &c.probes {
        let by_keys = || distinct().filter(|_| whole_key(probe, width)).map(|d| all / d.max(1.0_f64));
        est += index.estimate(probe).map(cost_of).or_else(by_keys).unwrap_or(share);
        found += index.probe_pages(probe).map_or(levels, |pages| descent * pages);
    }
    (est, found + est * storage.fetch_cost())
}

/// Whether `probe` holds every column of an index `width` columns wide
/// equal.
fn whole_key(probe: &Probe, width: usize) -> bool {
    match probe {
        Probe::Compare { op, .. } => *op == CompOp::Eq && width == 1,
        Probe::Prefix { equal, then } => then.is_none() && equal.len() == width,
        Probe::All | Probe::Range { .. } => false,
    }
}

/// The index scans of `probed` on the table `read`, answering the
/// conjuncts `answered`, of the rows `est` and the `cost` the planner gave
/// them all, read `reverse` if so: one for each of its probes, the rows of
/// each after those of the one before (an OR's ranges, none overlapping,
/// so each row is found once). Each shows the conjuncts it answers, or its
/// probe, if it answers some only in part, or one of several.
fn index_scans(read: TableRead<'_>, probed: Probed, answered: Vec<Cond>, (est, cost, reverse): (f64, f64, bool)) -> Plan {
    let share = cost_of(probed.probes.len());
    let whole = and_all(answered).filter(|_| !probed.loose && probed.probes.len() == 1).map(|c| render_cond(&c, read.names));
    let Probed { index, method, columns, probes, .. } = probed;
    let mut level: Vec<Plan> = probes
        .into_iter()
        .map(|probe| {
            let detail = whole.clone().unwrap_or_else(|| render_probe(&probe, &columns, read.names));
            let (index, method, table) = (index.clone(), method.clone(), read.table.clone());
            Plan::IndexScan { table, index, method, probe, reverse, detail, est: est / share, cost: cost / share }
        })
        .collect();
    // Appended in pairs, a level at a time, so they are about log2(n) deep
    // (RULES.md R-CF-6).
    while level.len() > 1 {
        let mut pairs = level.into_iter();
        let mut next = Vec::new();
        while let Some(left) = pairs.next() {
            next.push(match pairs.next() {
                Some(right) => Plan::Append { left: Box::new(left), right: Box::new(right) },
                None => left,
            });
        }
        level = next;
    }
    level.pop().unwrap_or(Plan::Values { rows: Vec::new(), detail: String::new() })
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
/// comparisons `compared` and their ORs `ors` answer: the equalities of
/// its first columns, as many as there are, then each comparison of the
/// next column, a lower and an upper bound of it, each OR of it, or none.
/// Values are those an index places as SQL compares them (`coerce`). An
/// index on one column is asked with `Probe::Compare`, which every method
/// answers that supports the operator.
fn probes(
    columns: &[usize],
    types: &[DataType],
    (compared, ors): (&[Compared], &[Alternatives]),
    all: f64,
    known: Known<'_>,
) -> Vec<Candidate> {
    let on = |at: usize| {
        let (column, data_type) = (columns.get(at).copied(), types.get(at));
        compared.iter().filter(move |c| Some(c.column) == column).flat_map(move |c| {
            let each = data_type.and_then(|t| coerce(c.op, &c.value, t)).unwrap_or_default();
            each.into_iter().map(move |(op, value)| (c, op, value))
        })
    };
    let (mut equal, mut used, mut guess) = (Vec::new(), Vec::new(), all);
    while let Some((c, _, value)) = on(equal.len()).find(|(_, op, _)| *op == CompOp::Eq) {
        guess *= estimate::compare((c.column, CompOp::Eq, &value), known);
        equal.push(value);
        used.push(c.at);
    }
    let fixed = equal.len();
    // The conjuncts answered: of the equalities, and of `also` if exactly.
    let with = |also: &[&Compared]| [used.clone(), also.iter().filter(|c| c.exact).map(|c| c.at).collect()].concat();
    let mut out = Vec::new();
    for (c, op, value) in on(fixed).filter(|(_, op, _)| *op != CompOp::Eq || fixed == 0) {
        let guess = guess * estimate::compare((c.column, op, &value), known);
        let probe = if fixed == 0 { Probe::Compare { op, value } } else { Probe::Prefix { equal: equal.clone(), then: Some((op, value)) } };
        out.push(Candidate::one(probe, (with(&[c]), fixed, guess, !c.exact)));
    }
    // [ISO/IEC 9075:1992 §8.3 GR1] a lower and an upper bound of the next
    // column, as of BETWEEN: one range, the keys past the upper not read.
    let bound =
        |below: bool| on(fixed).find(|(_, op, _)| matches!(op, CompOp::Lt | CompOp::Le) == below && *op != CompOp::Eq && *op != CompOp::Ne);
    if let (Some((lo_c, lo_op, lo)), Some((hi_c, hi_op, hi))) = (bound(false), bound(true)) {
        let (a, b) = (estimate::compare((lo_c.column, lo_op, &lo), known), estimate::compare((hi_c.column, hi_op, &hi), known));
        // Of one column, both hold where either does not fail: a + b - 1,
        // if the statistics know; else as if independent.
        let guess = guess * (a + b - 1.0_f64).max(a * b);
        let probe = Probe::Range { equal: equal.clone(), low: Box::new((lo_op, lo)), high: Box::new((hi_op, hi)) };
        out.push(Candidate::one(probe, (with(&[lo_c, hi_c]), fixed, guess, !lo_c.exact || !hi_c.exact)));
    }
    // An OR of the next column's ranges, an IN list among them: a probe of
    // each range.
    for any in ors.iter().filter(|a| Some(a.column) == columns.get(fixed).copied()) {
        let Some(probes) = types.get(fixed).and_then(|t| probes_of_any(&equal, t, any)) else { continue };
        let used = if any.exact { [used.as_slice(), &[any.at]].concat() } else { used.clone() };
        out.push(Candidate { probes, used, fixed, guess: guess * any.share, loose: !any.exact });
    }
    if fixed > 0 {
        let probe = match (equal.as_slice(), columns.len()) {
            ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
            _ => Probe::Prefix { equal, then: None },
        };
        out.push(Candidate::one(probe, (used, fixed, guess, false)));
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
    /// The rows the filter's selectivity is a fraction of: the table's,
    /// for a scan, or those its index probes find.
    read: f64,
    /// The most rows the filter may keep: those the access reads, a scan
    /// only those of the zones or chunks its conditions may match
    /// (`TableStorage::scan_share`).
    most: f64,
}

/// A table access `plan` under a filter for the conjuncts `conds` it does not
/// answer itself, estimated to keep the fraction of the rows that the
/// table's statistics give, no more than the access reads: a scan that
/// skips the rows its conditions cannot match keeps the same rows of the
/// fewer it reads, not a fraction of them again.
fn filtered(plan: Plan, conds: Vec<Cond>, with: &Filtered<'_>) -> Built {
    let plan = match and_all(conds) {
        Some(c) => {
            let est = Some((with.read * estimate::selectivity(&c, with.known)).min(with.most));
            Plan::Filter { detail: render_cond(&c, with.names), cond: c, est, input: Box::new(plan) }
        }
        None => plan,
    };
    Built { plan, ordered: with.ordered }
}
