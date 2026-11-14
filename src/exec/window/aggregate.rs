//! [ISO/IEC 9075-2:2023 §7.15 GR5, §10.9 GR] A set function over each
//! row's window frame in a partition, computed the cheapest of three ways.
//! A frame that only grows, from the partition's first row with nothing
//! left out, feeds one accumulator the rows its end passes, finished for
//! each row; that is the frame of no ORDER BY, or of one with no framing
//! clause, and the only one a DISTINCT set function has [§9.23 SR8d], its
//! values taken kept in a bag that spills (`exec/work.rs`). Otherwise a
//! set function whose state can be written (`Accumulator::write_state`)
//! is given a tree of states, each the merge of its two children's, built
//! only while it fits the bytes it may hold, so each frame is the merge of
//! at most a few states for each of its ranges, merged first to last. Of
//! any other, each frame's rows are fed to a new accumulator. A frame
//! alike to the row before it gives the value it gave.

use std::slice;

use super::frame::{Bounds, EMPTY, Partition, Ranges};
use super::reader::Reader;
use crate::error::Result;
use crate::exec::broken;
use crate::exec::work::{Bag, Work};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::spill::norm_row;
use crate::value::Value;

/// The bytes a node of a tree of states takes in memory beyond its state,
/// near enough.
const NODE_BYTES: usize = size_of::<Option<Node>>();

/// The most levels a tree of states over a partition has: one more than
/// the bits of a place.
const MOST_LEVELS: usize = 65;

/// A node of a tree of states: the state of the rows below it, and
/// whether any of their arguments was null, left out.
#[derive(Clone)]
struct Node {
    state: Vec<u8>,
    nulls: bool,
}

/// A set function over each row's frame in a partition.
pub(in crate::exec) struct Framed<'a> {
    call: &'a AggCall,
    /// Where its argument is among a row's values.
    slot: usize,
    way: Way,
    /// The last frame and the value and nulls of it.
    last: Option<(Ranges, Value, bool)>,
}

/// How the frames are computed.
enum Way {
    /// One accumulator fed the rows up to the frame's end, and how many it
    /// was fed, the nulls it left out, and the values it took if DISTINCT.
    Growing { acc: Box<dyn Accumulator>, fed: usize, nulls: bool, seen: Option<Bag>, reader: Reader },
    /// A tree of states, its leaves from `size` on.
    Tree { nodes: Vec<Option<Node>>, size: usize },
    /// Each frame fed to a new accumulator.
    Each(Reader),
}

impl<'a> Framed<'a> {
    /// `call` over the frames `bounds` finds of `part`, its argument at
    /// `slot`, a tree of states built if it fits in `bytes`, a DISTINCT
    /// one's values kept in `work`.
    ///
    /// # Errors
    ///
    /// The spill area's, reading `part`; the set function's.
    pub(in crate::exec) fn new(
        call: &'a AggCall,
        (bounds, slot): (&Bounds, usize),
        part: &mut Partition,
        bytes: usize,
        work: &Work,
    ) -> Result<Framed<'a>> {
        let way = if bounds.grows() {
            let seen = call.distinct.then(|| Bag::new(work.another()));
            Way::Growing { acc: fresh(call), fed: 0, nulls: false, seen, reader: Reader::default() }
        } else if call.distinct {
            return Err(broken("a DISTINCT set function over a frame that moves"));
        } else {
            match tree(call, slot, part, bytes)? {
                Some((nodes, size)) => Way::Tree { nodes, size },
                None => Way::Each(Reader::default()),
            }
        };
        Ok(Framed { call, slot, way, last: None })
    }

    /// Its value over `frame`, the frame of `current`, and whether it left
    /// out a null: of the frame of the row before if alike.
    ///
    /// # Errors
    ///
    /// The spill area's, reading `part`; the set function's, taking a value
    /// or finishing.
    pub(in crate::exec) fn value(&mut self, frame: Ranges, part: &mut Partition) -> Result<(Value, bool)> {
        if let Some((last, value, nulls)) = &self.last
            && *last == frame
        {
            return Ok((value.clone(), *nulls));
        }
        let (call, slot) = (self.call, self.slot);
        let (value, nulls) = match &mut self.way {
            Way::Growing { acc, fed, nulls, seen, reader } => {
                let to = frame.first().map_or(0, |r| r.1);
                while *fed < to {
                    let row = reader.get(&mut part.keys, *fed)?;
                    take(acc.as_mut(), argument(call, slot, &row.values)?, (nulls, seen.as_mut()))?;
                    *fed = fed.saturating_add(1);
                }
                (acc.finish()?, *nulls)
            }
            Way::Tree { nodes, size } => merged(call, (nodes, *size), &frame)?,
            Way::Each(reader) => each(call, slot, (reader, part), &frame)?,
        };
        self.last = Some((frame, value.clone(), nulls));
        Ok((value, nulls))
    }
}

/// A new accumulator of `call`, fed nothing.
pub(super) fn fresh(call: &AggCall) -> Box<dyn Accumulator> {
    call.func.0.accumulator(call.result.clone())
}

/// The argument of `call` among a row's `values`, at `slot`: none for
/// COUNT(*).
fn argument<'v>(call: &AggCall, slot: usize, values: &'v [Value]) -> Result<Option<&'v Value>> {
    if call.arg.is_none() {
        return Ok(None);
    }
    values.get(slot).map(Some).ok_or_else(|| broken("a window's row has no argument of its set function"))
}

