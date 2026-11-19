//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index's probes (`physical/probe.rs`), one, or one of each range of
//! an OR, chosen by cost.

mod columns;
mod guarded;
mod json;
mod predicate;
mod scans;
mod spatial;
mod xml;

use std::sync::Arc;

use super::estimate::{self, Known};
use super::expression_index::Keyed;
use super::kept::{Place, point_probe};
use super::probe::{Alternatives, Compared, comparisons, probes_of_any, served_by};
use super::shown::Shown;
use super::{Built, CostModel, PhysicalPlanner, Plan, ProbeSlot, TableRead};
use crate::catalog::IndexDesc;
use crate::engine::key::restores;
use crate::error::Result;
use crate::index::{Index, IndexMethod, Probe};
use crate::logical::{Cond, SortKey, and_all, filled_cond, judged_by_sets, pushed_conjuncts, render_cond};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType, Value};

use predicate::answerable;
pub(crate) use predicate::predicate_call;
use predicate::{omits_rows, supports};

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

/// One possible index access: its probes, the WHERE terms they serve,
/// the number of leading key columns held equal, the estimated rows and
/// any parameter slots. A loose candidate only narrows the rows to check;
/// LIKE and function calls still need a filter on the fetched rows.
struct Candidate {
    probes: Vec<Probe>,
    used: Vec<usize>,
    fixed: usize,
    guess: f64,
    loose: bool,
    /// The `?`s among the one probe's values (`physical/kept.rs`).
    slots: Vec<ProbeSlot>,
}

/// What a table's paths are asked for: its conjuncts, those a probe of
/// comparisons may answer, their comparisons and ORs, the filters an
/// index may find the rows of, implied (`access/xml.rs`), and its rows as
/// the planner knows them.
struct Asked<'a> {
    /// The conjuncts.
    conds: &'a [Cond],
    /// Those a probe of comparisons may answer (D15).
    answerable: &'a [Cond],
    /// The filters implied.
    implied: &'a [Cond],
    /// The comparisons of `answerable`.
    compared: &'a [Compared],
    /// Their ORs.
    ors: &'a [Alternatives],
    /// The table's rows.
    all: f64,
    /// What is known of them.
    known: Known<'a>,
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

    /// Builds a path with the candidate count, lookup cost and total cost
    /// including any sort. `order` records a usable forward or reverse order.
    fn path(self, (est, cost, total): (f64, f64, f64), order: Option<bool>) -> Path {
        Path { total, cost, est, ordered: order.is_some(), reverse: order.unwrap_or(false), index: Some(self), order: Vec::new() }
    }

    /// Removes WHERE terms fully answered by comparison probes. A function
    /// lookup keeps every term for the check on fetched visible rows (D34).
    fn answered(&mut self, conds: &mut Vec<Cond>) -> Vec<Cond> {
        if predicate::named(&self.probes) {
            self.used.clear();
        }
        // Two comparisons can serve the same WHERE term. Remove its
        // position only once, from the end so earlier positions stay valid.
        self.used.sort_unstable();
        self.used.dedup();
        let mut answered = Vec::new();
        for &ci in self.used.iter().rev() {
            if ci < conds.len() {
                answered.push(conds.remove(ci));
            }
        }
        answered.reverse();
        answered
    }
}

