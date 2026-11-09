//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index's probes (`physical/probe.rs`), one, or one of each range of
//! an OR, chosen by cost.

use super::estimate::{self, Known};
use super::kept::{Place, point_probe};
use super::probe::{Alternatives, Compared, comparisons, probes_of_any, render_probe, served_by};
use super::shown::Shown;
use super::{Built, CostModel, PhysicalPlanner, Plan, ProbeSlot, TableRead};
use crate::catalog::IndexDesc;
use crate::engine::key::restores;
use crate::error::Result;
use crate::index::{Index, IndexMethod, Probe};
use crate::logical::{Cond, SortKey, and_all, filled_cond, judged_by_sets, pushed_conjuncts, render_cond};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType, Value};

/// \[DESIGN\] What a row an index-only scan makes of its entry costs, in
/// rows of a scan of its table: its zone's visibility looked at and the
/// row made of the key, an entry at a time as a search gives them, where
/// a scan reads its rows by batches (RULES.md D34). Measured.
const ENTRY_SCAN_ROWS: f64 = 2.0;

/// \[DESIGN\] The rows from which a table is taken to have more of them than
/// a probe finding one could lose to a scan, so a probe fixing a unique key
/// is chosen unpriced (`point_path`).
const POINT_ROWS: usize = 1024;

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
/// for each range of an OR, the `?`s among the one probe's values if a
/// prepared statement's (`physical/kept.rs`), the conjuncts they answer
/// exactly, whether they only bound the rows of others, which a filter
/// still judges, and whether its key holds every column read (`covers`).
struct Probed {
    index: String,
    method: String,
    columns: Vec<usize>,
    probes: Vec<Probe>,
    slots: Vec<ProbeSlot>,
    used: Vec<usize>,
    loose: bool,
    covering: bool,
}

/// Probes an index could answer: its probes, of one range each, the
/// conjuncts they answer exactly, how many of the index's first columns
/// they hold equal, the rows they are guessed to find from the table's
/// statistics, whether they only bound the rows of a conjunct, which a
/// filter still judges (a LIKE's), and the `?`s among their values.
struct Candidate {
    probes: Vec<Probe>,
    used: Vec<usize>,
    fixed: usize,
    guess: f64,
    loose: bool,
    /// The `?`s among the one probe's values (`physical/kept.rs`).
    slots: Vec<ProbeSlot>,
}