/// [ISO/IEC 9075:1992 §6.5 GR1b, GR4b] Feeds `acc` the argument `arg`: a
/// row of COUNT(*) if none; nothing if null, `nulls` then set; if
/// DISTINCT, only once of each value, those taken kept in `seen`.
pub(super) fn take(acc: &mut dyn Accumulator, arg: Option<&Value>, (nulls, seen): (&mut bool, Option<&mut Bag>)) -> Result<()> {
    let Some(v) = arg else { return acc.update(&Value::Null) };
    if v.is_null() {
        *nulls = true;
        return Ok(());
    }
    if let Some(seen) = seen
        && !seen.add_new(&norm_row(slice::from_ref(v)))?
    {
        return Ok(());
    }
    acc.update(v)
}

/// The value of `call` over the ranges of `frame`, each of its rows read
/// by `reader` of `part` and fed to a new accumulator; and whether it left
/// out a null.
fn each(call: &AggCall, slot: usize, (reader, part): (&mut Reader, &mut Partition), frame: &Ranges) -> Result<(Value, bool)> {
    let (mut acc, mut nulls) = (fresh(call), false);
    for &(from, to) in frame {
        for place in from..to {
            let row = reader.get(&mut part.keys, place)?;
            take(acc.as_mut(), argument(call, slot, &row.values)?, (&mut nulls, None))?;
        }
    }
    Ok((acc.finish()?, nulls))
}

/// A tree of the states of `call` over `part`, its argument at `slot`:
/// leaves from the returned size on, a power of two, each of one row, and
/// each node before them the merge of its two children, none of no rows.
/// None if its state cannot be written, or the tree passes `bytes`.
fn tree(call: &AggCall, slot: usize, part: &mut Partition, bytes: usize) -> Result<Option<(Vec<Option<Node>>, usize)>> {
    if !fresh(call).write_state(&mut Vec::new()) {
        return Ok(None);
    }
    let size = part.len.checked_next_power_of_two().ok_or_else(|| broken("a partition too large for a tree of states"))?;
    let Some(mut held) = NODE_BYTES.checked_mul(size.saturating_mul(2)).filter(|&b| b <= bytes) else { return Ok(None) };
    let mut nodes: Vec<Option<Node>> = vec![None; size.saturating_mul(2)];
    let mut reader = Reader::default();
    for place in 0..part.len {
        let (mut acc, mut nulls) = (fresh(call), false);
        let row = reader.get(&mut part.keys, place)?;
        take(acc.as_mut(), argument(call, slot, &row.values)?, (&mut nulls, None))?;
        let node = written(acc.as_ref(), nulls)?;
        held = held.saturating_add(node.state.len());
        if held > bytes {
            return Ok(None);
        }
        let leaf = nodes.get_mut(size.saturating_add(place)).ok_or_else(|| broken("a tree of states has no leaf of a row"))?;
        *leaf = Some(node);
    }
    for at in (1..size).rev() {
        let children =
            (nodes.get(at.saturating_mul(2)).cloned().flatten(), nodes.get(at.saturating_mul(2).saturating_add(1)).cloned().flatten());
        let node = joined(call, children)?;
        held = held.saturating_add(node.as_ref().map_or(0, |n| n.state.len()));
        if held > bytes {
            return Ok(None);
        }
        let parent = nodes.get_mut(at).ok_or_else(|| broken("a tree of states has no node of its place"))?;
        *parent = node;
    }
    Ok(Some((nodes, size)))
}

/// The state of `acc`, which left out a null if `nulls`.
fn written(acc: &dyn Accumulator, nulls: bool) -> Result<Node> {
    let mut state = Vec::new();
    if !acc.write_state(&mut state) {
        return Err(broken("a set function's state was written once and not again"));
    }
    Ok(Node { state, nulls })
}

/// The node of two children, first then second, of `call`: the merge of
/// their states.
fn joined(call: &AggCall, children: (Option<Node>, Option<Node>)) -> Result<Option<Node>> {
    match children {
        (None, None) => Ok(None),
        (Some(one), None) | (None, Some(one)) => Ok(Some(one)),
        (Some(first), Some(second)) => {
            let mut acc = fresh(call);
            acc.merge(&first.state)?;
            acc.merge(&second.state)?;
            written(acc.as_ref(), first.nulls || second.nulls).map(Some)
        }
    }
}

/// The value of `call` over the ranges of `frame`, of the tree `nodes`
/// whose leaves are from `size` on: the states covering each range merged
/// first to last into a new accumulator; and whether it left out a null.
fn merged(call: &AggCall, (nodes, size): (&[Option<Node>], usize), frame: &Ranges) -> Result<(Value, bool)> {
    let (mut acc, mut nulls) = (fresh(call), false);
    for &(from, to) in frame.iter().filter(|r| **r != EMPTY) {
        let mut take = |at: usize| -> Result<()> {
            if let Some(node) = nodes.get(at).ok_or_else(|| broken("a tree of states has no node of its place"))? {
                acc.merge(&node.state)?;
                nulls |= node.nulls;
            }
            Ok(())
        };
        let (mut low, mut high) = (from.saturating_add(size), to.saturating_add(size));
        let (mut later, mut count) = ([0_usize; MOST_LEVELS], 0_usize);
        while low < high {
            if low & 1 == 1 {
                take(low)?;
                low = low.saturating_add(1);
            }
            if high & 1 == 1 {
                high = high.saturating_sub(1);
                *later.get_mut(count).ok_or_else(|| broken("a tree of states deeper than a place has bits"))? = high;
                count = count.saturating_add(1);
            }
            (low, high) = (low >> 1_u32, high >> 1_u32);
        }
        later.get(..count).unwrap_or_default().iter().rev().try_for_each(|&at| take(at))?;
    }
    Ok((acc.finish()?, nulls))
}

// Unit tests: each way a set function's frames are computed against a new
// accumulator fed each frame (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
