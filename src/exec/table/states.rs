//! \[DESIGN\] (RULES.md D356) The states of a table's set functions, a
//! column of each: COUNT, SUM, AVG, MAX and MIN of integers, and SUM and
//! AVG of approximate numbers, each a vector of a slot a group, fed a
//! batch's rows in one loop, `sums[g] += units[r]`, as Boncz, Zukowski and
//! Nes, "MonetDB/X100" (2005), update aggregates by primitive loops; every
//! other function, or one given values of a type its column does not hold,
//! an accumulator a group (`Boxed`), fed each group's rows of a batch a
//! vector at a time. Approximate numbers are added to each group in the
//! order its rows come, so a sum is the one an accumulator makes.
//!
//! A column of states finishes, writes and is turned into accumulators by
//! the accumulators' own code (`functions/columnar.rs`), so a group's
//! result, error and written state are an accumulator's. Merged with a
//! column of another table, two columns of one kind add their slots; any
//! other pair merges the written state, as a spill's pieces are merged.

mod boxed;
mod typed;

use crate::error::{Code, Result, fail};
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken};
use crate::functions::ColumnarKind;
use crate::logical::AggCall;
use crate::storage::{Batch, Row};
use crate::value::DataType;

use super::Pending;
pub(super) use boxed::Boxed;
pub(in crate::exec) use typed::Typed;

/// \[DESIGN\] The bytes an accumulator a group holds, near enough: as
/// `exec/group.rs` counts a set function's state.
const STATE_BYTES: usize = crate::exec::group::STATE_BYTES;

/// The states of one set function of a table's groups.
pub(in crate::exec) enum States {
    Typed(Typed),
    Boxed(Boxed),
}

/// The states of one set function of a table's groups, as they go to
/// another thread: in columns, or each written (`Accumulator::write_state`)
/// one after another, as an accumulator cannot go.
pub(in crate::exec) enum Sent {
    Typed(Typed),
    Written { bytes: Vec<u8>, ends: Vec<usize> },
}

impl Sent {
    /// The bytes it holds.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Sent::Typed(typed) => typed.bytes(),
            Sent::Written { bytes, ends } => bytes.capacity().saturating_add(ends.capacity().saturating_mul(size_of::<usize>())),
        }
    }

    /// \[DESIGN\] (RULES.md D1110) The states of the groups at `order`,
    /// in that order.
    pub(super) fn gathered(&self, order: &[u32]) -> Sent {
        if let Sent::Typed(typed) = self {
            return Sent::Typed(typed.gathered(order));
        }
        let (mut bytes, mut ends) = (Vec::new(), Vec::with_capacity(order.len()));
        for &i in order {
            self.write(crate::storage::mask::index(i), &mut bytes);
            ends.push(bytes.len());
        }
        Sent::Written { bytes, ends }
    }

    /// Group `i`'s state, written to `out`: whether it has one.
    fn write(&self, i: usize, out: &mut Vec<u8>) -> bool {
        match self {
            Sent::Typed(typed) => typed.write(i, out),
            Sent::Written { bytes, ends } => {
                let from = i.checked_sub(1).map_or(Some(0), |before| ends.get(before).copied());
                let state = from.zip(ends.get(i)).and_then(|(from, &to)| bytes.get(from..to));
                state.map(|s| out.extend_from_slice(s)).is_some()
            }
        }
    }
}

/// The error of a state a set function never writes, as
/// `Accumulator::merge` raises of one by default.
fn never_written<T>() -> Result<T> {
    fail(Code::Corrupt, "[DESIGN] a set function's state that it never writes")
}

impl States {
    /// The states of `agg`, of no group yet: in a column of its kind, if
    /// it is a built-in set function of a result one holds, else an
    /// accumulator a group.
    pub(super) fn of(agg: &AggCall) -> States {
        let kind = agg.func.0.columnar().map(|c| c.0);
        Typed::of(kind, &agg.result).map_or_else(|| States::Boxed(Boxed::of(agg)), States::Typed)
    }

    /// Adds a group's, fed nothing.
    pub(super) fn grow(&mut self) {
        match self {
            States::Typed(typed) => typed.grow(),
            States::Boxed(boxed) => boxed.grow(),
        }
    }

    /// Feeds each row of `pending` to its group's state, of `batch`, as
    /// `take` says, of `lane` if evaluated by vectors; a column that
    /// cannot take them turned into accumulators first. Warnings are heard
    /// by `runner`, an expression's row made in `scratch`; `agg` the call
    /// the states are of.
    pub(super) fn feed(
        &mut self,
        (pending, agg): (&Pending, &AggCall),
        (batch, take, lane): (&Batch, &Taken, &Option<Fed<'_>>),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        if let States::Typed(typed) = self {
            if typed.feed(pending, (batch, take, lane.as_ref()), runner)? {
                return Ok(());
            }
            self.boxed(agg);
        }
        match self {
            States::Boxed(boxed) => boxed.feed(pending, (batch, take, lane), (scratch, runner)),
            States::Typed(_) => Ok(()),
        }
    }

    /// Turns a column of states of `agg` into an accumulator a group, each
    /// as it was.
    fn boxed(&mut self, agg: &AggCall) {
        if let States::Typed(typed) = self {
            *self = States::Boxed(typed.to_boxed(agg));
        }
    }

