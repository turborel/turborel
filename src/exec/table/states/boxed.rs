//! \[DESIGN\] (RULES.md D356) States of set functions not kept in columns:
//! an accumulator a group, as `exec/group.rs` keeps them. A batch's rows
//! of each group are gathered in order, a stable sort by group, and fed to
//! its accumulator a vector at a time (`exec/batch.rs`, `fed_group`).

use crate::error::{Code, Result, SqlError};
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken, fed_group};
use crate::functions::{Accumulator, AggregateRef};
use crate::logical::AggCall;
use crate::storage::{Batch, Row};
use crate::value::{DataType, Value};

use super::super::Pending;
use super::{STATE_BYTES, Sent};

/// An accumulator a group, and the function and result type of each.
pub(in crate::exec) struct Boxed {
    accs: Vec<Box<dyn Accumulator>>,
    func: AggregateRef,
    result: DataType,
}

impl Boxed {
    /// The accumulators of `agg`, of no group yet.
    pub(super) fn of(agg: &AggCall) -> Boxed {
        Boxed { accs: Vec::new(), func: agg.func.clone(), result: agg.result.clone() }
    }

    /// The accumulators `accs` of the function and result of `agg`, of a
    /// column of states turned into accumulators.
    pub(super) fn made(accs: Vec<Box<dyn Accumulator>>, agg: &AggCall) -> Boxed {
        Boxed { accs, func: agg.func.clone(), result: agg.result.clone() }
    }

    /// A fresh accumulator.
    fn fresh(&self) -> Box<dyn Accumulator> {
        self.func.0.accumulator(self.result.clone())
    }

    /// Adds a group's, fed nothing.
    pub(super) fn grow(&mut self) {
        let fresh = self.fresh();
        self.accs.push(fresh);
    }

    /// Feeds each group's rows of `pending`, gathered in order, to its
    /// accumulator, a vector at a time, of `batch`, as `take` says, of
    /// `lane` if evaluated by vectors, an expression's row made in
    /// `scratch`, warnings heard by `runner`.
    pub(super) fn feed(
        &mut self,
        pending: &Pending,
        (batch, take, lane): (&Batch, &Taken, &Option<Fed<'_>>),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        let mut order: Vec<usize> = (0..pending.groups.len()).collect();
        // Stable: each group's rows keep their order.
        order.sort_by_key(|&at| pending.groups.get(at).copied().unwrap_or(u32::MAX));
        let (taken, lanes) = (std::slice::from_ref(take), std::slice::from_ref(lane));
        let (mut given, mut read) = (Vec::new(), Vec::new());
        for run in order.chunk_by(|&a, &b| pending.groups.get(a) == pending.groups.get(b)) {
            let Some(&g) = run.first().and_then(|&at| pending.groups.get(at)) else { continue };
            given.clear();
            given.extend(run.iter().filter_map(|&at| pending.given.get(at).copied()));
            read.clear();
            read.extend(run.iter().filter_map(|&at| pending.read.get(at).copied()));
            let Some(acc) = self.accs.get_mut(crate::storage::mask::index(g)) else { continue };
            fed_group(std::iter::once(acc), (taken, scratch, runner), (batch, lanes), (&given, &read))?;
        }
        Ok(())
    }

    /// Merges `state` into group `g`'s accumulator.
    pub(super) fn merge(&mut self, g: usize, state: &[u8]) -> Result<()> {
        match self.accs.get_mut(g) {
            Some(acc) => acc.merge(state),
            None => Ok(()),
        }
    }

    /// Group `g`'s state, written: whether it has one.
    pub(super) fn write(&self, g: usize, out: &mut Vec<u8>) -> bool {
        self.accs.get(g).is_some_and(|acc| acc.write_state(out))
    }

    /// Group `g`'s result.
    pub(super) fn finish(&self, g: usize) -> Result<Value> {
        self.accs.get(g).map_or(Ok(Value::Null), |acc| acc.finish())
    }

    /// Keeps the first `len` groups'.
    pub(super) fn truncate(&mut self, len: usize) {
        self.accs.truncate(len);
        self.accs.shrink_to_fit();
    }

    /// The bytes it holds, near enough.
    pub(super) fn bytes(&self) -> usize {
        self.accs.capacity().saturating_mul(size_of::<Box<dyn Accumulator>>().saturating_add(STATE_BYTES))
    }

    /// Each group's state, written one after another.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes none.
    pub(super) fn written(self) -> Result<Sent> {
        let (mut bytes, mut ends) = (Vec::new(), Vec::with_capacity(self.accs.len()));
        for acc in &self.accs {
            if !acc.write_state(&mut bytes) {
                return Err(SqlError::new(Code::Api, "[DESIGN] a group partly made of a set function of no state"));
            }
            ends.push(bytes.len());
        }
        Ok(Sent::Written { bytes, ends })
    }
}
