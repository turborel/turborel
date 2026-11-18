//! \[EXTENSION\] One `Index` over any of the vector engines (DiskANN,
//! HNSW, IVF), each adapted by its method to `Engine`.
//!
//! Its entries are counted (RULES.md D29): the engines count each
//! (vector, row) pair, so a row's versions of one vector are one entry
//! counted twice, and of two vectors, two. A null is not indexed (the
//! vector design document R10.6), so inserting or removing one changes
//! nothing. Writes go through the engines, each of which orders its
//! changes so a failure leaves none (D19).
//!
//! An approximate search (`Index::approximate`, RULES.md D-APPROX) asks
//! the engine with no lock of this layer held. A filter of the query's
//! condition is called by the engine's search outside its own locks too,
//! as `RowFilter` requires; its first error ends the search and is
//! returned. The candidates are given once a row, nearest first, their
//! memory reserved of the budget; if it or the engine's search memory
//! cannot be had, the search declines and the executor reads the table.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::ask::Ask;
use super::column::{Column, entry_key, projection, refused};
use super::options::{Shared, serves, shared_of, unit_radius};
use crate::bytes::{Put, Reader};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::index::{Entries, Index, Key, KeyOrder, KeyProjection, NearestQuery, NeighbourEntries, Probe, RowFilter};
use crate::memory::MemoryBudget;
use crate::spill::{Area, WorkAreas};
use crate::storage::RowId;
use crate::vector::metric::{EUCLIDEAN_SQUARED, L2_SQUARED};

/// The version of a vector index's locator (format versions stay 1).
const LOCATOR_VERSION: u8 = 1;
/// What a search's candidates reserve their memory as.
const CANDIDATES: &str = "an approximate search's candidates";
/// Bytes a candidate costs, near enough: its distance, key and row.
const CANDIDATE_BYTES: usize = 64;

/// \[EXTENSION\] What a vector index method's engine does, in the terms
/// of `Index`, for `VectorIndex`.
pub(crate) trait Engine: Send + Sync + Sized + 'static {
    /// The method's name.
    const METHOD: &'static str;
    /// The option of its own search list or probes, which once set
    /// searches in place of its TARGET_ACCURACY (`options.rs`).
    const LIST_OPTION: &'static str;
    /// Its meta page, which reopens it.
    fn meta(&self) -> PageId;
    /// Adds one entry of `row` under `vector`.
    fn insert(&self, vector: &[f32], row: u64) -> Result<()>;
    /// Takes back one entry of `row` under `vector`, if there is one.
    fn remove(&self, vector: &[f32], row: u64) -> Result<()>;
    /// The rows with an entry under `vector`, each once.
    fn rows(&self, vector: &[f32]) -> Result<Vec<u64>>;
    /// Builds it, empty, of `entries` (`Index::load`).
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()>;
    /// The row of every entry, once each, read as taken.
    fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send>;
    /// The candidates of `ask` near `query`, (distance, row), nearest
    /// first, of the rows `filter` admits, if given.
    fn search(&self, query: &[f32], ask: &Ask, filter: Option<&mut dyn FnMut(u64) -> bool>) -> Result<Vec<(f32, u64)>>;
    /// Its live entries, if known.
    fn live(&self) -> Option<u64>;
    /// Whether maintenance is due.
    fn maintenance_due(&self) -> bool;
    /// Does the maintenance due, and whether it did any.
    fn maintain(&self) -> Result<bool>;
    /// Writes what it keeps in memory to its pages, for a checkpoint.
    fn flush(&self) -> Result<()>;
    /// Frees its pages: it is dropped, and nothing reads it again.
    fn free(&self) -> Result<()>;
    /// Takes the options of `desc` that change in place.
    fn retune(&self, desc: &IndexDesc);
}

/// An engine and whether its index was dropped: the last reference let
/// go, a dropped one frees its pages.
pub(crate) struct Held<E: Engine> {
    /// The engine.
    pub(crate) engine: E,
    discarded: AtomicBool,
}

impl<E: Engine> Held<E> {
    /// `engine`, not dropped.
    pub(crate) fn new(engine: E) -> Arc<Held<E>> {
        Arc::new(Held { engine, discarded: AtomicBool::new(false) })
    }

    /// Whether its index was dropped.
    pub(crate) fn discarded(&self) -> bool {
        self.discarded.load(Ordering::Acquire)
    }
}

