//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index's probes (`physical/probe.rs`), one, or one of each range of
//! an OR, chosen by cost.

mod columns;
mod geometric;
mod json;
mod keys;
mod predicate;
mod spatial;

use super::estimate::{self, Known};
use super::expression_index::Keyed;
use super::kept::point_probe;
use super::probe::{Alternatives, Compared, comparisons, render_probe, served_by};
use super::shown::Shown;
use super::{Built, CostModel, PhysicalPlanner, Plan, ProbeSlot, TableRead};
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::engine::key::restores;
use crate::error::Result;
use crate::index::{Index, IndexMethod, Probe};
use crate::logical::{Cond, SortKey, and_all, filled_cond, judged_by_sets, pushed_conjuncts, render_cond};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType, Value};

use keys::{index_order, probes};
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
    /// What EXPLAIN shows of the spatial probe priced, if any (RULES.md
    /// D137).
    note: String,
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
    /// Of a spatial probe, what the geometry column's statistics tell of
    /// it (`geometric::Stated`, RULES.md D137).
    stated: Option<geometric::Stated>,
}

impl Candidate {
    /// One probe, of no `?`.
    fn one(probe: Probe, held: (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate::of(vec![probe], held)
    }

    /// Its probes, of no `?`.
    fn of(probes: Vec<Probe>, (used, fixed, guess, loose): (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate { probes, used, fixed, guess, loose, slots: Vec::new(), stated: None }
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
        Path {
            total,
            cost,
            est,
            ordered: order.is_some(),
            reverse: order.unwrap_or(false),
            index: Some(self),
            order: Vec::new(),
            note: String::new(),
        }
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
                most: None,
                located: read.located,
            };
            let cost = self.db.cost();
            let with = Filtered {
                shown: read.shown,
                known: Known { stats: None, cost: &cost },
                ordered: false,
                read: 0.0_f64,
                most: 0.0_f64,
                note: "",
            };
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
                let plan = Plan::SeqScan {
                    table: read.table.clone(),
                    columns,
                    rows,
                    cost: best.cost,
                    order,
                    backward,
                    most: None,
                    located: read.located,
                };
                (plan, (cost_of(rows), best.est))
            }
            Some(mut probed) => {
                let (named, keyed) = (predicate::named(&probed.probes), Keyed::of_read(self.db, read));
                let answered = probed.answered(&mut conds);
                let found = if named { cost_of(rows) } else { best.est };
                (index_scans(keyed.read(read), probed, answered, (best.est, best.cost, best.reverse)), (found, best.est))
            }
        };
        let (cost, stats) = (self.db.cost(), self.db.statistics(read.table));
        let known = Known { stats: stats.as_deref(), cost: &cost };
        filtered(plan, conds, &Filtered { shown: read.shown, known, ordered: best.ordered, read: found, most, note: &best.note })
    }

    /// The cheapest way to read the table: a sequential scan, index probes
    /// answering conjuncts of `conds`, or a full ordered index read. Each
    /// path pays for evaluating the spatial conjuncts on the rows it reads
    /// (`geometric::evaluation`, RULES.md D137).
    fn best_path(&self, read: TableRead<'_>, conds: &[Cond], want: &[SortKey]) -> Path {
        let cost = &self.db.cost();
        let unread = Path {
            total: 0.0_f64,
            cost: 0.0_f64,
            est: 0.0_f64,
            ordered: false,
            reverse: false,
            index: None,
            order: Vec::new(),
            note: String::new(),
        };
        let Some((storage, (desc, indexes, keyed))) = self.planned(read.table) else { return unread };
        let (desc, scanned, conds) = (&*desc, conds, &*keyed.substituted(conds));
        let (stats, all) = (self.db.statistics(read.table), cost_of(storage.row_count()));
        let known = Known { stats: stats.as_deref(), cost };
        let probed = answerable(conds);
        let (compared, ors) = comparisons(probed, known, self.values);
        if let Some(path) = want.is_empty().then(|| self.point_path(read, (probed, &compared), known.cost)).flatten() {
            return path;
        }
        let evaluation = geometric::evaluation(conds, known);
        let (seq_cost, scanned) = scan_priced(&**storage, read.needed, &self.priced(scanned));
        let seq_cost = seq_cost + scanned * evaluation.per_row;
        let scan = Path { total: seq_cost + self.sort_penalty(want, scanned, false), cost: seq_cost, est: scanned, ..unread };
        let mut best = self.scan_path(&**storage, want, scan);
        let mut spatial: Option<geometric::Priced> = None;
        // An index being built is not read until it is (`Database::create_index`).
        for ix in indexes.iter().map(|ix| &**ix).filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (kind, found, types) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), ix.key_types(desc));
            let (Some(kind), Ok(found), Some(columns), Some(types)) = (kind, found, columns, types) else { continue };
            let served = served_by(&compared, ix);
            let alternatives: &[Alternatives] = if ix.is_collated() { &[] } else { &ors };
            let mut candidates = columns::candidates((&ix.columns, &types), (&served, alternatives), (all, known), &**kind);
            candidates.extend(self.called((ix, &types), (probed, conds), (all, known)));
            let on = On { ix, kind: &**kind, found: &**found, types: &types, columns: &columns, storage: &**storage, evaluation };
            for c in candidates {
                let Some((path, priced)) = self.candidate_path(read, &on, c, want) else { continue };
                if priced.is_some_and(|p| spatial.is_none_or(|s| p.probe < s.probe)) {
                    spatial = priced;
                }
                if path.total < best.total {
                    best = path;
                }
            }
        }
        let shown = read.shown.names().is_some();
        best.note = spatial.filter(|_| shown).map(|p| p.text(seq_cost, evaluation)).unwrap_or_default();
        best
    }

    /// The candidates of the calls among `conds` on the index `ix`, of key
    /// `types`, those `probed` answerable: function lookups, spatial
    /// windows and json lookups, then a full ordered read.
    fn called(
        &self,
        (ix, types): (&IndexDesc, &[DataType]),
        (probed, conds): (&[Cond], &[Cond]),
        (all, known): (f64, Known<'_>),
    ) -> Vec<Candidate> {
        let mut candidates = predicate::candidates(probed, ix, self.values, all);
        let against = spatial::Against { ix, types, values: self.values, systems: self.db.catalog.spatial(), known };
        candidates.extend(spatial::candidates(conds, &against, all));
        // A json index's lookups raise what a scan would of the calls it rechecks (`access/json.rs`).
        candidates.extend(json::candidates(conds, ix, types, self.values, all));
        // A full ordered read, only to provide order.
        candidates.push(Candidate::one(Probe::All, (Vec::new(), 0, all, false)));
        candidates
    }

    /// What sorting a path's `est` rows costs, if `want` asks an order the
    /// path does not give.
    fn sort_penalty(&self, want: &[SortKey], est: f64, ordered: bool) -> f64 {
        if !want.is_empty() && !ordered { self.db.cost().sort(est) } else { 0.0_f64 }
    }

    /// The path of the candidate `c` on the index `on`, if it can read the
    /// table for `want`, and of a spatial probe, what EXPLAIN shows of its
    /// price (RULES.md D137).
    fn candidate_path(
        &self,
        read: TableRead<'_>,
        on: &On<'_>,
        c: Candidate,
        want: &[SortKey],
    ) -> Option<(Path, Option<geometric::Priced>)> {
        let (ix, kind, storage) = (on.ix, on.kind, on.storage);
        let supported = c.probes.iter().all(|p| supports(kind, p, (on.types, !want.is_empty())));
        // An index that omits null keys can miss a match when a
        // key column outside the filter is nullable (D34).
        let omitted = omits_rows(ix, on.columns, c.probes.first());
        // The probes of several ranges, an OR's, are of ranges apart
        // and in key order (`probe::probes_of_any`): their rows,
        // appended, are in key order too, read backward in reverse.
        // Collation sort keys and surviving nulls may require a
        // different SQL order, so neither can replace a sort.
        let ordered_index = kind.ordered() && !ix.is_collated() && !predicate::untested_nulls(on.columns, c.probes.first());
        let order = if ordered_index && !want.is_empty() { index_order(&ix.columns, c.fixed, want) } else { None };
        if !supported || omitted || matches!(c.probes.first(), Some(Probe::All)) && order.is_none() {
            return None;
        }
        let (all, cost) = (cost_of(storage.row_count()), self.db.cost());
        let distinct = || self.db.distinct_keys(read.table, on.found);
        let (est, probed_cost) = priced((on.found, &distinct), &c, (all, on.types.len(), cost.index_descent), storage);
        // Of a covering index, the rows its table says every reader
        // sees are made of their entries, not fetched (RULES.md D34).
        let whole = !predicate::named(&c.probes) && !ix.is_collated() && covers(kind, (&ix.columns, on.types), read.needed);
        let visible = whole.then(|| storage.visible_share()).flatten();
        let row = storage.scan_cost(read.needed) / all.max(1.0_f64);
        let saved = |share: f64| est * share * (storage.fetch_cost() - ENTRY_SCAN_ROWS * row).max(0.0_f64);
        let clustering = c.stated.map_or(0.0_f64, |s| s.clustering);
        let clustered = geometric::clustered_saving(est, clustering, (storage.fetch_cost(), row));
        let path_cost = probed_cost - visible.map_or(clustered, saved) + est * on.evaluation.per_row;
        let total = path_cost * order.map_or(1.0_f64, |_| self.read_share(want, est)) + self.sort_penalty(want, est, order.is_some());
        let stated = geometric::Stated::rows(c.stated.as_ref()).is_some();
        let priced = c.stated.is_some().then_some(geometric::Priced { est, stated, clustering, probe: total });
        Some((Probed::of(ix, c, visible.is_some()).path((est, path_cost, total), order), priced))
    }
}

