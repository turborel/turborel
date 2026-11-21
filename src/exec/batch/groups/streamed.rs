//! \[DESIGN\] (RULES.md D551) Groups made by batch of rows that come in the
//! order of their grouping columns: one group at a time, its key in a
//! table's lanes of one group (`exec/table/keys.rs`), so a row is of it
//! exactly when a hashed grouping would find it there. Each batch is cut
//! into runs of rows of one key, each run fed to the group's set functions
//! a vector at a time (`fed_group`), and a group is finished as the first
//! row of the next comes [X3.135-1986 §5.22 GR2]. No key is hashed and
//! nothing is held but the group being made, so a grouping of more groups
//! than the work area holds never spills, and its groups are given as its
//! input is read, a FETCH FIRST over it stopping it.
//!
//! A set function's state may not go to another thread, as rows may, so
//! none is kept between two asks for rows: each ask makes the groups of a
//! batch whole, and of the next batches the rows of its last group, and
//! keeps the batch where the next group begins, read from there at the
//! next ask. A batch is so read at most twice, its first run alone the
//! first time.

use std::collections::VecDeque;

use crate::error::Result;
use crate::exec::batch::stream::Kept;
use crate::exec::batch::{Batches, Fed, Read, fed_group, lanes};
use crate::exec::table::{Key, Keys};
use crate::exec::{Db, RowIter, Runner};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::storage::sel::nth;
use crate::storage::{Batch, Row, Sel};
use crate::txn::TxId;

/// The groups of the rows of `batches`, in the order of the grouping
/// columns `read` reads, each its grouping values and the finished set
/// functions `aggs`, given as they are made, for the statement of `tx` in
/// `db`.
pub(in crate::exec) fn streamed(read: Read, aggs: Vec<AggCall>, (db, tx): (&Db, TxId), batches: Batches) -> RowIter {
    let how = How { keys: Keys::new(read.groups.len()), read, aggs, runner: Runner::new(db, tx) };
    let made = Made { out: VecDeque::new(), bytes: Vec::new(), scratch: Vec::new(), given: Vec::new() };
    Box::new(Streamed { how, made, batches: Some(batches), pending: None })
}

/// What a streamed grouping reads and computes, and the key of the group
/// being made.
struct How {
    read: Read,
    aggs: Vec<AggCall>,
    runner: Runner,
    keys: Keys,
}

/// The groups a streamed grouping finished, not yet given, and its
/// scratch space.
struct Made {
    out: VecDeque<Row>,
    /// The bytes a lane compares a key by where its values cannot tell.
    bytes: Vec<u8>,
    /// The row an expression a set function takes is evaluated of.
    scratch: Row,
    /// The places among the rows of a batch given, each its own.
    given: Vec<u32>,
}

/// The set functions of the group being made, if one is.
type States = Option<Vec<Box<dyn Accumulator>>>;

/// A streamed grouping, its batches read as its groups are asked for:
/// the batch the next group begins in and where, if one was left there.
struct Streamed {
    how: How,
    made: Made,
    batches: Option<Batches>,
    pending: Option<(Kept, usize)>,
}

impl Streamed {
    /// The next group, the groups of a batch made if none is held.
    fn step(&mut self) -> Option<Result<Row>> {
        if self.made.out.is_empty()
            && let Err(e) = self.fill()
        {
            self.batches = None;
            self.pending = None;
            return Some(Err(e));
        }
        self.made.out.pop_front().map(Ok)
    }

    /// The next batch to begin at, and where: the one left, or the next
    /// batch read, from its first row; none once they end.
    fn begun(&mut self) -> Result<Option<(Kept, usize)>> {
        if let Some(pending) = self.pending.take() {
            return Ok(Some(pending));
        }
        self.batches.as_mut().and_then(Iterator::next).transpose().map(|kept| kept.map(|k| (k, 0)))
    }

    /// Makes the groups of the batch begun at, whole, and of the next ones
    /// the rows of the last of them, leaving the batch the next group
    /// begins in; the last group made once the batches end.
    fn fill(&mut self) -> Result<()> {
        let Some((mut kept, mut start)) = self.begun()? else { return Ok(()) };
        let mut states: States = None;
        let mut first = true;
        loop {
            let (batch, rows) = (&kept.0, kept.1.as_ref().map(Sel::as_slice));
            let stop = !first && !self.made.out.is_empty();
            if let Some(at) = self.made.batch(&mut self.how, (batch, rows), (start, stop), &mut states)? {
                self.pending = Some((kept, at));
                return Ok(());
            }
            first = false;
            let Some(next) = self.batches.as_mut().and_then(Iterator::next) else {
                self.batches = None;
                if let Some(states) = states.take() {
                    self.made.out.push_back(finished(&self.how.keys, states)?);
                }
                return Ok(());
            };
            (kept, start) = (next?, 0);
        }
    }
}

impl Iterator for Streamed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step()
    }
}

