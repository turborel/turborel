//! \[DESIGN\] A point read through its index's own probe of a whole key
//! (RULES.md D881). The index gives the bytes its entries of the key
//! begin with (`KeyProbe::whole_key`) and lends each entry as it reads it
//! (`KeyProbe::entries_of`), so the read boxes no iterator, holds neither
//! its index nor its table, tells what it read with the probe it holds,
//! moved, and decodes no key but for a row made of one. What its key needs of the index's
//! definition is found once, as its plan is kept. It records its read,
//! looks for a cancel, counts and takes back entries as the search does
//! (`point.rs`, `entries.rs`): an index or a probe it does not fit is
//! searched as before.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use super::PointRead;
use crate::catalog::{KeyShape, is_global_index};
use crate::db::State;
use crate::engine::key::decode_key;
use crate::error::Result;
use crate::exec::entries::Pruning;
use crate::index::{EncodedEntry, Index, Key, Probe};
use crate::noted::{self, Noted};
use crate::storage::{Condition, Near, Row, RowId, TableStorage};
use crate::txn::{Stop, TxId};
use crate::value::{CompOp, Value};

/// What a point read keeps of its index to probe it by a whole key.
#[derive(Debug, Clone)]
pub(super) struct Keyed {
    /// The index's key, of its columns as they are.
    shape: KeyShape,
    /// The columns a read of the row for some columns reads: those and
    /// the key's, in order, each once (`entries::Reading::of`).
    fetched: Vec<usize>,
    /// Of a covering read, how many columns the table has.
    width: Option<usize>,
    /// What its table keeps from one execution's read to the next.
    near: KeptNear,
    /// How its row is read of the columns it gives alone, if it is.
    listed: Option<Listed>,
}

/// \[DESIGN\] (RULES.md D881, D862) A point read's row read as the values
/// of the columns it reads alone (`TableStorage::get_listed`), into a row
/// kept with its plan, with no row of the table's width made: of a read
/// that gives columns of its row, is not covering and gives no row id.
#[derive(Debug, Clone)]
struct Listed {
    /// Where each key column's value is among those read.
    key: Vec<usize>,
    /// Where each column given is among those read, and whether none is
    /// given twice, so each is moved out.
    outputs: Vec<usize>,
    distinct: bool,
}

/// \[DESIGN\] (RULES.md D881) What a kept point read's table keeps from
/// one read by row id to the next (`storage::Near`), held with the plan,
/// so each execution reuses its buffer and what its transaction's first
/// read there found. One execution takes it at a time: another running
/// at once, or under it, reads with one of its own. A copy of the plan
/// starts with none.
#[derive(Default)]
struct KeptNear(Mutex<Kept>);

/// What `KeptNear` keeps: what the table keeps, the row a listed read
/// reads into, and the bytes of the key probed.
type Kept = (Near, Row, Vec<u8>);

impl Clone for KeptNear {
    fn clone(&self) -> KeptNear {
        KeptNear::default()
    }
}

impl std::fmt::Debug for KeptNear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeptNear")
    }
}

impl Keyed {
    /// Of the point read of `index` of `table` for `probe`, reading the
    /// columns `needed`, in `st`: none unless the probe is an equality on
    /// its first columns and the index keys its table's own rows by its
    /// columns as they are, with no sort key, expression or projection,
    /// as `entries::fetch` checks a row in place.
    pub(super) fn of(
        st: &State,
        (table, index): (&crate::catalog::QualifiedName, &str),
        probe: &Probe,
        needed: (&[usize], bool),
        given: Option<(&[usize], bool)>,
    ) -> Option<Keyed> {
        key_of(probe)?;
        let shape = st.catalog.index(index).filter(|_| !is_global_index(index))?.key_shape();
        if shape.is_collated() || shape.has_expressions() || st.index_of(index).ok()?.key_projection().is_some() {
            return None;
        }
        let mut fetched: Vec<usize> = needed.0.iter().chain(&shape.reads).copied().collect();
        fetched.sort_unstable();
        fetched.dedup();
        let width = needed.1.then(|| st.catalog.table(table).map(|t| t.columns.len())).flatten();
        let place = |columns: &[usize]| columns.iter().map(|c| fetched.iter().position(|f| f == c)).collect::<Option<Vec<usize>>>();
        let listed = given
            .filter(|_| !needed.1)
            .and_then(|(outputs, distinct)| Some(Listed { key: place(&shape.columns)?, outputs: place(outputs)?, distinct }));
        Some(Keyed { shape, fetched, width, near: KeptNear::default(), listed })
    }
}