impl PhysicalPlanner<'_> {
    /// Choose how to read one table: a sequential scan or an index path.
    /// If an order is wanted, unordered paths are charged for a later sort.
    pub(super) fn access(&self, read: TableRead<'_>, conds: Vec<Cond>, want: &[SortKey]) -> Built {
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
                most: None,
                located: read.located,
            };
            let cost = self.db.cost();
            let with =
                Filtered { shown: read.shown, known: Known { stats: None, cost: &cost }, ordered: false, read: 0.0_f64, most: 0.0_f64 };
            return filtered(plan, conds, &with);
        };
        self.read_by(read, conds, (want, None), &**storage).0
    }

    /// The cheapest way to read the table: a sequential scan, index probes
    /// answering conjuncts of `conds`, or a full ordered index read.
    fn best_path(&self, read: TableRead<'_>, (conds, implied): (&[Cond], &[Cond]), want: &[SortKey]) -> Path {
        let cost = &self.db.cost();
        let sort_penalty = |est: f64, ordered: bool| if !want.is_empty() && !ordered { cost.sort(est) } else { 0.0_f64 };
        let unread = Path { total: 0.0_f64, cost: 0.0_f64, est: 0.0_f64, ordered: false, reverse: false, index: None, order: Vec::new() };
        let Some((storage, (desc, indexes, keyed))) = self.planned(read.table) else { return unread };
        let (desc, scanned, conds) = (&*desc, conds, &*keyed.substituted(conds));
        let (stats, all) = (self.db.statistics(read.table), cost_of(storage.row_count()));
        let known = Known { stats: stats.as_deref(), cost };
        let probed = answerable(conds);
        let (compared, ors) = comparisons(probed, known, self.values);
        if let Some(path) = want.is_empty().then(|| self.point_path(read, (probed, &compared), known.cost)).flatten() {
            return path;
        }
        let (seq_cost, scanned) = scan_priced(&**storage, read.needed, &self.priced(scanned));
        // RULES.md D106: what checking the XML queries of a row costs.
        let check = xml::check_cost(conds, known.stats, cost.index_descent);
        let scan = Path { total: seq_cost + scanned * check + sort_penalty(scanned, false), cost: seq_cost, est: scanned, ..unread };
        let mut best = self.scan_path(&**storage, want, scan);
        let asked = Asked { conds, answerable: probed, implied, compared: &compared, ors: &ors, all, known };
        // An index being built is not read until it is (`Database::create_index`).
        for ix in indexes.iter().map(|ix| &**ix).filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (kind, found, types) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), ix.key_types(desc));
            let (Some(kind), Ok(found), Some(columns), Some(types)) = (kind, found, columns, types) else { continue };
            for c in self.candidates_of(ix, (&**kind, found, &types), &asked) {
                let supported = c.probes.iter().all(|p| supports(&**kind, p, (&types, !want.is_empty())));
                // An index that omits null keys can miss a match when a
                // key column outside the filter is nullable (D34).
                let omitted = omits_rows(ix, &columns, c.probes.first());
                // The probes of several ranges, an OR's, are of ranges apart
                // and in key order (`probe::probes_of_any`): their rows,
                // appended, are in key order too, read backward in reverse.
                // Collation sort keys and surviving nulls may require a
                // different SQL order, so neither can replace a sort.
                let ordered_index = kind.ordered() && !ix.is_collated() && !predicate::untested_nulls(&columns, c.probes.first());
                let order = if ordered_index && !want.is_empty() { index_order(&ix.columns, c.fixed, want) } else { None };
                if !supported || omitted || matches!(c.probes.first(), Some(Probe::All)) && order.is_none() {
                    continue;
                }
                let distinct = || self.db.distinct_keys(read.table, &**found);
                let (est, probed_cost) = priced((&**found, &distinct), &c, (all, types.len(), cost.index_descent), &**storage);
                // Of a covering index, the rows its table says every reader
                // sees are made of their entries, not fetched (RULES.md D34).
                let whole = !predicate::named(&c.probes) && !ix.is_collated() && covers(&**kind, (&ix.columns, &types), read.needed);
                let visible = whole.then(|| storage.visible_share()).flatten();
                let entry_cost = ENTRY_SCAN_ROWS * storage.scan_cost(read.needed) / all.max(1.0_f64);
                let saved = |share: f64| est * share * (storage.fetch_cost() - entry_cost).max(0.0_f64);
                let path_cost = probed_cost - visible.map_or(0.0_f64, saved);
                let checked = if xml::answered(&c.probes) { 0.0_f64 } else { est * check };
                let total =
                    path_cost * order.map_or(1.0_f64, |_| self.read_share(want, est)) + checked + sort_penalty(est, order.is_some());
                if total < best.total {
                    best = Probed::of(ix, c, visible.is_some()).path((est, path_cost, total), order);
                }
            }
        }
        best
    }
}