impl Candidate {
    /// One probe, of no `?`.
    fn one(probe: Probe, (used, fixed, guess, loose): (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate { probes: vec![probe], used, fixed, guess, loose, slots: Vec::new() }
    }
}

impl Probed {
    /// The index `ix` probed by `c`'s probes, `covering` if so.
    fn of(ix: &IndexDesc, c: Candidate, covering: bool) -> Probed {
        let (index, method, columns) = (ix.name.clone(), ix.method.clone(), ix.columns.clone());
        Probed { index, method, columns, probes: c.probes, slots: c.slots, used: c.used, loose: c.loose, covering }
    }
}

impl PhysicalPlanner<'_> {
    /// Choose how to read one table: a sequential scan or an index path.
    /// If an order is wanted, unordered paths are charged for a later sort.
    pub(super) fn access(&self, read: TableRead<'_>, mut conds: Vec<Cond>, want: &[SortKey]) -> Built {
        let Ok(storage) = self.db.storage_of(read.table) else {
            // [DESIGN] A bound table has storage. If not, it is scanned anyway,
            // and the executor reports the missing storage (SQLCODE -910).
            let plan = Plan::SeqScan {
                table: read.table.clone(),
                columns: read.needed.to_vec(),
                rows: 0,
                cost: 0.0_f64,
                order: Vec::new(),
                backward: false,
            };
            let cost = self.db.cost();
            let with =
                Filtered { shown: read.shown, known: Known { stats: None, cost: &cost }, ordered: false, read: 0.0_f64, most: 0.0_f64 };
            return filtered(plan, conds, &with);
        };
        let rows = storage.row_count();
        let best = self.best_path(read, &conds, want);
        // A scan's filter keeps its share of the table's rows, but no more
        // than the scan reads: the rows its zones or chunks may match.
        let (plan, (found, most)) = match best.index {
            None => {
                let order = best.order;
                let (columns, backward) = (read.needed.to_vec(), best.reverse);
                let plan = Plan::SeqScan { table: read.table.clone(), columns, rows, cost: best.cost, order, backward };
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
        filtered(plan, conds, &Filtered { shown: read.shown, known, ordered: best.ordered, read: found, most })
    }

    /// The cheapest way to read the table: a sequential scan, index probes
    /// answering conjuncts of `conds`, or a full ordered index read.
    fn best_path(&self, read: TableRead<'_>, conds: &[Cond], want: &[SortKey]) -> Path {
        let cost = &self.db.cost();
        let sort_penalty = |est: f64, ordered: bool| if !want.is_empty() && !ordered { cost.sort(est) } else { 0.0_f64 };
        let unread = Path { total: 0.0_f64, cost: 0.0_f64, est: 0.0_f64, ordered: false, reverse: false, index: None, order: Vec::new() };
        let (Ok(storage), Some(desc)) = (self.db.storage_of(read.table), self.db.catalog.table(read.table)) else { return unread };
        let stats = self.db.statistics(read.table);
        let known = Known { stats: stats.as_deref(), cost };
        let all = cost_of(storage.row_count());
        // RULES.md D15: a probe reads only the rows its conjuncts select, so the
        // other conjuncts would not be evaluated on the rest. When one of them
        // can raise an error it must see every row: probe no index then.
        let answerable: &[Cond] = if conds.iter().any(Cond::can_raise) { &[] } else { conds };
        let (compared, ors) = comparisons(answerable, known, self.values);
        if let Some(path) = want.is_empty().then(|| self.point_path(read, (answerable, &compared), known.cost)).flatten() {
            return path;
        }
        let (seq_cost, scanned) = scan_priced(&**storage, read.needed, &self.priced(conds));
        let mut best =
            in_order(&**storage, want, Path { total: seq_cost + sort_penalty(scanned, false), cost: seq_cost, est: scanned, ..unread });

        // An index being built is not read until it is (`Database::create_index`).
        for ix in self.db.catalog.indexes(read.table).into_iter().filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (Some(kind), Ok(found), Some(columns)) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), columns) else {
                continue;
            };
            let Some(types) = ix.key_types(desc) else { continue };
            let served = served_by(&compared, ix);
            let alternatives: &[Alternatives] = if ix.is_collated() { &[] } else { &ors };
            let mut candidates = probes(&ix.columns, &types, (&served, alternatives), all, known);
            // A full ordered read, only to provide order.
            candidates.push(Candidate::one(Probe::All, (Vec::new(), 0, all, false)));
            for c in candidates {
                let supported = c.probes.iter().all(|p| supports(&**kind, p, (types.len(), !want.is_empty())));
                // Indexes hold no key with a null, so the rows of a probe are
                // all its conjuncts select only if no column it does not test
                // may be null.
                let untested = columns.get(c.probes.first().map_or(0, tested)..).unwrap_or_default();
                // The probes of several ranges, an OR's, are of ranges apart
                // and in key order (`probe::probes_of_any`): their rows,
                // appended, are in key order too, read backward in reverse.
                // A key of sort keys is in the order of no column.
                let ordered_index = kind.ordered() && !ix.is_collated();
                let order = if ordered_index && !want.is_empty() { index_order(&ix.columns, c.fixed, want) } else { None };
                let all_keys = matches!(c.probes.first(), Some(Probe::All));
                if !supported || untested.iter().any(|col| !col.not_null) || all_keys && order.is_none() {
                    continue;
                }
                let distinct = || self.db.distinct_keys(read.table, &**found);
                let (est, probed_cost) = priced((&**found, &distinct), &c, (all, types.len(), cost.index_descent), &**storage);
                // Of a covering index, the rows its table says every reader
                // sees are made of their entries, not fetched (RULES.md D34).
                let whole = !ix.is_collated() && covers(&**kind, (&ix.columns, &types), read.needed);
                let visible = whole.then(|| storage.visible_share()).flatten();
                let entry_cost = ENTRY_SCAN_ROWS * storage.scan_cost(read.needed) / all.max(1.0_f64);
                let saved = |share: f64| est * share * (storage.fetch_cost() - entry_cost).max(0.0_f64);
                let path_cost = probed_cost - visible.map_or(0.0_f64, saved);
                let total = path_cost + sort_penalty(est, order.is_some());
                if total < best.total {
                    let chosen = Some(Probed::of(ix, c, visible.is_some()));
                    let ordered = (order.is_some(), order.unwrap_or(false));
                    best = Path { total, cost: path_cost, est, ordered: ordered.0, reverse: ordered.1, index: chosen, order: Vec::new() };
                }
            }
        }
        best
    }
}