/// The values of the key `probe` asks for by equality: of its first
/// columns, or its first.
fn key_of(probe: &Probe) -> Option<&[Value]> {
    match probe {
        Probe::Prefix { equal, then: None } => Some(equal),
        Probe::Compare { op: CompOp::Eq, value } => Some(std::slice::from_ref(value)),
        Probe::Prefix { then: Some(_), .. }
        | Probe::Compare { .. }
        | Probe::All
        | Probe::Columns(_)
        | Probe::Range { .. }
        | Probe::Predicate { .. }
        | Probe::Xml(_) => None,
    }
}

/// What one probe reads through: its table, its index, and what it
/// counts.
pub(super) struct Probing<'s> {
    st: &'s State,
    keyed: &'s Keyed,
    storage: &'s dyn TableStorage,
    index: &'s Arc<dyn Index>,
    key: &'s [Value],
    tx: TxId,
    near: &'s mut Near,
    /// The row a listed read reads into.
    held: &'s mut Row,
    /// The entries read and the rows fetched.
    counts: [usize; 2],
}

impl Probing<'_> {
    /// Row `entry` names, as its transaction sees it, if that version has
    /// the key probed, read of `columns` or whole: as `entries::fetch`
    /// reads it. The entry's key is the probe's, each value placed exactly
    /// (`KeyProbe::whole_key`), so the row is checked against the probe's
    /// values in place of the entry's, decoded.
    fn fetch(&mut self, entry: &EncodedEntry<'_>, columns: Option<&[usize]>) -> Result<Option<Row>> {
        self.counts[0] = self.counts[0].saturating_add(1);
        let Some(row) = self.storage.get_near(self.tx, entry.row, columns, self.near)? else {
            self.prune(entry);
            return Ok(None);
        };
        self.counts[1] = self.counts[1].saturating_add(1);
        Ok(Key::values_are_of(self.key, &row, &self.keyed.shape.columns).then_some(row))
    }

    /// Takes back `entry`, of a row a read found gone for every reader
    /// (RULES.md D837), its key decoded only then.
    fn prune(&mut self, entry: &EncodedEntry<'_>) {
        if let Some(gone) = self.near.gone.take() {
            Pruning::of(self.index, &self.st.tx).prune_encoded(Some(gone), (entry.key, entry.types), entry.row);
        }
    }

    /// As `fetch`, of the columns it reads alone, as `listed` places
    /// them: the columns it gives, of the row seen, if it has the key.
    fn listed(&mut self, entry: &EncodedEntry<'_>, listed: &Listed) -> Result<Option<Row>> {
        self.counts[0] = self.counts[0].saturating_add(1);
        self.held.clear();
        if !self.storage.get_listed(self.tx, entry.row, &self.keyed.fetched, (self.near, self.held))? {
            self.prune(entry);
            return Ok(None);
        }
        self.counts[1] = self.counts[1].saturating_add(1);
        if !Key::values_are_of(self.key, self.held, &listed.key) {
            return Ok(None);
        }
        let value = |at: &usize| {
            let held = self.held.get_mut(*at);
            let given = if listed.distinct { held.map(|v| std::mem::replace(v, Value::Null)) } else { held.map(|v| v.clone()) };
            given.unwrap_or(Value::Null)
        };
        Ok(Some(listed.outputs.iter().map(value).collect()))
    }

    /// As `entries::entry_row`: of a covering read of a row every reader
    /// sees, the row made of the entry's key; else the row fetched, of
    /// the columns read.
    fn row(&mut self, entry: &EncodedEntry<'_>) -> Result<Option<Row>> {
        if let Some(width) = self.keyed.width
            && self.storage.visible_to_all(entry.row)
        {
            self.counts[0] = self.counts[0].saturating_add(1);
            let mut row = vec![Value::Null; width];
            for (&column, value) in self.keyed.shape.columns.iter().zip(decode_key(entry.key, entry.types)?) {
                if let Some(slot) = row.get_mut(column) {
                    *slot = value;
                }
            }
            return Ok(Some(row));
        }
        self.fetch(entry, Some(&self.keyed.fetched))
    }
}