/// An index a candidate probes, and what pricing it needs: the index, its
/// method and its key's types and columns, the table's storage, and what
/// evaluating its spatial conjuncts costs a row read.
struct On<'a> {
    ix: &'a IndexDesc,
    kind: &'a dyn IndexMethod,
    found: &'a dyn Index,
    types: &'a [DataType],
    columns: &'a [&'a ColumnDesc],
    storage: &'a dyn TableStorage,
    evaluation: geometric::Evaluation,
}

impl PhysicalPlanner<'_> {
    /// \[DESIGN\] The share of a path's `est` rows read, if it gives the
    /// order `want` asks: below a FETCH FIRST only as many as it reads
    /// (`PhysicalPlanner::first`), so the path costs that share of its
    /// rows. A path in no order is read whole to be sorted, and one below
    /// WITH TIES is priced whole, its peers' count unknown.
    fn read_share(&self, want: &[SortKey], est: f64) -> f64 {
        let tied = self.fetch.is_some_and(|f| f.ties.is_some());
        let asked = self.first.filter(|_| !want.is_empty() && !tied);
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
            Some(Path {
                total,
                cost: total,
                est: 1.0_f64,
                ordered: false,
                reverse: false,
                index: Some(chosen),
                order: Vec::new(),
                note: String::new(),
            })
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
        // A broad spatial window is estimated by the column's statistics,
        // not by sampling the tree (`geometric::Stated`, RULES.md D137).
        let counted = || index.estimate(probe).map(cost_of);
        est += geometric::Stated::rows(c.stated.as_ref()).or_else(counted).or_else(by_keys).unwrap_or(share);
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
        Probe::All | Probe::Columns(_) | Probe::Range { .. } | Probe::Predicate { .. } => false,
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
                located: read.located,
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
    /// What EXPLAIN shows of a spatial probe priced against the scan
    /// (RULES.md D137), if any, after the condition.
    note: &'a str,
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
            let shown = with.shown.text(|names| render_cond(&c, names));
            let detail = if with.note.is_empty() { shown } else { format!("{shown}  {}", with.note) };
            Plan::Filter { detail, cond: c, est, input: Box::new(plan) }
        }
        None => plan,
    };
    Built { plan, ordered: with.ordered }
}
