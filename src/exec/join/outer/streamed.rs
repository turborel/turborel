//! \[DESIGN\] An outer join on keys whose right operand outgrows its work
//! area, of which no order is asked (RULES.md D38): partitioned as
//! `spilled.rs` partitions it, each pair of partitions joined as
//! `chunked.rs` joins it, a part of the right at a time; but its rows given
//! as the left rows probe each part, not written to runs and merged back
//! into the join's order, so a join of streams reads each of them once
//! (`exec/batch/joined/outer.rs`). A part's right rows that matched none
//! come once the left partition has probed it; the left rows of a null key
//! or barred by their own condition, and the right rows of a null key,
//! last. The pairs come in the order of their partitions.

use std::collections::VecDeque;
use std::iter::Peekable;

use super::super::super::partition::unnumbered;
use super::super::super::work::Work;
use super::super::super::{RowIter, failed};
use super::Spec;
use super::chunked::Part;
use super::spilled::{Parts, alone_rights, lost};
use crate::error::Result;
use crate::spill::{Records, SpillRun};
use crate::storage::Row;

/// A left row's flag, in a run: it matched a right row of a part before
/// (`chunked.rs`).
const MATCHED: u8 = 1;
/// A left row's flag, in a run: it matched none yet.
const UNMATCHED: u8 = 0;

/// The outer join `spec` says of `left` with a right operand of which
/// `held` came first, then `rest`, in `work`, its rows in no set order.
pub(super) fn joined(left: RowIter, (held, rest): (Vec<Row>, RowIter), spec: Spec, mut work: Work) -> RowIter {
    match prepared(left, (held, rest), &spec, &mut work) {
        Ok((pending, last)) => {
            let judged = spec.own.1.is_some() && last.1;
            Box::new(Streamed { spec, work, judged, pending, current: None, last: Some(last.0), ready: VecDeque::new(), done: false })
        }
        Err(e) => failed(e),
    }
}

/// A pair of partitions to join: the right's, its bytes, and the left's,
/// and how many times it was partitioned again.
type Pending = (Box<dyn SpillRun>, usize, Box<dyn SpillRun>, usize);

/// The rows given last, and whether the left had a row.
type Last = (Vec<Records>, bool);

/// Both operands partitioned: each pair to join; and the rows given last,
/// the left's alone with nulls and the right's of a null key, after nulls
/// if kept, and whether the left had a row.
fn prepared(left: RowIter, (held, rest): (Vec<Row>, RowIter), spec: &Spec, work: &mut Work) -> Result<(Vec<Pending>, Last)> {
    let parts = Parts::new(spec, work)?;
    let (rights, lonely_right) = parts.rights(held.into_iter().map(Ok).chain(rest), spec, work)?;
    let (lefts, lonely_left, any_left) = parts.lefts(left, spec, work)?;
    let judged = spec.own.1.as_ref().filter(|_| any_left);
    let lonely_right = alone_rights(lonely_right.into_records()?, (spec, judged), work)?;
    let pending = rights.into_iter().zip(lefts).map(|((right, bytes), probes)| (right, bytes, probes, 0)).collect();
    Ok((pending, (vec![lonely_left.into_records()?, lonely_right], any_left)))
}

/// A pair being joined: what is left of its right partition, its left
/// partition, the part held, the left rows of this part to read, of each
/// whether it matched a part before, and the flags written for the next.
struct Pair {
    right: Peekable<Records>,
    probes: Box<dyn SpillRun>,
    part: Part,
    left: Records,
    before: Option<Records>,
    flags: Option<Box<dyn SpillRun>>,
}

/// The join's rows, as they are pulled.
struct Streamed {
    spec: Spec,
    work: Work,
    /// Whether the right's own condition is judged of each of its rows.
    judged: bool,
    pending: Vec<Pending>,
    current: Option<Pair>,
    /// The rows given last, once every pair is joined.
    last: Option<Vec<Records>>,
    ready: VecDeque<Row>,
    done: bool,
}