/// What a point read's probe of a whole key gave.
pub(super) enum Probed<'p> {
    /// Nothing was done: it is not read so, and the caller searches for
    /// its probe, given back.
    Unread(Cow<'p, Probe>),
    /// The row its transaction sees under the key, with its id, if any.
    Read(Option<(RowId, Row)>),
    /// The columns it gives of that row, read alone (`Listed`).
    Given(Option<Row>),
}

/// Whether `stop`, if any, lets its statement go on.
fn looked(stop: Option<&Stop>) -> Result<()> {
    stop.map_or(Ok(()), Stop::check)
}

impl PointRead {
    /// The row under `probe`, its slots placed, read through the index's
    /// probe of a whole key, `whole` or of the columns given: the row and
    /// its id, none if `tx` sees none; or that it is not read so.
    ///
    /// # Errors
    ///
    /// As `read`.
    pub(super) fn keyed<'p>(&self, st: &State, tx: TxId, probe: Cow<'p, Probe>, whole: bool) -> Result<Probed<'p>> {
        let Some(keyed) = &self.keyed else { return Ok(Probed::Unread(probe)) };
        let (Ok(storage), Ok(index)) = (st.storage_of(&self.table), st.index_of(&self.index)) else {
            return Ok(Probed::Unread(probe));
        };
        // An index naming the region its search reads is searched.
        let regional = keyed.shape.columns.len() == 1 && index.read_region(&probe).is_some();
        let Some(by) = index.key_probe().filter(|_| !regional) else { return Ok(Probed::Unread(probe)) };
        // What the plan keeps from read to read: its table's, the row a
        // listed read reads into, and the key's bytes.
        let (mut kept, mut own) = (keyed.near.0.try_lock().ok(), Kept::default());
        let (near, held, bytes) = kept.as_deref_mut().unwrap_or(&mut own);
        if !key_of(&probe).is_some_and(|key| by.whole_key(key, bytes)) {
            return Ok(Probed::Unread(probe));
        }
        // What it read, told with the probe it holds, moved, not copied:
        // the condition `scan::asked` makes of a key of its columns.
        let told = Condition { columns: keyed.shape.columns.clone(), probe: probe.into_owned() };
        storage.read_by_condition(tx, &told)?;
        let key = key_of(&told.probe).unwrap_or_default();
        let stop = st.tx.stop_of(tx);
        looked(stop.as_deref())?;
        let notes = noted::noting();
        let listed = keyed.listed.as_ref().filter(|_| !whole);
        let mut probing = Probing { st, keyed, storage: &**storage, index, key, tx, near, held, counts: [0; 2] };
        let (mut found, mut entries) = (None, 0_u64);
        // Each entry is counted and looked to for a cancel as the search's
        // are (RULES.md D44, D521).
        let read = by.entries_of(bytes.as_slice(), &mut |entry| {
            looked(stop.as_deref())?;
            entries = entries.saturating_add(1);
            let row = match listed {
                Some(listed) => probing.listed(&entry, listed)?,
                None if whole => probing.fetch(&entry, None)?,
                None => probing.row(&entry)?,
            };
            found = row.map(|row| (entry.row, row));
            Ok(found.is_none())
        });
        let [read_entries, fetched] = probing.counts;
        noted::add(notes.as_deref(), Noted::Entries, read_entries);
        noted::add(notes.as_deref(), Noted::Fetched, fetched);
        if let Some(uses) = st.index_uses.get(&self.index) {
            uses.add(1, entries);
        }
        if let Some(stop) = stop.as_deref().filter(|_| entries > 0) {
            stop.activity().read(entries);
        }
        read?;
        if found.is_none() {
            looked(stop.as_deref())?;
        }
        Ok(if listed.is_some() { Probed::Given(found.map(|(_, row)| row)) } else { Probed::Read(found) })
    }
}
