//! \[DESIGN\] The index range a direct read reads (RULES.md D930). Its
//! probe is placed with the run's values and its index searched for the
//! entries as the tree holds them (`Index::search_encoded`, D821). Each
//! entry's row is read of the columns the statement reads alone
//! (`TableStorage::get_listed`, D862), into a row kept with the plan,
//! through what its table keeps from read to read (`storage::Near`), and
//! its key compared with the entry's as bytes. The values are then put
//! where the table has them in a row as wide as the table, kept with the
//! plan too, which is lent to what reads the range. So a row read
//! allocates nothing but its values.
//!
//! It records its read, looks for a cancel, counts its entries and takes
//! back those of rows gone for every reader as the index scan does
//! (`scan/encoded.rs`, `entries.rs`), so its rows, errors and conflicts
//! are that scan's. What it needs of its index's definition is found once,
//! as its plan is kept, for a plan is kept for one generation of the
//! definitions. An index, a probe or a scan it does not fit is left to
//! the executor: an index by a sort key, an expression or a projection of
//! its keys, a global one, a probe whose keys must each be checked, a scan
//! that decides a condition of its own or follows each row with its id.

use std::sync::Arc;

use super::{At, Each, Ran};
use crate::catalog::{KeyShape, QualifiedName, is_global_index};
use crate::db::State;
use crate::engine::key::{decode_key, encode_columns};
use crate::error::Result;
use crate::exec::entries::Pruning;
use crate::index::{EncodedEntries, EncodedEntry, Index, Key, KeyOrder, Probe};
use crate::noted::{self, Noted};
use crate::physical::{Plan, ProbeSlot, placed};
use crate::storage::{Condition, Near, Row, TableStorage};
use crate::txn::{Stop, TxId};
use crate::value::Value;

/// An index range read direct, and what it keeps of its index.
#[derive(Debug, Clone)]
pub(super) struct Scan {
    table: QualifiedName,
    index: String,
    probe: Probe,
    slots: Vec<ProbeSlot>,
    reverse: bool,
    /// The index's key, of its columns as they are.
    shape: KeyShape,
    /// The columns read of each row: those the statement reads and the
    /// key's, in order, each once (`entries::Reading::of`).
    fetched: Vec<usize>,
    /// Where each key column's value is among those read.
    key: Vec<usize>,
    /// How many columns the table has.
    width: usize,
    /// Whether the index holds every column read (`Plan::IndexScan`).
    covering: bool,
}

/// What a range read keeps from one run to the next: what its table keeps
/// from read to read, the row its columns are read into, the row lent,
/// as wide as the table, and the key made of a row to compare.
#[derive(Default)]
pub(super) struct Held {
    near: Near,
    listed: Row,
    wide: Row,
    key: Vec<u8>,
}

impl Scan {
    /// The range read the index scan `plan` is, in `st`, if it is one it
    /// reads direct.
    pub(super) fn of(plan: &Plan, st: &State) -> Option<Scan> {
        let Plan::IndexScan { table, index, probe, slots, reverse, columns, covering, located: false, decided: None, .. } = plan else {
            return None;
        };
        if probe.opaque() || is_global_index(index) {
            return None;
        }
        let shape = st.catalog.index(index)?.key_shape();
        if shape.is_collated() || shape.has_expressions() || st.index_of(index).ok()?.key_projection().is_some() {
            return None;
        }
        let mut fetched: Vec<usize> = columns.iter().chain(&shape.reads).copied().collect();
        fetched.sort_unstable();
        fetched.dedup();
        let key = shape.columns.iter().map(|c| fetched.iter().position(|f| f == c)).collect::<Option<Vec<usize>>>()?;
        let width = st.catalog.table(table)?.columns.len();
        Some(Scan {
            table: table.clone(),
            index: index.clone(),
            probe: probe.clone(),
            slots: slots.clone(),
            reverse: *reverse,
            shape,
            fetched,
            key,
            width,
            covering: *covering,
        })
    }

