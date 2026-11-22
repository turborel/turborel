//! \[DESIGN\] (RULES.md D551) Grouping and DISTINCT of rows that come in
//! the order of their grouping columns, a row at a time: the rows of a group
//! come together, so one group is made at a time, finished and given as
//! the first row of the next comes, and none is held, hashed or spilled
//! [X3.135-1986 §5.22 GR2, §5.25 GR2]. A row is of the group before if its
//! key is that group's, the bytes `exec/group.rs` keys a group by, so two
//! values are of one group here exactly when they are hashed together
//! there. Groups come in the order of their first rows, as hashed ones
//! do, and that is their keys' order. A grouping that makes groups before
//! any row, one by a collation, and one of set functions DISTINCT or of an
//! inverse distribution, is made hashed (`grouping`): the planner never
//! streams them. (RULES.md D785) A grouping by no column, of set functions
//! neither DISTINCT nor of an inverse distribution, is its one group
//! [X3.135-1986 §5.22 GR1]: fed every row as it comes and finished at the
//! end, even of no rows [X3.135-1986 §5.23 GR1], with no key, hash or work
//! area, whichever way the planner asked it made (`whole`).

use super::{Argument, Spec, argument, fresh, grouping};
use crate::error::Result;
use crate::exec::{Db, RowIter, Runner, column};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::spill::put_norm;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The groups of `input` under `spec`, its rows in the order of the
/// grouping columns, each given as soon as its last row has come; or, of
/// a grouping that cannot be so made, hashed (`grouping`).
#[inline(never)]
pub(in crate::exec) fn streamed_grouping(spec: Spec, input: RowIter, db: &Db, tx: TxId) -> RowIter {
    let alone = spec.aggs.iter().any(|a| a.distinct || a.distribution.is_some());
    let collated = spec.collations.iter().any(Option::is_some);
    if spec.groups.is_empty() || !spec.seeds.is_empty() || alone || collated {
        return grouping(spec, input, db, tx);
    }
    Box::new(Streamed { spec, input, runner: Runner::new(db, tx), pending: None, key: Vec::new(), next: Vec::new() })
}

/// The one row of `spec`'s grouping of `input`, if it is a grouping by
/// no column, made before no row, of set functions none DISTINCT or of an
/// inverse distribution (RULES.md D785): each set function fed each row
/// [X3.135-1986 §5.8 GR1], made and finished within the one ask for its
/// row, as a streamed group is (RULES.md D551); else `spec` and `input`
/// given back, to be grouped by key.
pub(in crate::exec) fn whole(spec: Spec, input: RowIter, db: &Db, tx: TxId) -> Whole {
    let alone = spec.aggs.iter().any(|a| a.distinct || a.distribution.is_some());
    if !spec.groups.is_empty() || !spec.seeds.is_empty() || alone {
        return Whole::Keyed(spec, input);
    }
    let runner = Runner::new(db, tx);
    Whole::Made(Box::new(std::iter::once_with(move || {
        let mut states = fresh(&spec);
        for read in input {
            fed(&spec.aggs, &mut states, &read?, &runner)?;
        }
        states.into_iter().map(|state| state.finish()).collect()
    })))
}

/// A grouping's rows made whole (`whole`), or what it groups, to be
/// grouped by key.
pub(in crate::exec) enum Whole {
    Made(RowIter),
    Keyed(Spec, RowIter),
}

/// The rows of `input`, in the order of every column, DISTINCT: each the
/// first of a run of rows alike, compared with the row before alone.
#[inline(never)]
pub(in crate::exec) fn streamed_distinct(input: RowIter) -> RowIter {
    let (mut before, mut key): (Option<Vec<u8>>, Vec<u8>) = (None, Vec::new());
    Box::new(input.filter(move |r| {
        let Ok(row) = r else { return true };
        key.clear();
        for v in row {
            put_norm(v, &mut key);
        }
        let fresh = before.as_ref() != Some(&key);
        if fresh {
            before = Some(key.clone());
        }
        fresh
    }))
}

/// A grouping made as its rows are pulled: the row the next group begins
/// with, once read, and the keys of the group being made and of the row
/// read, as `group_key` makes them of a grouping by no collation. A set
/// function's state may not go to another thread, as rows may, so each
/// group's are made and finished within one ask for a row.
struct Streamed {
    spec: Spec,
    input: RowIter,
    runner: Runner,
    pending: Option<Row>,
    key: Vec<u8>,
    next: Vec<u8>,
}

impl Streamed {
    /// The next group, finished once a row of another or the end comes.
    /// Never inlined into the iterator, as a row is pulled through every
    /// level of a plan on the stack (RULES.md R-CF-6).
    #[inline(never)]
    fn step(&mut self) -> Option<Result<Row>> {
        let first = match self.pending.take() {
            Some(row) => row,
            None => match self.input.next()? {
                Ok(row) => row,
                Err(e) => return Some(Err(e)),
            },
        };
        Some(self.group(&first))
    }

    /// The group whose first row is `first`, fed each row after it of its
    /// key, the first of another kept for the next.
    fn group(&mut self, first: &Row) -> Result<Row> {
        let mut values: Row = self.spec.groups.iter().map(|&c| column(first, c).cloned()).collect::<Result<_>>()?;
        key_of(first, &self.spec.groups, &mut self.key)?;
        let mut states = fresh(&self.spec);
        fed(&self.spec.aggs, &mut states, first, &self.runner)?;
        for read in self.input.by_ref() {
            let row = read?;
            key_of(&row, &self.spec.groups, &mut self.next)?;
            if self.next != self.key {
                self.pending = Some(row);
                break;
            }
            fed(&self.spec.aggs, &mut states, &row, &self.runner)?;
        }
        for state in states {
            values.push(state.finish()?);
        }
        Ok(values)
    }
}

/// Writes over `out` the key of `row` grouped by `groups`, of no
/// collation: the bytes `group_key` makes of its grouping values.
fn key_of(row: &Row, groups: &[usize], out: &mut Vec<u8>) -> Result<()> {
    out.clear();
    for &c in groups {
        put_norm(column(row, c)?, out);
    }
    Ok(())
}

impl Iterator for Streamed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step()
    }
}

/// Feeds each of `states` its set function's argument of `row`
/// [X3.135-1986 §5.8 GR1]: a null joins none.
fn fed(aggs: &[AggCall], states: &mut [Box<dyn Accumulator>], row: &Row, runner: &Runner) -> Result<()> {
    for (a, acc) in aggs.iter().zip(states.iter_mut()) {
        match argument(a, row, runner)? {
            Argument::Passed => {}
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Argument::Counted => acc.update(&Value::Null)?,
            Argument::Value(v) => {
                if !v.is_null() {
                    acc.update(&v)?;
                }
            }
        }
    }
    Ok(())
}