impl<E: Engine> Drop for Held<E> {
    fn drop(&mut self) {
        if self.discarded() {
            // The last reader has let it go. A failure leaks pages only;
            // Drop cannot report it.
            let _ = self.engine.free();
        }
    }
}

/// \[EXTENSION\] An index of a vector index method.
pub(crate) struct VectorIndex<E: Engine> {
    held: Arc<Held<E>>,
    column: Column,
    /// The metric it orders by, upper case.
    metric: String,
    /// The metric its engine measures by: `metric`, or of a column
    /// declared normalized, the one it orders as there (`options.rs`).
    measured: String,
    /// Whether its column is declared normalized.
    normalized: bool,
    /// Its TARGET_ACCURACY, 0 for none (`Shared::accuracy`); changed in
    /// place by ALTER INDEX.
    accuracy: AtomicU8,
    projection: Arc<dyn KeyProjection>,
}

impl<E: Engine> VectorIndex<E> {
    /// An index of `column` ordered by the metric `metric` names and
    /// measured by `measured`, over `held`, its shared options `shared`.
    pub(crate) fn new(held: Arc<Held<E>>, column: Column, (metric, measured): (&str, &str), shared: Shared) -> VectorIndex<E> {
        VectorIndex {
            held,
            column,
            metric: metric.to_owned(),
            measured: measured.to_owned(),
            normalized: shared.normalized,
            accuracy: AtomicU8::new(shared.accuracy.unwrap_or(0)),
            projection: projection(),
        }
    }

    fn engine(&self) -> &E {
        &self.held.engine
    }

    /// The vector of `key`, or none for a null.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for a vector of a norm other than 1 if the column is
    /// declared normalized (R13.1); -900 for a key that is no vector of
    /// the column's shape.
    fn floats(&self, key: &Key) -> Result<Option<Vec<f32>>> {
        let [value] = key.0.as_slice() else { return Err(refused(E::METHOD, "was given a key of more than one column")) };
        let normalized = || crate::vector::vector(value).map(|v| v.normalized);
        if self.normalized && !value.is_null() && !normalized()? {
            let message = format!(
                "[EXTENSION] a {} index declares its column NORMALIZED (R13.1), and takes no vector whose norm is not 1",
                E::METHOD
            );
            return Err(SqlError::new(Code::Data, message));
        }
        self.column.floats(E::METHOD, value)
    }

    /// The radius, of a call of metric `call` and query `q`, in the
    /// terms of the metric the engine measures by: over a column declared
    /// normalized, carried to EUCLIDEAN's (`options::unit_radius`);
    /// EUCLIDEAN's and EUCLIDEAN_SQUARED's differ by a square.
    fn radius(&self, call: &str, radius: f64, q: &[f32]) -> f64 {
        let squared = |m: &str| m.eq_ignore_ascii_case(EUCLIDEAN_SQUARED) || m.eq_ignore_ascii_case(L2_SQUARED);
        let norm = q.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
        if let Some(euclidean) = unit_radius(call, radius, norm).filter(|_| self.normalized) {
            return if squared(&self.measured) { euclidean * euclidean } else { euclidean };
        }
        match (squared(&self.measured), squared(call)) {
            (true, false) => radius.max(0.0_f64) * radius.max(0.0_f64),
            (false, true) => radius.max(0.0_f64).sqrt(),
            (true, true) | (false, false) => radius,
        }
    }

    /// The candidates of an approximate search, or none to decline.
    fn candidates(&self, query: &NearestQuery, filter: Option<&mut dyn RowFilter>) -> Result<Option<Vec<(f32, u64)>>> {
        let (Some(approximation), Some(call)) = (query.approximate.as_ref(), query.metric.as_deref()) else { return Ok(None) };
        let Some(q) = self.column.query(&query.value).filter(|_| serves(&self.metric, self.normalized, call)) else { return Ok(None) };
        let accuracy = Some(self.accuracy.load(Ordering::Relaxed)).filter(|a| *a > 0);
        let mut ask = Ask::of(approximation).of_index(accuracy);
        ask.radius = ask.radius.map(|r| self.radius(call, r, &q));
        let mut failed = None;
        let found = match filter {
            Some(f) => {
                let key = entry_key();
                // After its first error the filter is asked nothing more,
                // and the search, admitting nothing, ends.
                let mut admits = |row: u64| {
                    if failed.is_some() {
                        return false;
                    }
                    match f.admits(&key, row) {
                        Ok(admitted) => admitted,
                        Err(error) => {
                            failed = Some(error);
                            false
                        }
                    }
                };
                self.engine().search(&q, &ask, Some(&mut admits))
            }
            None => self.engine().search(&q, &ask, None),
        };
        if let Some(error) = failed {
            return Err(error);
        }
        match found {
            Ok(found) => Ok(Some(found)),
            // Its memory refused: the scan, budgeted as it is, reads the
            // table instead (D38).
            Err(error) => {
                if error.code == Code::Resources {
                    Ok(None)
                } else {
                    Err(error)
                }
            }
        }
    }
}

