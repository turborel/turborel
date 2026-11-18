//! \[EXTENSION\] An index of the hybrid method: a full-text part of its
//! first column and a vector part of its second (the vector design
//! document §11.1).
//!
//! Its key is the two columns, and it holds keys with a null
//! (`IndexDesc::nulls`): each part holds its own column's value, a null
//! in neither, so a document without an embedding is still found by its
//! words. A change goes to both parts, the first taken back if the
//! second fails, so a failure leaves none (RULES.md D19).
//!
//! Its keys are projected (D85) to one constant key: the parts project
//! their keys each its own way, and a search of one part cannot know the
//! other's key of a row, so a reader takes each row as it sees it, and
//! the executor measures every candidate again on it. A nearest read of
//! the text part, a score of its column, is the part's certified one
//! (D87); an approximate read of the vector part, a distance of its
//! column, the part's approximate one (D94, D100). Each is asked by the
//! position of its column in the key.

use std::collections::HashSet;
use std::sync::Arc;

use crate::error::{Code, Result, SqlError};
use crate::fulltext::rank::Statistics;
use crate::index::{Entries, Index, Key, KeyOrder, KeyProjection, NearestQuery, NeighbourEntries, Probe, RowFilter, RowIds};
use crate::memory::MemoryBudget;
use crate::spill::{Area, WorkAreas};
use crate::storage::RowId;
use crate::value::{Binary, Value};

/// The position in the key of the text column.
pub(super) const TEXT: usize = 0;
/// The position in the key of the vector column.
pub(super) const VECTOR: usize = 1;

fn malformed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a hybrid index's key is a text and a vector")
}

/// The projected key of every entry.
fn projected() -> Key {
    Key(vec![Value::Binary(Binary::new(Vec::new()))])
}

/// Projects every key to one (D85).
struct Projection;

impl KeyProjection for Projection {
    fn project(&self, key: &Key) -> Result<Key> {
        if key.0.len() == 2 { Ok(projected()) } else { Err(malformed()) }
    }
}

/// The values of the text and the vector of `key`.
fn split(key: &Key) -> Result<(&Value, &Value)> {
    let [text, vector] = key.0.as_slice() else { return Err(malformed()) };
    Ok((text, vector))
}

/// `value` as a part's key, unless it is null.
fn single(value: &Value) -> Option<Key> {
    (!value.is_null()).then(|| Key(vec![value.clone()]))
}

/// The entries of `found`, each of the projected key.
fn rekeyed(found: NeighbourEntries) -> NeighbourEntries {
    let key = projected();
    Box::new(found.map(move |entry| entry.map(|(distance, _, row)| (distance, key.clone(), row))))
}

/// A query's filter, asked of a part's candidate as of the projected key.
struct Rekeyed<'f>(&'f mut dyn RowFilter, Key);

impl RowFilter for Rekeyed<'_> {
    fn admits(&mut self, _key: &Key, row: RowId) -> Result<bool> {
        self.0.admits(&self.1, row)
    }
}

/// \[EXTENSION\] An index of the hybrid method.
pub(crate) struct HybridIndex {
    text: Arc<dyn Index>,
    vector: Arc<dyn Index>,
    projection: Arc<dyn KeyProjection>,
}

impl HybridIndex {
    /// The index of parts `text` and `vector`.
    pub(super) fn new(text: Box<dyn Index>, vector: Box<dyn Index>) -> HybridIndex {
        HybridIndex { text: Arc::from(text), vector: Arc::from(vector), projection: Arc::new(Projection) }
    }

    /// `query` as its part of `query.column` asks it: of that part's one
    /// column.
    fn of_part(query: &NearestQuery) -> NearestQuery {
        NearestQuery { column: 0, ..query.clone() }
    }

