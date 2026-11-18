//! \[EXTENSION\] Indexes do not exist in X3.135-1986 (it has no CREATE INDEX).
//! \[DESIGN\] Index access methods. An `IndexMethod` declares which comparisons
//! and calls it finds rows by, and whether in key order; the planner asks each
//! for an estimate and picks the cheapest supported path. Built in are
//! "btree", a B-link tree for equality and ranges; "hash", a linear hash
//! table for equality; "rtree", an R-link tree for coordinate bounds;
//! "fulltext", an inverted index of words (RULES.md D86); and "json", an
//! inverted index of JSON documents' paths and values; and "adjacency",
//! the edges of a graph's edge table by endpoint. An index's key may be of
//! expressions of its table's columns (`index/expression.rs`).

pub mod adjacency;
mod blink;
pub mod diskann;
mod key;
pub use key::Key;
pub mod expression;
pub mod fulltext;
mod hash;
pub mod hnsw;
pub mod ivf;
pub mod json;
mod nearest;
mod probe;
mod projection;
mod rtree;
mod sparse;
mod stats;
pub(crate) mod vector;

use std::sync::Arc;

pub use adjacency::{ADJACENCY_METHOD, AdjacencyMethod};
pub use blink::BTreeMethod;
pub use diskann::{DISKANN_METHOD, DiskAnnMethod};
pub use fulltext::{FULLTEXT_METHOD, FullTextMethod};
pub use hash::HashMethod;
pub use hnsw::{HNSW_METHOD, HnswMethod};
pub use ivf::{IVF_METHOD, IvfMethod};
pub use json::{JSON_METHOD, JsonMethod};
pub use nearest::{Approximation, FilterHint, NearestQuery, NearestSupport, NeighbourEntries, RowFilter, same_metric};
pub use probe::{ColumnComparison, Predicate, Probe};
pub use projection::KeyProjection;
pub use rtree::RTreeMethod;
pub use sparse::{BLOCK_SIZE_OPTION, QUANTIZATION_OPTION, SPARSE_METHOD, SparseIndex, SparseMethod, VECTOR_TYPE};

use crate::catalog::{IndexDesc, TableDesc};
use crate::error::Result;
use crate::memory::Host;
use crate::spill::{Area, WorkAreas};
use crate::storage::{RowId, TableOption};
use crate::value::{CompOp, DataType};

/// \[DESIGN\] The entries a search finds, (key, row) pairs, read as they
/// are taken (`Index::search`).
pub type Entries = Box<dyn Iterator<Item = Result<(Key, RowId)>> + Send>;

/// \[DESIGN\] The rows with an entry under one key, read as they are taken
/// (`Index::key_rows`).
pub type RowIds = Box<dyn Iterator<Item = Result<RowId>> + Send>;

/// \[DESIGN\] The order an ordered method's search gives its entries in
/// (`IndexMethod::ordered`); a method that is not ordered gives them in
/// its own, whichever is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrder {
    /// Ascending keys, by the [X3.135-1986 §5.11] comparison rules.
    Ascending,
    /// Descending keys.
    Descending,
}