impl PhysicalPlanner<'_> {
    /// `conds`, each `?` filled with its value, if the planner is given
    /// an execution's: what a scan's share is priced by, as a plan made
    /// with the values would price it.
    fn priced(&self, conds: &[Cond]) -> Vec<Cond> {
        let filled = |values: &[Value]| conds.iter().map(|c| filled_cond(c.clone(), values)).collect::<Result<Vec<_>>>().ok();
        self.values.and_then(filled).unwrap_or_else(|| conds.to_vec())
    }

    /// A probe that fixes every column of a unique index, by the values of
    /// this execution, or by a prepared statement's `?`s, filled at each
    /// (`physical/kept.rs`), if there is one among `conds`, whose
    /// comparisons with constants are `compared`. It finds one row at
    /// most, so no other path is priced: not the zones of a scan, nor any
    /// index's count, nor the rest of the rows' statistics.
    fn point_path(&self, read: TableRead<'_>, (conds, compared): (&[Cond], &[Compared]), cost: &CostModel) -> Option<Path> {
        let (storage, desc) = (self.db.storage_of(read.table).ok()?, self.db.catalog.table(read.table)?);
        if storage.row_count() < POINT_ROWS {
            return None;
        }
        let usable = |ix: &&_| ix_is_point(ix) && !self.db.building.contains(&ix.name);
        self.db.catalog.indexes(read.table).into_iter().filter(usable).find_map(|ix| {
            let (kind, found, types) = (self.db.methods.get(&ix.method)?, self.db.index_of(&ix.name).ok()?, ix.key_types(desc)?);
            let served = served_by(compared, ix);
            let (probe, slots, used) = point_probe(conds, &served, (&ix.columns, &types))?;
            if !supports(&**kind, &probe, (types.len(), false)) {
                return None;
            }
            let descent = cost.index_descent * cost_of(storage.row_count().saturating_add(1)).log2();
            let pages = found.probe_pages(&probe).map_or(descent, |pages| cost.index_descent * pages);
            let total = pages + found.probe_missed().unwrap_or(0.0_f64) * storage.miss_cost() + storage.fetch_cost();
            let (index, method, columns) = (ix.name.clone(), ix.method.clone(), ix.columns.clone());
            let covering = covers(&**kind, (&ix.columns, &types), read.needed) && storage.visible_share().is_some();
            let chosen = Probed { index, method, columns, probes: vec![probe], slots, used, loose: false, covering };
            Some(Path { total, cost: total, est: 1.0_f64, ordered: false, reverse: false, index: Some(chosen), order: Vec::new() })
        })
    }
}

/// Whether a probe of all of `ix`'s columns finds one row at most.
pub(super) fn ix_is_point(ix: &IndexDesc) -> bool {
    ix.unique && !ix.is_collated()
}

/// The rows `c`'s probes of `index` are estimated to find, and their cost,
/// of a table of `all` rows in `storage`, the index `width` columns wide,
/// a level of a tree costing `descent`: the index's count of each probe's
/// if cheap; else, for an equality on all its columns, the rows over its
/// distinct keys, `distinct` asks the index for (`State::distinct_keys`);
/// else the guess, shared by the probes. Its pages to find the entries, if
/// it knows, else a tree's levels; and those the pool misses, as the
/// table's fetches' pages missed cost.
fn priced(
    (index, distinct): (&dyn Index, &dyn Fn() -> Option<f64>),
    c: &Candidate,
    (all, width, descent): (f64, usize, f64),
    storage: &dyn TableStorage,
) -> (f64, f64) {
    let levels = descent * cost_of(storage.row_count().saturating_add(1)).log2();
    let missed = index.probe_missed().unwrap_or(0.0_f64) * storage.miss_cost();
    let share = c.guess / cost_of(c.probes.len());
    let (mut est, mut found) = (0.0_f64, 0.0_f64);
    for probe in &c.probes {
        let by_keys = || whole_key(probe, width).then(distinct).flatten().map(|d| all / d.max(1.0_f64));
        est += index.estimate(probe).map(cost_of).or_else(by_keys).unwrap_or(share);
        found += index.probe_pages(probe).map_or(levels, |pages| descent * pages) + missed;
    }
    (est, found + est * storage.fetch_cost())
}