    /// Merges group `i`'s of `other` into group `g`'s: added, if both are
    /// columns of one kind; else as written.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(super) fn merge(&mut self, (g, agg): (usize, &AggCall), (other, i): (&Sent, usize)) -> Result<()> {
        if let (States::Typed(typed), Sent::Typed(theirs)) = (&mut *self, other)
            && typed.merge_from(g, theirs, i)?
        {
            return Ok(());
        }
        let mut state = Vec::new();
        if !other.write(i, &mut state) {
            return never_written();
        }
        self.merge_written((g, agg), &state)
    }

    /// \[DESIGN\] (RULES.md D1110) Merges each group at `places` of
    /// `other` into the group of `groups` beside it: two columns of one
    /// kind in one loop, else group by group as `merge` does.
    ///
    /// # Errors
    ///
    /// As `merge`.
    pub(super) fn merge_all(&mut self, agg: &AggCall, groups: &[u32], (other, places): (&Sent, &[u32])) -> Result<()> {
        if let (States::Typed(typed), Sent::Typed(theirs)) = (&mut *self, other)
            && typed.merge_all(groups, theirs, places)?
        {
            return Ok(());
        }
        for (&g, &i) in groups.iter().zip(places) {
            self.merge((crate::storage::mask::index(g), agg), (other, crate::storage::mask::index(i)))?;
        }
        Ok(())
    }

    /// Adds `more` groups', fed nothing.
    pub(super) fn grow_by(&mut self, more: usize) {
        match self {
            States::Typed(typed) => typed.grow_by(more),
            States::Boxed(boxed) => (0..more).for_each(|_| boxed.grow()),
        }
    }

    /// Room for `more` groups more, of a column.
    pub(super) fn reserve(&mut self, more: usize) {
        if let States::Typed(typed) = self {
            typed.reserve(more);
        }
    }

    /// Merges `state`, as `Accumulator::write_state` writes it, into group
    /// `g`'s, of `agg`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(super) fn merge_written(&mut self, (g, agg): (usize, &AggCall), state: &[u8]) -> Result<()> {
        if let States::Typed(typed) = self {
            if typed.merge_written(g, state)? {
                return Ok(());
            }
            self.boxed(agg);
        }
        match self {
            States::Boxed(boxed) => boxed.merge(g, state),
            States::Typed(_) => Ok(()),
        }
    }

    /// Group `g`'s state, written to `out` as its accumulator writes it:
    /// whether it has one.
    pub(super) fn write(&self, g: usize, out: &mut Vec<u8>) -> bool {
        match self {
            States::Typed(typed) => typed.write(g, out),
            States::Boxed(boxed) => boxed.write(g, out),
        }
    }

    /// Group `g`'s result.
    ///
    /// # Errors
    ///
    /// As `Accumulator::finish`.
    pub(super) fn finish(&self, g: usize) -> Result<crate::value::Value> {
        match self {
            States::Typed(typed) => typed.finish(g),
            States::Boxed(boxed) => boxed.finish(g),
        }
    }

    /// Keeps the first `len` groups'.
    pub(super) fn truncate(&mut self, len: usize) {
        match self {
            States::Typed(typed) => typed.truncate(len),
            States::Boxed(boxed) => boxed.truncate(len),
        }
    }

    /// The bytes it holds.
    pub(super) fn bytes(&self) -> usize {
        match self {
            States::Typed(typed) => typed.bytes(),
            States::Boxed(boxed) => boxed.bytes(),
        }
    }

    /// The states of `agg` of `len` groups as `sent` holds them: a column
    /// as it is, each accumulator's merged from its state written.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(super) fn thawed(sent: Sent, agg: &AggCall, len: usize) -> Result<States> {
        if let Sent::Typed(typed) = sent {
            return Ok(States::Typed(typed));
        }
        let (mut states, mut state) = (States::Boxed(Boxed::of(agg)), Vec::new());
        for g in 0..len {
            states.grow();
            state.clear();
            if !sent.write(g, &mut state) {
                return never_written();
            }
            states.merge_written((g, agg), &state)?;
        }
        Ok(states)
    }

    /// As it goes to another thread: an accumulator's state written.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state, as a grouping run in
    /// parallel has none such.
    pub(super) fn sent(self) -> Result<Sent> {
        match self {
            States::Typed(typed) => Ok(Sent::Typed(typed)),
            States::Boxed(boxed) => boxed.written(),
        }
    }
}

/// Whether `kind` of result `result` keeps an exact column: of a sum, a
/// mean or an extreme whose result is exact; and an approximate one, of a
/// sum or a mean whose result is approximate.
fn columns_of(kind: ColumnarKind, result: &DataType) -> (bool, bool) {
    let (exact, approx) = (matches!(result, DataType::Exact { .. }), matches!(result, DataType::Approximate { .. }));
    match kind {
        ColumnarKind::Count => (false, false),
        ColumnarKind::Sum | ColumnarKind::Avg => (exact, approx),
        ColumnarKind::Min | ColumnarKind::Max => (exact, false),
    }
}

// Unit tests: each column of states is its accumulator (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