impl Made {
    /// Groups the rows of `batch` given, `rows` or all, from `start`: each
    /// run of one key fed to its group, the group before finished where a
    /// new key begins. If `stop`, it stops where the first new key begins,
    /// once the group before is finished: there, which it gives.
    fn batch(
        &mut self,
        how: &mut How,
        (batch, rows): (&Batch, Option<&[u32]>),
        (start, stop): (usize, bool),
        states: &mut States,
    ) -> Result<Option<usize>> {
        let n = rows.map_or(batch.rows, <[u32]>::len);
        self.given.clear();
        self.given.extend((0..n).map(|i| u32::try_from(i).unwrap_or(u32::MAX)));
        let lanes = lanes(&how.read.taken, batch, rows);
        let mut from = start;
        for i in start..n {
            let key = Key::Row(batch, &how.read.groups, nth(rows, i));
            if states.is_some() && how.keys.equal(0, key, &mut self.bytes) {
                continue;
            }
            if let Some(mut made) = states.take() {
                self.fed(how, (batch, &lanes, rows), (from, i), &mut made)?;
                self.out.push_back(finished(&how.keys, made)?);
                if stop {
                    return Ok(Some(i));
                }
            }
            how.keys.truncate(0);
            how.keys.push(key);
            *states = Some(how.aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect());
            from = i;
        }
        if let Some(made) = states.as_mut() {
            self.fed(how, (batch, &lanes, rows), (from, n), made)?;
        }
        Ok(None)
    }

    /// Feeds `states` the rows given from `from` up to `to` of `batch`, of
    /// the rows `rows` or all.
    fn fed(
        &mut self,
        how: &How,
        (batch, lanes, rows): (&Batch, &[Option<Fed<'_>>], Option<&[u32]>),
        (from, to): (usize, usize),
        states: &mut [Box<dyn Accumulator>],
    ) -> Result<()> {
        let Some(given) = self.given.get(from..to).filter(|g| !g.is_empty()) else { return Ok(()) };
        let read_rows = rows.and_then(|r| r.get(from..to)).unwrap_or(given);
        fed_group(states.iter_mut(), (&how.read.taken, &mut self.scratch, &how.runner), (batch, lanes), (given, read_rows))
    }
}

/// The row of the finished group whose key `keys` holds: its grouping
/// values, then each of `states`' results.
fn finished(keys: &Keys, states: Vec<Box<dyn Accumulator>>) -> Result<Row> {
    let mut row = keys.values(0);
    for state in states {
        row.push(state.finish()?);
    }
    Ok(row)
}

// Unit tests: groups cut at batches' bounds and within them, of selected
// rows, and where an ask stops (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::batch::Taken;
    use crate::exec::tests::state;
    use crate::txn::NO_TRANSACTION;
    use crate::value::{Decimal, Value};

    /// An integer value.
    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// The groups, as text, of batches of the rows of keys `keys`, each of
    /// value 1, keyed by column 0, counted and summed: each batch given as
    /// the rows its selection keeps, if `selected` has one.
    fn grouped(keys: &[&[Option<i128>]], selected: &[Option<Vec<u32>>]) -> Vec<String> {
        let db = Db::from(std::sync::Arc::new(state()));
        let aggs: Vec<AggCall> = ["COUNT", "SUM"]
            .into_iter()
            .map(|name| {
                let func = db.functions.aggregate(name).unwrap();
                let arg = (name == "SUM").then_some(crate::logical::Expr::Column(1));
                let result = func
                    .0
                    .result_type(arg.as_ref().map(|_| crate::value::DataType::Exact {
                        precision: crate::value::INTEGER_PRECISION,
                        scale: 0,
                        spelled: crate::value::Spelled::Integer,
                    }))
                    .unwrap();
                AggCall { func, arg, distinct: false, result, filter: None, distribution: None, label: name.into() }
            })
            .collect();
        let read = Read { taken: vec![Taken::Count, Taken::Column(1)], groups: vec![0] };
        let made: Vec<Result<Kept>> = keys
            .iter()
            .zip(selected)
            .map(|(rows, sel)| {
                let mut batch = Batch::empty(2);
                for key in *rows {
                    batch.push(&vec![key.map_or(Value::Null, int), int(1)], &[0, 1]);
                }
                Ok((batch, sel.clone().map(Sel::of)))
            })
            .collect();
        let rows = streamed(read, aggs, (&db, NO_TRANSACTION), Box::new(made.into_iter()));
        rows.map(|r| r.unwrap().iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect()
    }

    /// Verifies: [X3.135-1986 §5.22 GR2, §5.11 GR7] [DESIGN] RULES.md D551:
    /// rows of one key in a run are one group, within a batch and across
    /// batches' bounds, nulls of one group; a group spanning three batches
    /// is one; only the rows a batch's selection keeps are grouped.
    ///
    /// Decision `states.is_some() && how.keys.equal(..)`: the first row
    /// (no group yet) against a row of the group's key (fed) and one of
    /// another (a new group). Decision `!first && !out.is_empty()` in
    /// `fill`: a batch after the first, of a group spanning it before any
    /// is finished (read whole), against one after a group was finished
    /// (it stops at the first new key, the rest read at the next ask).
    #[test]
    fn runs_of_one_key_are_one_group() {
        let none = vec![None; 4];
        let found = grouped(&[&[Some(1), Some(1), Some(2)], &[Some(2), Some(2), Some(3)], &[Some(3), None, None]], &none);
        assert_eq!(found, ["1,2,2", "2,3,3", "3,2,2", "NULL,2,2"]);
        let spanning = grouped(&[&[Some(5)], &[Some(5), Some(5)], &[Some(5), Some(6)], &[Some(6), Some(7)]], &none);
        assert_eq!(spanning, ["5,4,4", "6,2,2", "7,1,1"]);
        let selected = grouped(&[&[Some(1), Some(2), Some(2), Some(3)], &[Some(3), Some(4)]], &[Some(vec![0, 2]), Some(vec![1])]);
        assert_eq!(selected, ["1,1,1", "2,1,1", "4,1,1"]);
        assert_eq!(grouped(&[&[]], &none), Vec::<String>::new());
    }
}
