//! \[DESIGN\] Access paths: how one table is read, by a sequential scan or
//! an index's probes (`physical/probe.rs`), one, or one of each range of
//! an OR, chosen by cost.

mod columns;
mod filter;
mod geometric;
mod guarded;
mod json;
mod keys;
mod partitioned;
mod predicate;
mod scans;
mod spatial;
mod xml;

use std::sync::Arc;

use super::estimate::Known;
use super::expression_index::Keyed;
use super::kept::point_probe;
use super::probe::{Alternatives, Compared, comparisons, served_by};
use super::{Built, CostModel, PhysicalPlanner, ProbeSlot, TableRead};
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::engine::key::restores;
use crate::error::Result;
use crate::index::{BTREE_METHOD, BTreeMethod, Index, IndexMethod, Probe};
use crate::logical::{Cond, SortKey, and_all, filled_cond, pushed_conjuncts};
use crate::storage::{TableStorage, cost_of};
use crate::value::{CompOp, DataType, Value};

use filter::{Filtered, filtered};
use keys::{index_order, probes};
use predicate::answerable;
pub(crate) use predicate::predicate_call;
use predicate::{omits_rows, supports};
use spatial::Decision;

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
    /// The share of the rows read its windowed spatial conjuncts keep, by
    /// the spatial probe priced, if any (RULES.md D137).
    windowed: Option<f64>,
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
    /// The conjunct the scan decides of each entry, if one
    /// (`Plan::IndexScan`, `access/spatial.rs`).
    decided: Option<Decision>,
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
    fn one(probe: Probe, held: (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate::of(vec![probe], held)
    }

    /// Its probes, of no `?`.
    fn of(probes: Vec<Probe>, (used, fixed, guess, loose): (Vec<usize>, usize, f64, bool)) -> Candidate {
        Candidate { probes, used, fixed, guess, loose, slots: Vec::new(), stated: None }
    }
}

impl Probed {
    /// The index `ix` probed by `c`'s probes, `covering` if so, deciding
    /// a conjunct of each entry if `decided`.
    fn of(ix: &IndexDesc, c: Candidate, (covering, decided): (bool, Option<Decision>)) -> Probed {
        let (index, method, columns) = (ix.name.clone(), ix.method.clone(), ix.columns.clone());
        Probed { index, method, columns, probes: c.probes, slots: c.slots, used: c.used, loose: c.loose, covering, decided }
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
            windowed: None,
        }
    }

    /// Removes WHERE terms fully answered by comparison probes. A function
    /// lookup keeps every term for the check on fetched visible rows (D34),
    /// but the one the scan decides of each entry itself (D85), which it
    /// takes.
    fn answered(&mut self, conds: &mut Vec<Cond>) -> Vec<Cond> {
        if predicate::named(&self.probes) {
            self.used.clear();
            if let Some(decision) = &mut self.decided {
                decision.cond = (decision.at < conds.len()).then(|| conds.remove(decision.at));
            }
            return Vec::new();
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
        // [EXTENSION] a partitioned table's read is its partitions' (RULES.md D156).
        if let Some(built) = self.partitioned(read, &conds, want) {
            return built;
        }
        let Ok(storage) = self.db.storage_of(read.table) else {
            // [DESIGN] A bound table has storage. If not, it is scanned anyway,
            // and the executor reports the missing storage (SQLCODE -910).
            return filter::unstored(read, conds, &self.db.cost());
        };
        self.read_by(read, conds, (want, None), &**storage).0
    }

    /// The cheapest way to read the table: a sequential scan, index probes
    /// answering conjuncts of `conds`, or a full ordered index read. Each
    /// path pays for evaluating the spatial conjuncts on the rows it reads
    /// (`geometric::evaluation`, RULES.md D137), and for checking the XML
    /// queries of the rows an XML lookup does not answer (D106).
    fn best_path(&self, read: TableRead<'_>, (conds, implied): (&[Cond], &[Cond]), want: &[SortKey]) -> Path {
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
            windowed: None,
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
        // RULES.md D106: what checking the XML queries of a row costs.
        let check = xml::check_cost(conds, known.stats, cost.index_descent);
        let scan_total = seq_cost + scanned * check + self.sort_penalty(want, scanned, false);
        let mut best = self.scan_path(&**storage, want, Path { total: scan_total, cost: seq_cost, est: scanned, ..unread });
        let mut spatial: Option<geometric::Priced> = None;
        let asked = Asked { conds, answerable: probed, implied, compared: &compared, ors: &ors, all, known };
        // An index being built is not read until it is (`Database::create_index`).
        for ix in indexes.iter().map(|ix| &**ix).filter(|ix| !self.db.building.contains(&ix.name)) {
            // [DESIGN] An index the state lacks is not a path; neither is one on
            // a column the table lacks.
            let columns: Option<Vec<_>> = ix.columns.iter().map(|&c| desc.columns.get(c)).collect();
            let (kind, found, types) = (self.db.methods.get(&ix.method), self.db.index_of(&ix.name), ix.key_types(desc));
            let (Some(kind), Ok(found), Some(columns), Some(types)) = (kind, found, columns, types) else { continue };
            let on =
                On { ix, kind: &**kind, found: &**found, types: &types, columns: &columns, storage: &**storage, conds, evaluation, check };
            for c in self.candidates_of(ix, (&**kind, found, &types), &asked) {
                let Some((path, priced)) = self.candidate_path(read, &on, c, want) else { continue };
                if priced.is_some_and(|p| spatial.is_none_or(|s| p.probe < s.probe)) {
                    spatial = priced;
                }
                if path.total < best.total {
                    best = path;
                }
            }
        }
        best.windowed = spatial.map(|p| p.est / all.max(1.0_f64));
        let shown = read.shown.names().is_some();
        best.note = spatial.filter(|_| shown).map(|p| p.text(seq_cost, evaluation)).unwrap_or_default();
        best
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
        let ordered_index = kind.ordered() && !ix.is_collated() && !predicate::surviving_nulls(ix, on.columns, c.probes.first());
        let order = if ordered_index && !want.is_empty() { index_order(&ix.columns, c.fixed, want) } else { None };
        if !supported || omitted || matches!(c.probes.first(), Some(Probe::All)) && order.is_none() {
            return None;
        }
        let (all, cost) = (cost_of(storage.row_count()), self.db.cost());
        let distinct = || self.db.distinct_keys(read.table, on.found);
        let most = (ix.method == BTREE_METHOD).then_some(BTreeMethod::MOST_COUNTED);
        let (est, probed_cost) = priced((on.found, &distinct, most), &c, (all, on.types.len(), cost.index_descent), storage);
        // Of a covering index, the rows its table says every reader
        // sees are made of their entries, not fetched (RULES.md D34); nor
        // are the entries a scan decides reading no column of their rows,
        // the decided conjunct's alone among those needed (D85).
        let decided = read.above.and_then(|above| spatial::decision(&c, on.conds, above));
        let entries = decided.as_ref().is_some_and(|d| d.unread && read.needed.iter().all(|&column| column == d.column));
        let whole = !predicate::named(&c.probes) && !ix.is_collated() && covers(kind, (&ix.columns, on.types), read.needed);
        let visible = (whole || entries).then(|| storage.visible_share()).flatten();
        let row = storage.scan_cost(read.needed) / all.max(1.0_f64);
        let saved = |share: f64| est * share * (storage.fetch_cost() - ENTRY_SCAN_ROWS * row).max(0.0_f64);
        let clustering = c.stated.map_or(0.0_f64, |s| s.clustering);
        let clustered = geometric::clustered_saving(est, clustering, (storage.fetch_cost(), row));
        let path_cost = probed_cost - visible.map_or(clustered, saved) + est * on.evaluation.per_row;
        let checked = if xml::answered(&c.probes) { 0.0_f64 } else { est * on.check };
        let total =
            path_cost * order.map_or(1.0_f64, |_| self.read_share(want, est)) + checked + self.sort_penalty(want, est, order.is_some());
        let stated = geometric::Stated::rows(c.stated.as_ref()).is_some();
        let priced = c.stated.is_some().then_some(geometric::Priced { est, stated, clustering, probe: total });
        Some((Probed::of(ix, c, (visible.is_some() && whole, decided)).path((est, path_cost, total), order), priced))
    }
}