    /// Lends `each` every row `at`'s transaction sees in the range, in the
    /// index's order or reversed, until it wants no more: or says it read
    /// nothing, for a probe or an index it is not read direct by.
    ///
    /// # Errors
    ///
    /// As the index scan, for a value not of its slot's type, the engine's
    /// refusal of the read (SQLCODE -911), a failed read or a cancel; and
    /// `each`'s.
    pub(super) fn run(&self, at: &At<'_>, held: &mut Held, each: Each<'_>) -> Result<Ran> {
        let (st, tx) = (at.st, at.tx);
        let (Ok(storage), Ok(index)) = (st.storage_of(&self.table), st.index_of(&self.index)) else { return Ok(Ran::Unread) };
        let probe = placed(&self.probe, &self.slots, at.values)?;
        // An index naming the region its search reads is searched.
        if self.shape.columns.len() == 1 && index.read_region(&probe).is_some() {
            return Ok(Ran::Unread);
        }
        let order = if self.reverse { KeyOrder::Descending } else { KeyOrder::Ascending };
        let Some(mut found) = Arc::clone(index).search_encoded(&probe, order) else { return Ok(Ran::Unread) };
        // What it read, told as `scan::asked` tells a key of its columns.
        let told = Condition { columns: self.shape.columns.clone(), probe: probe.into_owned() };
        storage.read_by_condition(tx, &told)?;
        let stop = st.tx.stop_of(tx);
        let notes = noted::noting();
        let mut reading = Reading { scan: self, st, storage: &**storage, index, tx, held, counts: [0; 2], entries: 0 };
        let read = reading.rows(&mut *found, stop.as_deref(), each);
        let (entries, [read_entries, fetched]) = (reading.entries, reading.counts);
        noted::add(notes.as_deref(), Noted::Entries, read_entries);
        noted::add(notes.as_deref(), Noted::Fetched, fetched);
        if let Some(uses) = st.index_uses.get(&self.index) {
            uses.add(1, entries);
        }
        if let Some(stop) = stop.as_deref().filter(|_| entries > 0) {
            stop.activity().read(entries);
        }
        read.map(|()| Ran::Done)
    }
}

/// Whether `stop`, if any, lets its statement go on.
fn looked(stop: Option<&Stop>) -> Result<()> {
    stop.map_or(Ok(()), Stop::check)
}

/// One run of a range read: its table, its index, and what it counts.
struct Reading<'s> {
    scan: &'s Scan,
    st: &'s State,
    storage: &'s dyn TableStorage,
    index: &'s Arc<dyn Index>,
    tx: TxId,
    held: &'s mut Held,
    /// The entries read and the rows fetched.
    counts: [usize; 2],
    /// The entries the search gave.
    entries: u64,
}

impl Reading<'_> {
    /// Lends `each` the row of each of `entries` its transaction sees,
    /// until it wants no more. Each entry is counted and looked to for a
    /// cancel as the scan's are (RULES.md D44, D521).
    fn rows(&mut self, entries: &mut dyn EncodedEntries, stop: Option<&Stop>, each: Each<'_>) -> Result<()> {
        loop {
            looked(stop)?;
            let next = entries.next_encoded();
            looked(stop)?;
            let Some(entry) = next else { return Ok(()) };
            let entry = entry?;
            self.entries = self.entries.saturating_add(1);
            if self.row(&entry)? && !each(&mut self.held.wide)? {
                return Ok(());
            }
        }
    }

    /// Reads the row of `entry` into the row lent, if its transaction
    /// sees a version with the entry's key: whether it does. As
    /// `entries::entry_row`, a covering read of a row every reader sees is
    /// made of the entry's key.
    fn row(&mut self, entry: &EncodedEntry<'_>) -> Result<bool> {
        let scan = self.scan;
        self.counts[0] = self.counts[0].saturating_add(1);
        let Held { near, listed, wide, key } = &mut *self.held;
        // A reader that took the row lent leaves none: another is made.
        if wide.len() != scan.width {
            wide.clear();
            wide.resize(scan.width, Value::Null);
        }
        if scan.covering && self.storage.visible_to_all(entry.row) {
            return Ok(placed_in(wide, &scan.shape.columns, decode_key(entry.key, entry.types)?));
        }
        listed.clear();
        if !self.storage.get_listed(self.tx, entry.row, &scan.fetched, (near, listed))? {
            // A row gone for every reader has its entry taken back
            // (RULES.md D837), its key decoded only then.
            if let Some(gone) = near.gone.take() {
                Pruning::of(self.index, &self.st.tx).prune_encoded(Some(gone), (entry.key, entry.types), entry.row);
            }
            return Ok(false);
        }
        self.counts[1] = self.counts[1].saturating_add(1);
        // As `entries::fetch_encoded`: equal bytes are of equal values,
        // and bytes not equal are decoded and compared as values.
        key.clear();
        encode_columns(listed, &scan.key, entry.types, key)?;
        if key.as_slice() != entry.key && !Key(decode_key(entry.key, entry.types)?).is_of(listed, &scan.key) {
            return Ok(false);
        }
        Ok(placed_in(wide, &scan.fetched, listed.drain(..)))
    }
}

/// Puts each of `values` in `wide` at its column of `columns`: true.
fn placed_in(wide: &mut Row, columns: &[usize], values: impl IntoIterator<Item = Value>) -> bool {
    for (&column, value) in columns.iter().zip(values) {
        if let Some(slot) = wide.get_mut(column) {
            *slot = value;
        }
    }
    true
}