/// \[EXTENSION\] A kind of index: creates `Index`es and says which probes they answer.
///
/// Contract (RULES.md R-SAFE-3; checked for btree and hash by
/// `test/integration/index_contracts.rs`):
/// * `name` is unique among the registered methods, and constant.
/// * `supports_compare(op, types)` is true only if every index it creates
///   on those types answers `Probe::Compare` with `op` exactly, and
///   `supports_prefix(columns, equal, then)` only if every index it creates
///   on `columns` columns answers exactly a `Probe::Prefix` of `equal`
///   values and a comparison by `then`.
/// * `supports_predicate` is true only if every index it creates on the
///   given types returns every match of the registered function, indexed
///   column and argument position. Extra candidates are allowed
///   (RULES.md D34). Support covers every accepted other argument,
///   including null, so prepared executions may replace that value.
/// * `ordered()` is true only if `search` gives its entries in the key
///   order asked (`KeyOrder`), by the [X3.135-1986 §5.11] comparison rules.
/// * `create` returns an empty index on columns of the types given.
/// * `open` returns the index whose `locator` it gave, holding the entries
///   it held at the last `checkpoint`, or `None` if it keeps nothing between
///   runs; the database then makes it anew from its table.
/// * `attach` is called once, as the method is registered, before any other
///   method but `name`; the memory its caches hold it reserves of the
///   `Host`'s budget, giving it back as they are dropped (RULES.md D38).
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait IndexMethod: Send + Sync {
    /// Whether indexes on these types offer distance candidates for this
    /// call's declared semantics: certified ([`NeighbourEntries`]), or
    /// approximate ([`Approximation`]), which only a query asking for an
    /// approximate search reads, of the index's own metric (the query's
    /// `metric`, [`same_metric`]). Runtime requests may still decline.
    /// Default: unsupported.
    fn supports_nearest(&self, _query: &NearestQuery, _columns: &[DataType]) -> NearestSupport {
        NearestSupport::No
    }
    /// \[EXTENSION\] The metric the index `index` describes orders by, of
    /// an approximate method, read only for a call of it (R13.1). None
    /// by default.
    fn metric(&self, _index: &IndexDesc) -> Option<String> {
        None
    }
    /// \[EXTENSION\] Whether the index `index` describes serves a call of
    /// metric `call`, upper case (R13.1). By default, of its own metric
    /// (`metric`, [`same_metric`]); a vector index of a column declared
    /// normalized serves COSINE of a DOT index too.
    fn serves_metric(&self, index: &IndexDesc, call: &str) -> bool {
        self.metric(index).is_some_and(|m| same_metric(&m, call))
    }
    /// The method's name, as `create_index` and `IndexDesc::method` give it.
    fn name(&self) -> &'static str;
    /// Takes what the database registering it provides: its memory budget.
    /// Default: nothing, for a method that holds no cache.
    ///
    /// # Errors
    ///
    /// The method cannot work with it; it is not registered.
    fn attach(&self, _host: &Host) -> Result<()> {
        Ok(())
    }
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
    /// Can a `Probe::Compare` with this operator be answered?
    fn supports(&self, op: CompOp) -> bool;
    /// Whether comparison probes are supported on these column types.
    /// Defaults to `supports`; a method accepting lossy keys may restrict it.
    fn supports_compare(&self, op: CompOp, _columns: &[DataType]) -> bool {
        self.supports(op)
    }
    /// Can an index on `columns` columns answer a `Probe::Prefix` of
    /// `equal` values, then a comparison by `then`, if given? Default: no.
    fn supports_prefix(&self, _columns: usize, _equal: usize, _then: Option<CompOp>) -> bool {
        false
    }
    /// Can an index on `columns` columns answer a `Probe::Range` of `equal`
    /// values, then two bounds of the next column? Default: no.
    fn supports_range(&self, _columns: usize, _equal: usize) -> bool {
        false
    }
    /// \[EXTENSION\] Whether an index on `columns` can find every match
    /// for this function call, for every accepted other argument,
    /// including null. Extra candidates are allowed (RULES.md D34).
    /// Defaults to false. Support must stay fixed for the method's life.
    fn supports_predicate(&self, _predicate: &Predicate, _columns: &[DataType]) -> bool {
        false
    }
    /// Can an index on `columns` columns answer all independent
    /// comparisons, each a key-column position and operator, exactly?
    /// Default: no. Values use the same SQL rules as `Probe::Compare`.
    fn supports_columns(&self, _columns: usize, _comparisons: &[(usize, CompOp)]) -> bool {
        false
    }
    /// Does `search` give its entries in the key order asked (by the
    /// [X3.135-1986 §5.11] comparison rules)? If so, the planner may use it
    /// to satisfy ORDER BY.
    fn ordered(&self) -> bool {
        false
    }
    /// \[DESIGN\] (RULES.md D34) Are the keys `search` gives decoded by
    /// `engine::key::decode_key` from those `encode_key` made of the rows'
    /// values? If so, an index-only scan may give a column's value from
    /// them, of a type the codec restores (`engine::key::restores`).
    /// Default: no.
    fn restores_keys(&self) -> bool {
        false
    }
    /// A new, empty index on columns of `columns`, the types of its key's
    /// values in order.
    ///
    /// # Errors
    ///
    /// The method cannot make the index, as when its storage fails; the
    /// definition that needed it then fails.
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>>;
    /// The index on columns of `columns` this method made before, found by
    /// the `locator` it gave (`Index::locator`), or `None` if it keeps
    /// nothing between runs. Default: `None`.
    ///
    /// # Errors
    ///
    /// The method cannot read the index; the database is not opened.
    fn open(&self, _columns: &[DataType], _locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        Ok(None)
    }
    /// \[EXTENSION\] The options its indexes take (RULES.md D41), set by
    /// `CREATE INDEX ... WITH (...)` and changed by `ALTER INDEX ... SET`;
    /// `create_on` and `open_on` find an index's in its descriptor
    /// (`IndexDesc::options`). Default: none.
    fn index_options(&self) -> Vec<TableOption> {
        Vec::new()
    }
    /// \[EXTENSION\] `create`, for the index `on` describes, of the table
    /// it describes: it may read the index's options and the table's, the
    /// method's among them (RULES.md D41). Default: `create`.
    ///
    /// # Errors
    ///
    /// As `create`.
    fn create_on(&self, columns: &[DataType], _on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        self.create(columns)
    }
    /// \[EXTENSION\] `open`, for the index `on` describes, as `create_on`.
    /// Default: `open`.
    ///
    /// # Errors
    ///
    /// As `open`.
    fn open_on(&self, columns: &[DataType], locator: &[u8], _on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        self.open(columns, locator)
    }
    /// Makes every index's entries durable, no transaction being open.
    /// Default: nothing, for a method that keeps nothing.
    ///
    /// # Errors
    ///
    /// The method cannot write or sync its files; the database is then not
    /// closed cleanly.
    fn checkpoint(&self) -> Result<()> {
        Ok(())
    }
}