    /// Every row of a part's entries, once each, of `rows` seen before.
    fn rows_of(part: &Arc<dyn Index>, seen: &mut HashSet<RowId>) -> Result<Vec<RowId>> {
        let mut rows = Vec::new();
        for entry in Arc::clone(part).search(&Probe::All, KeyOrder::Ascending) {
            let (_, row) = entry?;
            if seen.insert(row) {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// Every row either part holds, each once.
    fn every_row(&self) -> Result<Vec<RowId>> {
        let mut seen = HashSet::new();
        let mut rows = Self::rows_of(&self.text, &mut seen)?;
        rows.extend(Self::rows_of(&self.vector, &mut seen)?);
        Ok(rows)
    }
}

impl Index for HybridIndex {
    fn nearest(self: Arc<Self>, query: &NearestQuery, budget: &Arc<MemoryBudget>) -> Result<Option<NeighbourEntries>> {
        if query.column != TEXT {
            return Ok(None);
        }
        Ok(Arc::clone(&self.text).nearest(&Self::of_part(query), budget)?.map(rekeyed))
    }
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        budget: &Arc<MemoryBudget>,
        filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        let part = if query.column == VECTOR { &self.vector } else { &self.text };
        let mut rekeyed_filter = filter.map(|f| Rekeyed(f, projected()));
        let asked = rekeyed_filter.as_mut().map(|f| -> &mut dyn RowFilter { f });
        Ok(Arc::clone(part).approximate(&Self::of_part(query), budget, asked)?.map(rekeyed))
    }
    fn ranking_statistics(self: Arc<Self>) -> Result<Option<Arc<dyn Statistics>>> {
        Arc::clone(&self.text).ranking_statistics()
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        Some(Arc::clone(&self.projection))
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        let (text, vector) = split(&key)?;
        let text = single(text);
        if let Some(t) = &text {
            self.text.insert(t.clone(), row)?;
        }
        let Some(v) = single(vector) else { return Ok(()) };
        let Err(error) = self.vector.insert(v, row) else { return Ok(()) };
        if let Some(t) = &text {
            // The text part's entry taken back, so the failed insert left
            // none; a failure here is the index's damage to report, and the
            // insert's error is the one the statement gives.
            let _ = self.text.remove(t, row);
        }
        Err(error)
    }
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        // No UNIQUE index of this method is made.
        Ok(false)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        // The vector part filled one insert at a time as the text part's
        // load reads its keys: the entries are read once.
        let mut texts = entries.filter_map(|entry| {
            let pair = entry.and_then(|(key, row)| {
                let (text, vector) = split(&key)?;
                if let Some(v) = single(vector) {
                    self.vector.insert(v, row)?;
                }
                Ok(single(text).map(|t| (t, row)))
            });
            pair.transpose()
        });
        self.text.load(&mut texts, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        let (text, vector) = split(key)?;
        let text = single(text);
        if let Some(t) = &text {
            self.text.remove(t, row)?;
        }
        let Some(v) = single(vector) else { return Ok(()) };
        let Err(error) = self.vector.remove(&v, row) else { return Ok(()) };
        if let Some(t) = text {
            // As `insert`: the text part's entry put back.
            let _ = self.text.insert(t, row);
        }
        Err(error)
    }
    /// Of a key, the rows of its text, or of its vector if its text is
    /// null; of the projected key, every row.
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        if key.0.len() != 2 {
            return self.every_row();
        }
        let (text, vector) = split(key)?;
        match (single(text), single(vector)) {
            (Some(t), _) => self.text.rows(&t),
            (None, Some(v)) => self.vector.rows(&v),
            (None, None) => Ok(Vec::new()),
        }
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        match self.rows(key) {
            Ok(found) => Box::new(found.into_iter().map(Ok)),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        match probe {
            Probe::All => self.text.estimate(probe).max(self.vector.estimate(probe)),
            Probe::Predicate { .. } => self.text.estimate(probe),
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } => None,
        }
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.text.distinct_keys()
    }
    fn maintenance_due(&self) -> bool {
        self.text.maintenance_due() || self.vector.maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        let text = self.text.maintain()?;
        Ok(self.vector.maintain()? || text)
    }
    /// Every row once, of the projected key: the rows of the text part,
    /// then those of the vector part not among them. A predicate of the
    /// text column, CONTAINS or TEXT_MATCHES, the text part's candidates
    /// (`HybridMethod::supports_predicate`), each of the projected key.
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let refused = || SqlError::new(Code::Unsupported, "[EXTENSION] the hybrid index method answers no such search");
        let rows = match probe {
            Probe::All => self.every_row(),
            Probe::Predicate { predicate, .. } => {
                if predicate.column != TEXT {
                    return Box::new(std::iter::once(Err(refused())));
                }
                return Box::new(Arc::clone(&self.text).search(probe, order).map(|entry| entry.map(|(_, row)| (projected(), row))));
            }
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } => Err(refused()),
        };
        match rows {
            Ok(rows) => Box::new(rows.into_iter().map(|row| Ok((projected(), row)))),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }
    fn locator(&self) -> Vec<u8> {
        super::options::locator(&self.text.locator(), &self.vector.locator())
    }
    fn discard(&self) {
        self.text.discard();
        self.vector.discard();
    }
    fn reside(&self, resident: bool) -> Result<()> {
        self.vector.reside(resident)
    }
    fn reconfigure(&self, on: (&crate::catalog::TableDesc, &crate::catalog::IndexDesc)) {
        let (text, vector) = super::options::parts(on.1);
        self.text.reconfigure((on.0, &text));
        self.vector.reconfigure((on.0, &vector));
    }
}
