//! \[EXTENSION\] Indexes do not exist in X3.135-1986 (it has no CREATE INDEX).
//! \[DESIGN\] Index access methods. An `IndexMethod` declares its capabilities: which
//! comparison operators it can answer (like PostgreSQL operator classes) and
//! whether it returns rows in key order. The physical planner asks each index
//! for an estimate and picks the cheapest path, so adding a new kind of index
//! never touches the planner.
//!
//! Built in: "btree" (`index/blink.rs`), a B-link tree in pages, ordered,
//! for equality and ranges; "hash" (`index/hash.rs`), a linear hash table
//! in pages, for equality.

mod blink;
mod hash;
mod stats;

use std::cmp::Ordering;

pub use blink::BTreeMethod;
pub use hash::HashMethod;

use crate::catalog::{IndexDesc, TableDesc};
use crate::error::Result;
use crate::memory::Host;
use crate::spill::{Area, WorkAreas};
use crate::storage::RowId;
use crate::storage::TableOption;
use crate::value::{CompOp, DataType, Value, compare};

// ---------------------------------------------------------------------------
// Index keys: ordered tuples of values, by the [X3.135-1986 §5.11] comparison rules
// ---------------------------------------------------------------------------

/// A tuple of column values ordered exactly as SQL compares them, so that
/// 'OPS' and 'OPS ' are the same key and 1.5 sorts after 1.
#[derive(Debug, Clone)]
pub struct Key(pub Vec<Value>);

fn total_cmp(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap_or_else(|| a.is_null().cmp(&b.is_null()).reverse())
}

impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        for (a, b) in self.0.iter().zip(&o.0) {
            let c = total_cmp(a, b);
            if c != Ordering::Equal {
                return c;
            }
        }
        self.0.len().cmp(&o.0.len())
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Key {}

/// What an index is asked for.
#[derive(Debug, Clone)]
pub enum Probe {
    /// Every row with a nonnull key (used to read rows in key order).
    All,
    /// "column `<op>` value", of the key's first column.
    Compare {
        /// The comparison operator.
        op: CompOp,
        /// The constant the column is compared with.
        value: Value,
    },
    /// The keys whose first columns equal `equal`, one value for each, and,
    /// if `then` is given, whose next column compares with its value by
    /// its operator (`IndexMethod::supports_prefix`).
    Prefix {
        /// The values of the key's first columns.
        equal: Vec<Value>,
        /// The comparison of the next column, if any.
        then: Option<(CompOp, Value)>,
    },
    /// The keys whose first columns equal `equal`, one value for each, if
    /// any, and whose next column lies between two bounds: above `low` by
    /// its operator, `>` or `>=`, and below `high` by its, `<` or `<=`; as
    /// BETWEEN gives [ISO/IEC 9075:1992 §8.3 GR1], or two comparisons
    /// (`IndexMethod::supports_range`).
    Range {
        /// The values of the key's first columns.
        equal: Vec<Value>,
        /// The lower bound of the next column, and its operator; boxed, as
        /// are `high`'s, so that a probe, and each plan holding one, is no
        /// larger than another kind of probe makes it.
        low: Box<(CompOp, Value)>,
        /// The upper bound of the next column, and its operator.
        high: Box<(CompOp, Value)>,
    },
}

impl Probe {
    /// Whether a key, its values in order, answers the probe by the
    /// [X3.135-1986 §5.11] comparison rules: never with a null where a
    /// value is compared (GR2).
    pub fn answers<'v>(&self, key: impl IntoIterator<Item = &'v Value>) -> bool {
        let mut key = key.into_iter();
        let holds = |op: CompOp, k: Option<&Value>, v: &Value| k.and_then(|k| compare(k, v)).is_some_and(|o| op.holds(o));
        match self {
            Probe::All => true,
            Probe::Compare { op, value } => holds(*op, key.next(), value),
            Probe::Prefix { equal, then } => {
                equal.iter().all(|v| holds(CompOp::Eq, key.next(), v)) && then.as_ref().is_none_or(|(op, v)| holds(*op, key.next(), v))
            }
            Probe::Range { equal, low, high } => {
                let equals = equal.iter().all(|v| holds(CompOp::Eq, key.next(), v));
                let next = key.next();
                equals && holds(low.0, next, &low.1) && holds(high.0, next, &high.1)
            }
        }
    }
}