/// An index a candidate probes, and what pricing it needs: the index, its
/// method and its key's types and columns, the table's storage, the
/// conjuncts the candidates are of, and what evaluating its spatial
/// conjuncts and checking its XML queries cost a row read.
struct On<'a> {
    ix: &'a IndexDesc,
    kind: &'a dyn IndexMethod,
    found: &'a dyn Index,
    types: &'a [DataType],
    columns: &'a [&'a ColumnDesc],
    storage: &'a dyn TableStorage,
    /// The conjuncts, those a scan may decide of each entry among them
    /// (`access/spatial.rs`).
    conds: &'a [Cond],
    evaluation: geometric::Evaluation,
    /// What checking the XML queries of a row costs (D106).
    check: f64,
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
        let against = spatial::Against { ix, types, values: self.values, systems: self.db.catalog.spatial(), known: asked.known };
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
        let usable =
            |ix: &&IndexDesc| ix_is_point(ix) && !self.db.building.contains(&ix.name) && !crate::catalog::is_global_index(&ix.name);
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
            let chosen = Probed { index, method, columns, probes: vec![probe], slots, used, loose: false, covering, decided: None };
            Some(Path {
                total,
                cost: total,
                est: 1.0_f64,
                ordered: false,
                reverse: false,
                index: Some(chosen),
                order: Vec::new(),
                note: String::new(),
                windowed: None,
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
/// else the guess, shared by the probes. Of a probe of keys an index
/// stopped counting at `most`, either is no fewer (RULES.md D302),
/// as a guess of independent columns may be far fewer where an index's
/// first columns go together. Its pages to find the entries, if
/// it knows, else a tree's levels; and those the pool misses, as the
/// table's fetches' pages missed cost.
fn priced(
    (index, distinct, most): (&dyn Index, &dyn Fn() -> Option<f64>, Option<usize>),
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
        let floor = most.filter(|_| keyed(probe)).map_or(0.0_f64, |most| cost_of(most.saturating_add(1)));
        let guessed = || by_keys().unwrap_or(share).max(floor);
        est += geometric::Stated::rows(c.stated.as_ref()).or_else(counted).unwrap_or_else(guessed);
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

/// Whether `probe` reads the keys of a value or a range of them, which an
/// index counts.
fn keyed(probe: &Probe) -> bool {
    match probe {
        Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } => true,
        Probe::All | Probe::Columns(_) | Probe::Predicate { .. } | Probe::Xml(_) => false,
    }
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