/// Methods take `&self`, `search` the index held: an index guards its own
/// state (see storage.rs).
/// Changes are recorded in the coordinator's undo log, so an index needs no
/// transaction support of its own. A change that fails has not been made:
/// the statement fails, and an undo that fails marks the table damaged
/// (RULES.md D19).
///
/// \[DESIGN\] An index holds entries, (key, id) pairs, counted: the same
/// pair may be inserted more than once, and each insert is taken back by
/// one remove. A table whose engine keeps versions of its rows (RULES.md
/// D29) has an entry for each version, so two versions of a row with the
/// same key give one pair twice; when a version is removed, its entry is
/// removed, and the pair stays while another version has it. A search may
/// then find an entry of a version a reader does not see: the reader keeps
/// a row only if the version it sees has the entry's key.
///
/// Contract (RULES.md R-SAFE-3; checked by `test/integration/index_contracts.rs`):
/// * Callers insert a key containing a null only into the index of a
///   UNIQUE NULLS NOT DISTINCT constraint (`IndexDesc::nulls`), and probe
///   only with probes the method supports. No comparison probe selects
///   such a key. `Probe::Predicate` may return one as an extra candidate.
/// * Unless `key_projection` declares the D85 projected-key contract,
///   counts and returned keys refer to the original inserted keys.
///   With a projection they refer to projected keys: `rows`/`key_rows`
///   give projection-equality candidates, and readers recheck original
///   row values. Such an index cannot provide SQL key order or covering reads.
/// * For comparison probes and `Probe::All`, `search` gives exactly the
///   (key, id) pairs inserted more often than removed whose key satisfies
///   the probe by the [X3.135-1986 §5.11]
///   comparison rules (`Probe::All`: every pair), each once, as it is read:
///   holding no more of them in memory than a page's, or a bucket's, so a
///   probe of a whole index costs what is read of it, and the caller
///   looks for a cancel between entries (RULES.md D34, D44). A pair
///   inserted or removed while it reads may or may not be given; one held
///   throughout is given once. An error of its storage is given as an
///   entry is; callers read no further.
/// * For `Probe::Predicate`, `search` returns every matching inserted
///   `(Key, RowId)` pair, each once. It may also return extra candidates,
///   each once. Keys contain inserted values unless `key_projection`
///   explicitly declares their projection. The executor fetches visible row versions and
///   checks the full WHERE condition (RULES.md D34). The function's
///   declaration and the method's support must hold for the index's life.
///   Estimates and searches must never call back into a database or session.
/// * `rows(key)` returns the ids of the pairs with a key equal to `key`
///   (or its declared projection), a null equal to a null, each once;
///   `key_rows(key)` gives the same ids, as they are read,
///   holding no more of them than `search` does, so a key of more rows
///   than memory holds is read whole (RULES.md D34, D38); an error of its
///   storage is given as an id is.
/// * `remove` takes back one insert of that (key, id) pair, using the
///   declared projection when present; removing a pair with none left changes nothing.
/// * `load` is called only on an index that is empty, and nothing else
///   reads or writes it until it returns; it leaves the index as inserting
///   each entry would.
/// * After `discard`, no method but `search`, `nearest`, `rows`, `key_rows` and `estimate` is
///   called, and those only by readers that found the index before.
///   Streams retain the index until they close, independently of catalog
///   snapshots, so its pages cannot be reclaimed while they are read.
/// * `estimate` is only a statistic: results never depend on it. `None` is
///   "not known cheaply", and the planner then estimates from the table's
///   statistics; for `Probe::Predicate` it prices fetching every table row.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait Index: Send + Sync {
    /// Distance candidates under [`NeighbourEntries`]' pruning contract,
    /// or `None` when the request needs an ordinary scan. Default: no
    /// capability. The returned stream retains this index.
    ///
    /// # Errors
    ///
    /// Storage errors, or SQLCODE -918 if its frontier cannot be reserved.
    fn nearest(self: Arc<Self>, _query: &NearestQuery, _budget: &Arc<crate::memory::MemoryBudget>) -> Result<Option<NeighbourEntries>> {
        Ok(None)
    }
    /// \[EXTENSION\] The adjacency it keeps of a graph's edges, read by
    /// path search as `nearest` is by a distance search (RULES.md
    /// D-GRAPH-ADJACENCY); none by default. It may retain the index.
    fn adjacency(self: Arc<Self>) -> Option<Arc<dyn crate::graph::adjacency::Adjacency>> {
        None
    }
    /// \[EXTENSION\] Candidates of an approximate search, its query's
    /// `approximate` given, under [`Approximation`]'s relaxed contract, or
    /// `None` when the request needs an ordinary scan. `filter`, given of
    /// `FilterHint::InFilter`, may be asked of candidates before this
    /// returns, never after ([`RowFilter`]). Default: `nearest`.
    ///
    /// # Errors
    ///
    /// As `nearest`, and those of `filter`.
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        budget: &Arc<crate::memory::MemoryBudget>,
        _filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        self.nearest(query, budget)
    }
    /// \[EXTENSION\] Takes its structure into memory, or lets it go (ALTER
    /// INDEX LOAD, UNLOAD; vector design document §10.6): a hint, changing
    /// nothing it holds. Default: nothing.
    ///
    /// # Errors
    ///
    /// Storage errors, or SQLCODE -918 if the memory cannot be reserved.
    fn reside(&self, _resident: bool) -> Result<()> {
        Ok(())
    }
    /// \[EXTENSION\] A snapshot, taken now, of the statistics a rank of its
    /// column reads (RULES.md D87); none by default. It may retain the index.
    ///
    /// # Errors
    ///
    /// As reading what it fixes.
    fn ranking_statistics(self: Arc<Self>) -> Result<Option<Arc<dyn crate::fulltext::rank::Statistics>>> {
        Ok(None)
    }
    /// A fixed projection of original keys, or none for exact keys.
    /// See [`KeyProjection`] for counts, candidate and recheck contracts.
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        None
    }
    /// Adds an entry of `row` under `key`. Keys containing a null are
    /// inserted only into the index of a UNIQUE NULLS NOT DISTINCT
    /// constraint [ISO/IEC 9075-2:2023 §11.7], to be found by `rows`: a
    /// comparison with null is never true [X3.135-1986 §5.11 GR2], so no
    /// comparison probe matches them. `Probe::Predicate` may return them
    /// as extra candidates (D34).
    ///
    /// # Errors
    ///
    /// The index cannot hold the entry, or its storage fails.
    fn insert(&self, key: Key, row: RowId) -> Result<()>;
    /// \[DESIGN\] As `insert`, and whether the index holds no entry of
    /// another row under `key` once it is in: so a UNIQUE constraint need
    /// not look for one at the statement's end (`db/assignment.rs`). An
    /// entry of another row inserted at once must make one of the two say
    /// not: so the index decides as it inserts, or after. Not when unsure.
    /// Default: `insert`, then `rows` of `key`.
    ///
    /// # Errors
    ///
    /// As `insert`, and `rows`.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key.clone(), row)?;
        Ok(self.rows(&key)?.iter().all(|&r| r == row))
    }
    /// \[DESIGN\] Adds every entry of `entries`, in any order, to the index,
    /// empty, which nothing else reads or writes meanwhile: as `insert`
    /// would, one by one, as the default does; a method may build it whole
    /// instead, sorting the entries in a spill area of `work` as its
    /// `Area` asks (`spill::sorted_records`).
    ///
    /// # Errors
    ///
    /// As `insert`'s; the entries' own; the spill area's.
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        let _ = work;
        for entry in entries {
            let (key, row) = entry?;
            self.insert(key, row)?;
        }
        Ok(())
    }
    /// Takes back one entry of `row` under `key`, if there is one.
    ///
    /// # Errors
    ///
    /// Its storage fails.
    fn remove(&self, key: &Key, row: RowId) -> Result<()>;
    /// The rows with an entry under this key, each once. Used to enforce
    /// UNIQUE [X3.135-1986 §6.4 GR2].
    ///
    /// # Errors
    ///
    /// Its storage fails.
    fn rows(&self, key: &Key) -> Result<Vec<RowId>>;
    /// \[DESIGN\] The rows with an entry under this key, each once, read as
    /// they are taken: a referential search's (`db/integrity/references.rs`),
    /// which may stop at the first. It holds the index, as `search` does.
    /// Default: those `rows` gives, gathered first.
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        match self.rows(key) {
            Ok(found) => Box::new(found.into_iter().map(Ok)),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
    /// How many rows the probe would return, if known cheaply: the
    /// planner's statistics. For `Probe::Predicate`, count all candidates,
    /// including those the full WHERE check will reject (D34).
    fn estimate(&self, probe: &Probe) -> Option<usize>;
    /// \[DESIGN\] How many distinct keys it holds, near enough, if it knows:
    /// the planner's estimate of the rows of an equality on all its
    /// columns, its table's rows over them (RULES.md D35). Asked inside
    /// planning, so it makes no pass over the index: one that must count
    /// them first gives none, and makes that work due (`maintenance_due`),
    /// for the database's maintenance. Default: not known.
    fn distinct_keys(&self) -> Option<f64> {
        None
    }
    /// \[DESIGN\] Whether work is due that the index leaves to the
    /// database's maintenance (`maintain`), as a count of its keys asked
    /// for: asked as planning reads its statistics, so it should be quick.
    /// Default: none.
    fn maintenance_due(&self) -> bool {
        false
    }
    /// \[DESIGN\] Does the work due (`maintenance_due`), on the database's
    /// maintenance thread, or when it is asked to (`Database::maintain`):
    /// whether it did any, for it to be called again. Default: none.
    ///
    /// # Errors
    ///
    /// The index cannot do it; it is tried again later.
    fn maintain(&self) -> Result<bool> {
        Ok(false)
    }
    /// \[DESIGN\] The pages it reads to find the entries `probe` selects, if
    /// it knows: the planner weighs each as one level of a descent
    /// (PLANNER.INDEX_DESCENT_COST). Default: not known, and the planner
    /// takes the levels of a tree over its table's rows.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        None
    }
    /// \[DESIGN\] The pages of a probe's that the buffer pool misses, of
    /// those `probe_pages` counts, if it knows: the planner prices each as
    /// a page its table's fetches miss (`TableStorage::miss_cost`).
    /// Default: not known, and the planner takes them held.
    fn probe_missed(&self) -> Option<f64> {
        None
    }
    /// The entries the probe selects, each (key, row) pair once, read as
    /// they are taken: in `order` if the method is ordered. It holds the
    /// index (`self: Arc<Self>`), so what it gives may outlive the call
    /// that asked, as a cursor's rows do; an error of its storage is one
    /// of the entries.
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries;
    /// What finds this index again when the database is opened
    /// (`IndexMethod::open`), kept in the catalog. Default: nothing.
    fn locator(&self) -> Vec<u8> {
        Vec::new()
    }
    /// The index was dropped: once the last reference to it goes, it is
    /// not read or written again, and may then release its storage. Until
    /// then, readers of earlier snapshots may still search it. Default:
    /// nothing, for an index that holds only memory.
    fn discard(&self) {}
    /// \[EXTENSION\] Options of its table or of it that change in place
    /// changed: `on` holds them now (RULES.md D41), as for
    /// `TableStorage::reconfigure`. Default: nothing.
    fn reconfigure(&self, _on: (&TableDesc, &IndexDesc)) {}
}

/// Every entry `probe` selects in `index`, in ascending key order if its
/// method is ordered: a search read whole, for tests.
#[cfg(test)]
pub(crate) fn searched<I: Index + ?Sized>(index: &Arc<I>, probe: &Probe) -> Result<Vec<(Key, RowId)>> {
    Index::search(Arc::clone(index), probe, KeyOrder::Ascending).collect()
}

/// The name of `BTreeMethod`, which also enforces UNIQUE constraints unless told otherwise.
pub const BTREE_METHOD: &str = "btree";
/// The name of `HashMethod`.
pub const HASH_METHOD: &str = "hash";

/// The name of `RTreeMethod`, for finite Cartesian point and rectangle keys.
pub const RTREE_METHOD: &str = "rtree";