impl Streamed {
    /// Takes up the next pair, partitioning it again if its right outgrows
    /// the work area and the depth allows: whether one is left.
    fn take_up(&mut self) -> Result<bool> {
        let Some((right, bytes, probes, depth)) = self.pending.pop() else { return Ok(false) };
        if bytes > self.work.limit() && depth < self.work.depth() {
            let parts = Parts::new(&self.spec, &self.work)?;
            let rights = parts.split(right.into_records()?, (&self.spec, true), &mut self.work)?;
            // A partition that did not shrink is joined in parts, not partitioned on.
            let next = if rights.iter().all(|(_, b)| *b < bytes) { depth.saturating_add(1) } else { usize::MAX };
            let lefts = parts.split(probes.into_records()?, (&self.spec, false), &mut self.work)?;
            for ((sub, sub_bytes), (sub_probes, _)) in rights.into_iter().zip(lefts) {
                self.pending.push((sub, sub_bytes, sub_probes, next));
            }
            return Ok(true);
        }
        self.current = Some(self.part_of(right.into_records()?.peekable(), probes, None)?);
        Ok(true)
    }

    /// The pair of `right`, what is left of a right partition, and its left
    /// partition `probes`, its next part held, `before` the flags of the
    /// parts before.
    fn part_of(&mut self, mut right: Peekable<Records>, mut probes: Box<dyn SpillRun>, before: Option<Records>) -> Result<Pair> {
        let part = Part::of(&mut right, (&self.spec, self.judged), &self.work)?;
        let flags = if right.peek().is_none() { None } else { Some(self.work.run()?) };
        let left = probes.records()?;
        Ok(Pair { right, probes, part, left, before, flags })
    }

    /// The next step of the pair being joined: a left row probing its part,
    /// or, the left read, the part's unmatched right rows readied and the
    /// next part taken up, or the pair done.
    fn step(&mut self) -> Result<()> {
        let Some(pair) = &mut self.current else { return Ok(()) };
        if let Some(record) = pair.left.next() {
            let (_, l) = unnumbered(&record?, self.work.types())?;
            let was = match pair.before.as_mut().and_then(Iterator::next).transpose()? {
                Some(flag) => flag.first() == Some(&MATCHED),
                None => false,
            };
            let ready = &mut self.ready;
            let found = pair.part.probed(&l, &self.spec, &mut |joined| {
                ready.push_back(joined);
                Ok(())
            })? || was;
            match &mut pair.flags {
                Some(flags) => flags.push(&[if found { MATCHED } else { UNMATCHED }])?,
                None => {
                    if !found && self.spec.kind.keeps_left() {
                        self.ready.push_back(self.spec.left_alone(&l));
                    }
                }
            }
            return Ok(());
        }
        if self.spec.kind.keeps_right() {
            self.ready.extend(pair.part.take_unmatched(&self.spec));
        }
        let Some(done) = self.current.take() else { return Err(lost()) };
        if let Some(flags) = done.flags {
            self.current = Some(self.part_of(done.right, done.probes, Some(flags.into_records()?))?);
        }
        Ok(())
    }

    /// The rows given last, readied.
    fn last(&mut self) -> Result<()> {
        for records in self.last.take().unwrap_or_default() {
            for record in records {
                self.ready.push_back(unnumbered(&record?, self.work.types())?.1);
            }
        }
        Ok(())
    }

    fn advance(&mut self) -> Result<()> {
        if self.current.is_some() {
            return self.step();
        }
        if !self.take_up()? {
            self.last()?;
            self.done = true;
        }
        Ok(())
    }
}

impl Iterator for Streamed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(row) = self.ready.pop_front() {
                return Some(Ok(row));
            }
            if self.done {
                return None;
            }
            if let Err(e) = self.advance() {
                self.done = true;
                return Some(Err(e));
            }
        }
    }
}