/// Whether an index of `method` on `columns`, of `types`, holds in its
/// key every column of `needed`, each a value the key restores, so an
/// index-only scan may give the rows of its entries (RULES.md D34).
fn covers(method: &dyn IndexMethod, (columns, types): (&[usize], &[DataType]), needed: &[usize]) -> bool {
    let restored = |c: &usize| columns.iter().position(|k| k == c).and_then(|at| types.get(at)).is_some_and(restores);
    method.restores_keys() && needed.iter().all(restored)
}

/// Whether `probe` holds every column of an index `width` columns wide
/// equal.
pub(super) fn whole_key(probe: &Probe, width: usize) -> bool {
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
/// so each row is found once, and in key order, so the rows are in the
/// index's order, or, read backward, the last range's first). Each shows
/// the conjuncts it answers, or its probe, if it answers some only in
/// part, or one of several.
fn index_scans(read: TableRead<'_>, probed: Probed, answered: Vec<Cond>, (est, cost, reverse): (f64, f64, bool)) -> Plan {
    let share = cost_of(probed.probes.len());
    let whole =
        and_all(answered).filter(|_| !probed.loose && probed.probes.len() == 1).map(|c| read.shown.text(|names| render_cond(&c, names)));
    let Probed { index, method, columns, mut probes, mut slots, covering, .. } = probed;
    if reverse {
        probes.reverse();
    }
    let mut level: Vec<Plan> = probes
        .into_iter()
        .map(|probe| {
            let detail = whole.clone().unwrap_or_else(|| read.shown.text(|names| render_probe(&probe, &columns, names)));
            let (index, method, table) = (index.clone(), method.clone(), read.table.clone());
            let needed = read.needed.to_vec();
            // The `?`s are of a probe alone, the one of a point path.
            let slots = std::mem::take(&mut slots);
            Plan::IndexScan {
                table,
                index,
                method,
                probe,
                slots,
                reverse,
                columns: needed,
                detail,
                est: est / share,
                cost: cost / share,
                covering,
            }
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
/// its rows sorted by, if `want` asks for the first of them, all ascending,
/// or, read backward, all descending (`TableStorage::scan_ordered`): no
/// sort, its rows merged as they are read, so its cost is the scan's.
fn in_order(storage: &dyn TableStorage, want: &[SortKey], scan: Path) -> Path {
    let keys: Vec<usize> = want.iter().map(|k| k.column).collect();
    let Some(backward) = want.first().map(|k| k.descending) else { return scan };
    if want.iter().any(|k| k.descending != backward) || !storage.ordered_by().starts_with(&keys) {
        return scan;
    }
    Path { total: scan.cost, ordered: true, reverse: backward, order: keys, ..scan }
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
/// answers that supports the operator. A `?`'s value stands in a probe
/// with its slot noted, so another execution's may take its place
/// (`physical/kept.rs`).
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
            let each = data_type.and_then(|t| c.placed(t)).unwrap_or_default();
            each.into_iter().map(move |(op, value)| (c, op, value))
        })
    };
    let mut held = Held { equal: Vec::new(), used: Vec::new(), slots: Vec::new(), guess: all };
    while let Some((c, _, value)) = on(held.equal.len()).find(|(_, op, _)| *op == CompOp::Eq) {
        held.guess *= estimate::compare((c.column, CompOp::Eq, &value), known);
        held.slots.extend(c.slot_at(Place::Key(held.equal.len())));
        held.equal.push(value);
        held.used.push(c.at);
    }
    let fixed = held.equal.len();
    let mut out: Vec<Candidate> = on(fixed).filter(|(_, op, _)| *op != CompOp::Eq || fixed == 0).map(|b| held.then(b, known)).collect();
    // [ISO/IEC 9075:1992 §8.3 GR1] a lower and an upper bound of the next
    // column, as of BETWEEN: one range, the keys past the upper not read.
    let bound =
        |below: bool| on(fixed).find(|(_, op, _)| matches!(op, CompOp::Lt | CompOp::Le) == below && *op != CompOp::Eq && *op != CompOp::Ne);
    if let (Some(low), Some(high)) = (bound(false), bound(true)) {
        out.push(held.between(low, high, known));
    }
    // An OR of the next column's ranges, an IN list among them: a probe of
    // each range. Of no `?` among the equal values, which each probe holds,
    // and a kept plan's `?`s stand in one probe alone.
    for any in ors.iter().filter(|a| Some(a.column) == columns.get(fixed).copied() && held.slots.is_empty()) {
        let Some(probes) = types.get(fixed).and_then(|t| probes_of_any(&held.equal, t, any)) else { continue };
        let used = if any.exact { [held.used.as_slice(), &[any.at]].concat() } else { held.used.clone() };
        out.push(Candidate { probes, used, fixed, guess: held.guess * any.share, loose: !any.exact, slots: Vec::new() });
    }
    if fixed > 0 {
        out.push(held.prefix(columns.len()));
    }
    out
}

/// The equalities of an index's first columns a probe holds: their values,
/// the conjuncts they answer, the `?`s among them, and the rows they are
/// guessed to keep.
struct Held {
    equal: Vec<Value>,
    used: Vec<usize>,
    slots: Vec<ProbeSlot>,
    guess: f64,
}

/// A comparison of an index's column, placed (`Compared::placed`): the
/// comparison, and the operator and value it is placed as.
type Bound<'c> = (&'c Compared, CompOp, Value);

