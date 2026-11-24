//! \[DESIGN\] An index join whose probe fixes a whole unique key, run as
//! nested probes of that key (RULES.md D932). Each row of its left operand
//! finds one row of the table at most, so its probe is read as a point
//! read reads (D881): the index is probed by the whole key with nothing
//! boxed (`Index::key_probe`), and the row found is read of the columns
//! the statement reads alone (`TableStorage::get_listed`) into a row kept
//! for the run. The pair is made in a row kept for the run too, the left
//! row's values moved into it, and the rest of the condition judged of
//! it, as the join judges it (`indexed.rs`).
//!
//! Each probe is told to the table, counted for its index and looked to
//! for a cancel as a probe of the join is, and its rows are not counted
//! among the rows the statement read, as the join's are not. A left row
//! with a null in its key finds nothing and reads nothing, as there. It
//! is taken only of an inner join by the columns of its left operand,
//! each of the type of the key column it is compared with, or a constant
//! that column's type takes unchanged, so each value is one of its
//! column's, and only of an index that is unique, as the read stops at a
//! key's first row (D1040); any other join is the executor's.

use std::sync::Arc;

use crate::catalog::{KeyShape, QualifiedName, is_global_index};
use crate::db::State;
use crate::error::Result;
use crate::exec::entries::Pruning;
use crate::exec::{RangeAt, eval_cond};
use crate::index::{EncodedEntry, Index, Key, KeyOrder, Probe};
use crate::logical::{Cond, Slot, Truth};
use crate::noted::{self, Noted};
use crate::physical::{JoinProbe, Place, Plan, Preserved, ProbeSlot, placed};
use crate::storage::{Condition, Near, Row, RowId};
use crate::value::{CompOp, DataType, Value};

/// An index join by a whole unique key, as nested point reads.
#[derive(Debug, Clone)]
pub(in crate::exec) struct Direct {
    /// The table probed, and what the probe keeps of its index.
    table: Probed,
    /// The left row's columns that are the key's values, in its order.
    keys: Vec<usize>,
    /// The table's columns the statement reads.
    needed: Vec<usize>,
    /// The table's degree, the width of its part of a pair.
    width: usize,
    /// The rest of the condition, over the pair's columns.
    cond: Option<Cond>,
}

/// What a column of a join's left operand gives: a value of a type, or a
/// constant.
#[derive(Debug, Clone)]
pub(in crate::exec) enum Given {
    Typed(DataType),
    Constant(Value),
}

impl Given {
    /// Whether a key column of `data_type` takes what it gives as it is,
    /// as a point read places it: a value of that type, or a constant the
    /// type's cast leaves unchanged.
    fn placed_as_is(&self, data_type: &DataType) -> bool {
        let constant = match self {
            Given::Typed(given) => return given == data_type,
            Given::Constant(constant) => constant,
        };
        let slot = ProbeSlot::of(Place::Key(0), Slot { index: 0, data_type: data_type.clone(), at: 0 });
        let probe = Probe::Compare { op: CompOp::Eq, value: Value::Null };
        let Ok(cast) = placed(&probe, &[slot], std::slice::from_ref(constant)) else { return false };
        let Probe::Compare { value, .. } = &*cast else { return false };
        format!("{value:?}") == format!("{constant:?}")
    }
}

/// What one run of such a join keeps from pair to pair: the key's values
/// and the pair.
#[derive(Default)]
pub(in crate::exec) struct Pairing {
    key: Vec<Value>,
    pair: Row,
    /// What the table keeps from read to read, the row its columns are
    /// read into, and the key's bytes.
    near: Near,
    listed: Row,
    bytes: Vec<u8>,
}

impl Direct {
    /// The join `plan` is, in `st`, if it is an inner index join whose
    /// probe is the whole key of a unique index, by columns of its left
    /// operand the key columns take as they are, `left` what that
    /// operand's columns give.
    pub(in crate::exec) fn of(plan: &Plan, left: &[Given], st: &State) -> Option<Direct> {
        let Plan::IndexJoin {
            outer: Preserved::Inner,
            spatial: None,
            table,
            index,
            probe: JoinProbe::Prefix(keys),
            cond,
            width,
            columns,
            located: false,
            ..
        } = plan
        else {
            return None;
        };
        let types = &st.catalog.table(table)?.columns;
        let ix = st.catalog.index(index)?;
        let same =
            |(&key, &column): (&usize, &usize)| left.get(key).zip(types.get(column)).is_some_and(|(l, r)| l.placed_as_is(&r.data_type));
        // A key that is not unique may have many rows, and the read stops
        // at a key's first (RULES.md D1040).
        if !ix.unique || keys.len() != ix.columns.len() || !keys.iter().zip(&ix.columns).all(same) {
            return None;
        }
        if is_global_index(index) {
            return None;
        }
        let shape = ix.key_shape();
        let found = st.index_of(index).ok()?;
        if shape.is_collated() || shape.has_expressions() || found.key_projection().is_some() || found.key_probe().is_none() {
            return None;
        }
        let mut fetched: Vec<usize> = columns.iter().chain(&shape.reads).copied().collect();
        fetched.sort_unstable();
        fetched.dedup();
        let place = |wanted: &[usize]| wanted.iter().map(|c| fetched.iter().position(|f| f == c)).collect::<Option<Vec<usize>>>();
        let (key, given) = (place(&shape.columns)?, place(columns)?);
        let table = Probed { table: table.clone(), index: index.to_string(), shape, key, given, fetched };
        Some(Direct { table, keys: keys.to_vec(), needed: columns.to_vec(), width: *width, cond: cond.clone() })
    }