impl<E: Engine> Index for VectorIndex<E> {
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        budget: &Arc<MemoryBudget>,
        filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        let Some(mut found) = self.candidates(query, filter)? else { return Ok(None) };
        let mut seen = std::collections::HashSet::with_capacity(found.len());
        found.retain(|(_, row)| seen.insert(*row));
        let Ok(reservation) = budget.reserve(found.len().saturating_mul(CANDIDATE_BYTES), CANDIDATES) else { return Ok(None) };
        let key = entry_key();
        Ok(Some(Box::new(found.into_iter().map(move |(distance, row)| {
            let _held = (&self, &reservation);
            Ok((f64::from(distance), key.clone(), row))
        }))))
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        Some(Arc::clone(&self.projection))
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        match self.floats(&key)? {
            Some(v) => self.engine().insert(&v, row),
            None => Ok(()),
        }
    }
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        // No UNIQUE index of a vector method is made.
        Ok(false)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, _work: (&WorkAreas, Area)) -> Result<()> {
        let mut vectors = entries.filter_map(|entry| match entry.and_then(|(key, row)| Ok((self.floats(&key)?, row))) {
            Ok((Some(v), row)) => Some(Ok((v, row))),
            Ok((None, _)) => None,
            Err(error) => Some(Err(error)),
        });
        self.engine().load(&mut vectors)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        match self.floats(key)? {
            Some(v) => self.engine().remove(&v, row),
            None => Ok(()),
        }
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        match self.floats(key)? {
            Some(v) => self.engine().rows(&v),
            None => Ok(Vec::new()),
        }
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        match probe {
            Probe::All => self.engine().live().and_then(|n| usize::try_from(n).ok()),
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Predicate { .. } => None,
        }
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.engine().live().and_then(|n| u32::try_from(n).ok()).map(f64::from)
    }
    fn maintenance_due(&self) -> bool {
        self.engine().maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.engine().maintain()
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        match probe {
            Probe::All => {
                let rows = self.engine().entries();
                let key = entry_key();
                Box::new(rows.map(move |row| {
                    let _held = &self;
                    row.map(|row| (key.clone(), row))
                }))
            }
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Predicate { .. } => {
                Box::new(std::iter::once(Err(refused(E::METHOD, "answers every entry and approximate searches, no comparison (R3.11)"))))
            }
        }
    }
    fn locator(&self) -> Vec<u8> {
        locator(self.engine().meta(), self.column)
    }
    fn discard(&self) {
        self.held.discarded.store(true, Ordering::Release);
    }
    fn reconfigure(&self, on: (&TableDesc, &IndexDesc)) {
        let group = E::METHOD.to_ascii_uppercase();
        self.accuracy.store(shared_of(on.1, &group, E::LIST_OPTION).accuracy.unwrap_or(0), Ordering::Relaxed);
        self.engine().retune(on.1);
    }
}

/// The locator of the index whose meta page is `meta`, of `column`: its
/// version, the column's format and dimensions, the page.
pub(crate) fn locator(meta: PageId, column: Column) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION, column.element.code()];
    out.put_u32(column.dims);
    out.put_u64(meta.0);
    out
}

/// The meta page `locator` names, of an index of `column`.
///
/// # Errors
///
/// SQLCODE -916 for a locator of another version or column, or malformed.
pub(crate) fn located(locator: &[u8], column: Column) -> Result<PageId> {
    let mut r = Reader::new(locator, "vector index locator");
    if r.u8()? != LOCATOR_VERSION {
        return Err(r.corrupt("its version is unknown"));
    }
    if r.u8()? != column.element.code() || r.u32()? != column.dims {
        return Err(r.corrupt("its column's format or dimensions differ"));
    }
    let meta = PageId(r.u64()?);
    r.finish()?;
    Ok(meta)
}