/// \[EXTENSION\] A kind of index: creates `Index`es and says which probes they answer.
///
/// Contract (RULES.md R-SAFE-3; checked for btree and hash by
/// `test/integration/index_contracts.rs`):
/// * `name` is unique among the registered methods, and constant.
/// * `supports(op)` is true only if every index it creates answers
///   `Probe::Compare` with `op` exactly (see `Index`), and
///   `supports_prefix(columns, equal, then)` only if every index it creates
///   on `columns` columns answers exactly a `Probe::Prefix` of `equal`
///   values and a comparison by `then`.
/// * `ordered()` is true only if `search` returns ids in ascending key order by
///   the [X3.135-1986 §5.11] comparison rules.
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
    /// Can a `Probe::Compare` with this operator be answered?
    fn supports(&self, op: CompOp) -> bool;
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
    /// Does `search` return row ids in ascending key order (by the [X3.135-1986 §5.11]
    /// comparison rules)? If so, the planner may use it to satisfy ORDER BY.
    fn ordered(&self) -> bool {
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

/// Methods take `&self`: an index guards its own state (see storage.rs).
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
/// * Callers never insert a key containing a null, and probe only with
///   operators the method `supports`.
/// * `search` returns exactly the (key, id) pairs inserted more often than
///   removed whose key satisfies the probe by the [X3.135-1986 §5.11]
///   comparison rules (`Probe::All`: every pair), each once.
/// * `rows(key)` returns the ids of the pairs with a key equal to `key`,
///   each once.
/// * `remove` takes back one insert of that (key, id) pair; removing a pair
///   with none left changes nothing.
/// * `load` is called only on an index that is empty, and nothing else
///   reads or writes it until it returns; it leaves the index as inserting
///   each entry would.
/// * After `discard`, no method but `search`, `rows` and `estimate` is
///   called, and those only by readers that found the index before.
/// * `estimate` is only a statistic: results never depend on it. `None` is
///   "not known cheaply", and the planner then estimates from the table's
///   statistics.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait Index: Send + Sync {
    /// Adds an entry of `row` under `key`. Keys containing a null are never
    /// inserted: a comparison with null is never true [X3.135-1986 §5.11
    /// GR2], so no probe could match them anyway.
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
    /// How many rows the probe would return, if known cheaply: the
    /// planner's statistics.
    fn estimate(&self, probe: &Probe) -> Option<usize>;
    /// \[DESIGN\] How many distinct keys it holds, near enough, if it knows:
    /// the planner's estimate of the rows of an equality on all its
    /// columns, its table's rows over them (RULES.md D35). Default: not
    /// known.
    fn distinct_keys(&self) -> Option<f64> {
        None
    }
    /// \[DESIGN\] The pages it reads to find the entries `probe` selects, if
    /// it knows: the planner weighs each as one level of a descent
    /// (PLANNER.INDEX_DESCENT_COST). Default: not known, and the planner
    /// takes the levels of a tree over its table's rows.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        None
    }
    /// The entries the probe selects, each (key, row) pair once.
    ///
    /// # Errors
    ///
    /// Its storage fails.
    fn search(&self, probe: &Probe) -> Result<Vec<(Key, RowId)>>;
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

/// The name of `BTreeMethod`, which also enforces UNIQUE constraints unless told otherwise.
pub const BTREE_METHOD: &str = "btree";
/// The name of `HashMethod`.
pub const HASH_METHOD: &str = "hash";