impl Held {
    /// The conjuncts answered: of the equalities, and of `also` if exactly.
    fn with(&self, also: &[&Compared]) -> Vec<usize> {
        [self.used.clone(), also.iter().filter(|c| c.exact).map(|c| c.at).collect()].concat()
    }

    /// The probe of the equalities, then the comparison `(c, op, value)` of
    /// the next column.
    fn then(&self, (c, op, value): Bound<'_>, known: Known<'_>) -> Candidate {
        let guess = self.guess * estimate::compare((c.column, op, &value), known);
        let (fixed, mut slots) = (self.equal.len(), self.slots.clone());
        slots.extend(c.slot_at(Place::Key(fixed)));
        let probe =
            if fixed == 0 { Probe::Compare { op, value } } else { Probe::Prefix { equal: self.equal.clone(), then: Some((op, value)) } };
        Candidate { slots, ..Candidate::one(probe, (self.with(&[c]), fixed, guess, !c.exact)) }
    }

    /// The probe of the equalities, then of the next column between `low`
    /// and `high`.
    fn between(&self, (lo_c, lo_op, lo): Bound<'_>, (hi_c, hi_op, hi): Bound<'_>, known: Known<'_>) -> Candidate {
        let (a, b) = (estimate::compare((lo_c.column, lo_op, &lo), known), estimate::compare((hi_c.column, hi_op, &hi), known));
        // Of one column, both hold where either does not fail: a + b - 1,
        // if the statistics know; else as if independent.
        let guess = self.guess * (a + b - 1.0_f64).max(a * b);
        let mut slots = self.slots.clone();
        slots.extend(lo_c.slot_at(Place::Low).into_iter().chain(hi_c.slot_at(Place::High)));
        let probe = Probe::Range { equal: self.equal.clone(), low: Box::new((lo_op, lo)), high: Box::new((hi_op, hi)) };
        let fixed = self.equal.len();
        Candidate { slots, ..Candidate::one(probe, (self.with(&[lo_c, hi_c]), fixed, guess, !lo_c.exact || !hi_c.exact)) }
    }

    /// The probe of the equalities alone, of an index `width` columns wide.
    fn prefix(self, width: usize) -> Candidate {
        let fixed = self.equal.len();
        let probe = match (self.equal.as_slice(), width) {
            ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
            _ => Probe::Prefix { equal: self.equal, then: None },
        };
        Candidate { slots: self.slots, ..Candidate::one(probe, (self.used, fixed, self.guess, false)) }
    }
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
    /// How the filter is described (`physical/shown.rs`).
    shown: Shown<'a>,
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
/// fewer it reads, not a fraction of them again. Its long IN lists are
/// judged by sets (`logical::judged_by_sets`).
fn filtered(plan: Plan, conds: Vec<Cond>, with: &Filtered<'_>) -> Built {
    let plan = match and_all(conds) {
        Some(c) => {
            let est = Some((with.read * estimate::selectivity(&c, with.known)).min(with.most));
            let c = judged_by_sets(c);
            Plan::Filter { detail: with.shown.text(|names| render_cond(&c, names)), cond: c, est, input: Box::new(plan) }
        }
        None => plan,
    };
    Built { plan, ordered: with.ordered }
}