impl PhysicalPlanner<'_> {
    /// The probes `ix`, of method `kind`, found as `found`, of key `types`,
    /// may answer what is `asked`: of comparisons, of function calls, of
    /// spatial relations, of XML queries, and a full ordered read, only to
    /// provide order.
    fn candidates_of(
        &self,
        ix: &IndexDesc,
        (kind, found, types): (&dyn IndexMethod, &Arc<dyn Index>, &[DataType]),
        asked: &Asked<'_>,
    ) -> Vec<Candidate> {
        let all = asked.all;
        let served = served_by(asked.compared, ix);
        let alternatives: &[Alternatives] = if ix.is_collated() { &[] } else { asked.ors };
        let mut candidates = columns::candidates((&ix.columns, types), (&served, alternatives), (all, asked.known), kind);
        candidates.extend(predicate::candidates(asked.answerable, ix, self.values, all));
        let against = spatial::Against { ix, types, values: self.values, systems: self.db.catalog.spatial() };
        candidates.extend(spatial::candidates(asked.conds, &against, all));
        // A json index's lookups raise what a scan would of the calls it rechecks (`access/json.rs`).
        candidates.extend(json::candidates(asked.conds, ix, types, self.values, all));
        candidates.extend(xml::candidates((asked.conds, asked.implied), ix, found, all));
        candidates.push(Candidate::one(Probe::All, (Vec::new(), 0, all, false)));
        candidates
    }

    /// \[DESIGN\] The share of a path's `est` rows read, if it gives the
    /// order `want` asks: below a FETCH FIRST only as many as it reads
    /// (`PhysicalPlanner::first`), so the path costs that share of its
    /// rows. A path in no order is read whole to be sorted.
    fn read_share(&self, want: &[SortKey], est: f64) -> f64 {
        let asked = self.first.filter(|_| !want.is_empty());
        asked.map_or(1.0_f64, |n| (cost_of(n) / est.max(1.0_f64)).min(1.0_f64))
    }

    /// `scan`, a sequential scan of `storage`, read in the order `want`
    /// asks if its storage keeps it (`in_order`), and then priced by the
    /// share of it read.
    fn scan_path(&self, storage: &dyn TableStorage, want: &[SortKey], scan: Path) -> Path {
        let scan = in_order(storage, want, scan);
        let total = if scan.ordered { scan.total * self.read_share(want, scan.est) } else { scan.total };
        Path { total, ..scan }
    }

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
            if !supports(&**kind, &probe, (&types, false)) {
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
        Probe::All | Probe::Columns(_) | Probe::Range { .. } | Probe::Predicate { .. } | Probe::Xml(_) => false,
    }
}

/// `scan`, a scan of `storage`, read in the order of the columns it keeps
/// its rows sorted by, if `want` asks for the first of them, all ascending,
/// or, read backward, all descending, each of the null ordering implied,
/// as a table kept in order places its nulls (`SortOrder::implied`)
/// (`TableStorage::scan_ordered`): no sort, its rows merged as they are
/// read, so its cost is the scan's.
fn in_order(storage: &dyn TableStorage, want: &[SortKey], scan: Path) -> Path {
    let keys: Vec<usize> = want.iter().map(|k| k.column).collect();
    let Some(backward) = want.first().map(|k| k.descending) else { return scan };
    let placed = want.iter().all(|k| k.descending == backward && k.order().implied());
    if !placed || !storage.ordered_by().starts_with(&keys) {
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

/// Whether reading an index on `columns` in key order, its first `fixed`
/// held equal by its probe, gives rows in the order of `want`: once the
/// keys of the columns held equal are left out, the rest are the index's
/// next columns, in order, all ascending, or all descending, when it is
/// read backward. Some(backward) if so. Any null ordering is kept, as an
/// index read in key order gives no null in them: its probe is true of no
/// null, and a column it does not test is NOT NULL (`best_path`). Function
/// lookups that may keep nulls are excluded by `best_path` (D34).
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
    /// The row count used to estimate the fraction a filter keeps: the
    /// whole table for a scan or function lookup, else comparison matches.
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