    /// The pair of the left row `left`, whose values it takes, and the
    /// table's row of its key, in `pairing`, if `at`'s transaction sees
    /// one and the rest of the condition is true of the pair: whether
    /// there is one.
    ///
    /// # Errors
    ///
    /// The probe's, and the condition's.
    pub(in crate::exec) fn pair(&self, at: &RangeAt<'_>, left: &mut Row, pairing: &mut Pairing) -> Result<bool> {
        let Pairing { key, pair, near, listed, bytes } = pairing;
        key.clear();
        for &k in &self.keys {
            // A null compared finds no entry, and is no read.
            let Some(value) = left.get(k).filter(|v| !v.is_null()) else { return Ok(false) };
            key.push(value.clone());
        }
        if !self.table.read(at, key, (near, listed, bytes))? {
            return Ok(false);
        }
        pair.clear();
        pair.append(left);
        let from = pair.len();
        pair.resize(from.saturating_add(self.width), Value::Null);
        for (&column, &place) in self.needed.iter().zip(&self.table.given) {
            if let (Some(slot), Some(value)) = (pair.get_mut(from.saturating_add(column)), listed.get(place)) {
                slot.clone_from(value);
            }
        }
        match &self.cond {
            Some(cond) => Ok(eval_cond(cond, pair, at.subqueries)? == Truth::True),
            None => Ok(true),
        }
    }
}

impl Pairing {
    /// The pair made, lent.
    pub(in crate::exec) fn row(&mut self) -> &mut Row {
        &mut self.pair
    }
}

/// The table such a join probes: its index's key, of its columns as they
/// are, the columns read of a row found, those the statement reads and
/// the key's, and where the key's and the statement's are among them.
#[derive(Debug, Clone)]
struct Probed {
    table: QualifiedName,
    index: String,
    shape: KeyShape,
    key: Vec<usize>,
    given: Vec<usize>,
    fetched: Vec<usize>,
}

impl Probed {
    /// Reads into `listed` the columns read of the row `at`'s transaction
    /// sees under the whole key `key`: whether it sees one. The read is
    /// told to the table and its search and entries counted for the index
    /// as a probe of the join tells and counts them, and no more: the
    /// rows an index join's probes read are not counted as the
    /// statement's rows read, here as there.
    fn read(&self, at: &RangeAt<'_>, key: &[Value], (near, listed, bytes): (&mut Near, &mut Row, &mut Vec<u8>)) -> Result<bool> {
        let (st, tx) = (at.st, at.tx);
        let (storage, index) = (st.storage_of(&self.table)?, st.index_of(&self.index)?);
        let stop = st.tx.stop_of(tx);
        let looked = || stop.as_deref().map_or(Ok(()), crate::txn::Stop::check);
        looked()?;
        let probe = match key {
            [only] => Probe::Compare { op: CompOp::Eq, value: only.clone() },
            _ => Probe::Prefix { equal: key.to_vec(), then: None },
        };
        let told = Condition { columns: self.shape.columns.clone(), probe };
        storage.read_by_condition(tx, &told)?;
        let notes = noted::noting();
        let (mut found, mut entries, mut fetched) = (false, 0_u64, 0_usize);
        // The row of an entry, if its version seen has the key: whether
        // the next entry is wanted.
        let mut row = |id: RowId, entry: Option<&EncodedEntry<'_>>| {
            looked()?;
            entries = entries.saturating_add(1);
            listed.clear();
            if !storage.get_listed(tx, id, &self.fetched, (&mut *near, &mut *listed))? {
                // A row gone for every reader has its entry taken back (RULES.md D837).
                if let (Some(gone), Some(entry)) = (near.gone.take(), entry) {
                    Pruning::of(index, &st.tx).prune_encoded(Some(gone), (entry.key, entry.types), id);
                }
                return Ok(true);
            }
            fetched = fetched.saturating_add(1);
            found = Key::values_are_of(key, listed, &self.key);
            Ok(!found)
        };
        let read = match index.key_probe().filter(|by| by.whole_key(key, bytes)) {
            Some(by) => by.entries_of(bytes.as_slice(), &mut |entry| row(entry.row, Some(&entry))),
            // A value its column does not place exactly is searched for.
            None => searched(index, &told.probe, &mut |id| row(id, None)),
        };
        noted::add(notes.as_deref(), Noted::Entries, usize::try_from(entries).unwrap_or(usize::MAX));
        noted::add(notes.as_deref(), Noted::Fetched, fetched);
        if let Some(uses) = st.index_uses.get(&self.index) {
            uses.add(1, entries);
        }
        read.map(|()| found)
    }
}

/// Gives `each` the row of each entry `index` finds for `probe`, until it
/// wants no more.
fn searched(index: &Arc<dyn Index>, probe: &Probe, each: &mut dyn FnMut(RowId) -> Result<bool>) -> Result<()> {
    for entry in Arc::clone(index).search(probe, KeyOrder::Ascending) {
        if !each(entry?.1)? {
            break;
        }
    }
    Ok(())
}
