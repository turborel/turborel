//! \[DESIGN\] (RULES.md D1201) A column of states of the set function
//! that keeps a group's value of a column functionally dependent on its
//! grouping columns (`functions/any_value.rs`): a value a group, its
//! first row's of the column, a null too, read of the batch's column in
//! place (`Taken::Kept`). An accumulator a group is given a row made of
//! each value instead, and its groups' rows gathered by a sort of every
//! batch.
//!
//! Every row of a group has a value not distinct from the one kept, so
//! which row gives it shows only as the standard lets it [ISO/IEC
//! 9075-2:2023 §6.7 GR1e]. A group written, merged or finished is as its
//! accumulator writes, merges and finishes it.

use crate::bytes::Reader;
use crate::error::Result;
use crate::exec::batch::Taken;
use crate::logical::AggCall;
use crate::spill::heap_bytes;
use crate::storage::Batch;
use crate::storage::mask::index;
use crate::value::Value;
use crate::value::codec::{put_value, value};

use super::super::Pending;
use super::Boxed;

/// Each group's value, whether it has one yet, and the bytes the values
/// hold apart from themselves.
#[derive(Debug, Default)]
pub(in crate::exec) struct Carried {
    values: Vec<Value>,
    held: Vec<bool>,
    heap: usize,
}

impl Carried {
    /// A column of states of `agg`, if it keeps its group's first row's
    /// value and writes its state, as a value kept apart from its row is
    /// not.
    pub(super) fn of(agg: &AggCall) -> Option<Carried> {
        let acc = agg.func.0.accumulator(agg.result.clone());
        (acc.keeps_first() && acc.write_state(&mut Vec::new())).then(Carried::default)
    }

    /// Adds `more` groups', of no value yet.
    pub(super) fn grow_by(&mut self, more: usize) {
        let len = self.held.len().saturating_add(more);
        self.values.resize(len, Value::Null);
        self.held.resize(len, false);
    }

    /// Room for `more` groups more.
    pub(super) fn reserve(&mut self, more: usize) {
        self.values.reserve(more);
        self.held.reserve(more);
    }

    /// Gives group `g` the value `v`, if it has none yet.
    fn keep(&mut self, g: usize, v: impl FnOnce() -> Value) {
        let Some(held) = self.held.get_mut(g).filter(|held| !**held) else { return };
        *held = true;
        if let Some(slot) = self.values.get_mut(g) {
            *slot = v();
            self.heap = self.heap.saturating_add(heap_bytes(slot));
        }
    }

    /// Gives each group of `pending` that has no value yet its first
    /// row's of the column `take` names of `batch`: whether it could,
    /// false of what is not a column read in place.
    pub(super) fn feed(&mut self, pending: &Pending, batch: &Batch, take: &Taken) -> bool {
        let Taken::Kept(c) = take else { return false };
        for (&r, &g) in pending.read.iter().zip(&pending.groups) {
            self.keep(index(g), || batch.cell(*c, index(r)).to_value());
        }
        true
    }

    /// Gives group `g` group `i`'s value of `other`, if it has none.
    pub(super) fn merge_from(&mut self, g: usize, other: &Carried, i: usize) {
        if other.held.get(i) == Some(&true) {
            self.keep(g, || other.values.get(i).cloned().unwrap_or(Value::Null));
        }
    }

    /// Gives group `g` the value of `state`, as the accumulator writes
    /// it, if it has none: a row of the value, or a null of no row.
    ///
    /// # Errors
    ///
    /// SQLCODE -904 of a state that is no value.
    pub(super) fn merge_written(&mut self, g: usize, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of a group's value");
        let later = value(&mut reader)?;
        reader.finish()?;
        if let Value::Row(fields) = later {
            self.keep(g, || fields.first().cloned().unwrap_or(Value::Null));
        }
        Ok(())
    }

    /// Group `g`'s state, written as its accumulator writes it.
    pub(super) fn write(&self, g: usize, out: &mut Vec<u8>) {
        match self.values.get(g).filter(|_| self.held.get(g) == Some(&true)) {
            Some(v) => put_value(out, &Value::Row(std::slice::from_ref(v).into())),
            None => put_value(out, &Value::Null),
        }
    }

    /// Group `g`'s value, null of none.
    pub(super) fn finish(&self, g: usize) -> Value {
        self.values.get(g).cloned().unwrap_or(Value::Null)
    }

    /// The states of the groups at `order`, in that order.
    pub(super) fn gathered(&self, order: &[u32]) -> Carried {
        let mut gathered = Carried::default();
        gathered.grow_by(order.len());
        for (g, &i) in order.iter().enumerate() {
            gathered.merge_from(g, self, index(i));
        }
        gathered
    }

    /// Keeps the first `len` groups'.
    pub(super) fn truncate(&mut self, len: usize) {
        self.values.truncate(len);
        self.values.shrink_to_fit();
        self.held.truncate(len);
        self.held.shrink_to_fit();
        self.heap = self.values.iter().map(heap_bytes).sum();
    }

    /// The bytes it holds.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let each = size_of::<Value>().saturating_add(size_of::<bool>());
        self.values.capacity().saturating_mul(each).saturating_add(self.heap)
    }

    /// Each group's accumulator, as it would be had it been given the
    /// same rows, of the function and result of `agg`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(super) fn to_boxed(&self, agg: &AggCall) -> Result<Boxed> {
        let mut state = Vec::new();
        let accs = (0..self.held.len()).map(|g| {
            let mut acc = agg.func.0.accumulator(agg.result.clone());
            state.clear();
            self.write(g, &mut state);
            acc.merge(&state).map(|()| acc)
        });
        Ok(Boxed::made(accs.collect::<Result<_>>()?, agg))
    }
}
