//! [ISO/IEC 9075-2:2023 §7.15 GR5, §10.9 GR] A set function no vector of
//! numbers computes, over each frame of a partition held as vectors, as
//! `aggregate.rs` computes it row after row (RULES.md D560): of a frame
//! that only grows, one accumulator fed the rows its end passes, finished
//! for each row; of a set function whose state can be written, a segment
//! tree of states, each the merge of its two children's, while it fits
//! its bytes, each frame the merge of a few states for each of its ranges,
//! first to last; else each frame fed to a new accumulator. A frame alike
//! to the row before it gives the value it gave.

use super::lanes::{Built, Lane};
use super::sets::{Frames, fresh};
use crate::error::Result;
use crate::exec::broken;
use crate::exec::window::frame::{EMPTY, Ranges};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::value::Value;

/// The most levels a tree of states has: one more than the bits of a place.
const MOST_LEVELS: usize = 65;

/// The bytes a node of a tree of states takes beside its state, near enough.
const NODE_BYTES: usize = size_of::<Option<(Vec<u8>, bool)>>();

/// A node of a tree of states: the state of the rows below it, and whether
/// one of their arguments was null.
type Node = Option<(Vec<u8>, bool)>;

/// Adds to `out` the value of `call` over each of `frames`, its argument
/// `lane`, none of COUNT(*), a tree of states held if it fits `bytes`;
/// whether one left out a null.
///
/// # Errors
///
/// The set function's, taking a value or finishing.
pub(in crate::exec::window) fn generic(
    call: &AggCall,
    lane: Option<&Lane>,
    (frames, grows): Frames<'_>,
    bytes: usize,
    out: &mut Built,
) -> Result<bool> {
    let mut left_out = false;
    let mut last: Option<(Ranges, Value)> = None;
    let tree = if grows { None } else { tree(call, lane, frames.len(), bytes)? };
    let (mut acc, mut fed) = (fresh(call), 0_usize);
    for frame in frames {
        if let Some((_, value)) = last.as_ref().filter(|(before, _)| before == frame) {
            out.push(value.clone());
            continue;
        }
        let value = if grows {
            let to = frame.first().map_or(0, |r| r.1);
            while fed < to {
                left_out |= take(acc.as_mut(), lane, fed)?;
                fed = fed.saturating_add(1);
            }
            acc.finish()?
        } else if let Some((nodes, size)) = &tree {
            let (value, nulls) = merged(call, (nodes, *size), frame)?;
            left_out |= nulls;
            value
        } else {
            let mut each = fresh(call);
            for &(from, to) in frame {
                for at in from..to {
                    left_out |= take(each.as_mut(), lane, at)?;
                }
            }
            each.finish()?
        };
        out.push(value.clone());
        last = Some((*frame, value));
    }
    Ok(left_out)
}

/// [ISO/IEC 9075:1992 §6.5 GR1b] Feeds `acc` the `at`th argument of
/// `lane`, a row of COUNT(*) if none, nothing if null; whether it was.
fn take(acc: &mut dyn Accumulator, lane: Option<&Lane>, at: usize) -> Result<bool> {
    let Some(lane) = lane else {
        acc.update(&Value::Null)?;
        return Ok(false);
    };
    if lane.is_null(at) {
        return Ok(true);
    }
    acc.update(&lane.value(at))?;
    Ok(false)
}

/// The state of `acc`, which left out a null if `nulls`.
fn written(acc: &dyn Accumulator, nulls: bool) -> Result<Node> {
    let mut state = Vec::new();
    if !acc.write_state(&mut state) {
        return Err(broken("a set function's state was written once and not again"));
    }
    Ok(Some((state, nulls)))
}

/// A tree of the states of `call` over `len` rows of `lane`: leaves from
/// the returned size on, each of a row, each node before them the merge of
/// its two children. None if its state cannot be written, or the tree
/// passes `bytes`.
fn tree(call: &AggCall, lane: Option<&Lane>, len: usize, bytes: usize) -> Result<Option<(Vec<Node>, usize)>> {
    if !fresh(call).write_state(&mut Vec::new()) {
        return Ok(None);
    }
    let size = len.max(1).checked_next_power_of_two().ok_or_else(|| broken("a partition too large for a tree of states"))?;
    let Some(mut held) = NODE_BYTES.checked_mul(size.saturating_mul(2)).filter(|&b| b <= bytes) else { return Ok(None) };
    let mut nodes: Vec<Node> = vec![None; size.saturating_mul(2)];
    for (place, leaf) in nodes.iter_mut().skip(size).take(len).enumerate() {
        let mut acc = fresh(call);
        let nulls = take(acc.as_mut(), lane, place)?;
        *leaf = written(acc.as_ref(), nulls)?;
        held = held.saturating_add(leaf.as_ref().map_or(0, |n| n.0.len()));
        if held > bytes {
            return Ok(None);
        }
    }
    for at in (1..size).rev() {
        let first = nodes.get(at.saturating_mul(2)).cloned().flatten();
        let second = nodes.get(at.saturating_mul(2).saturating_add(1)).cloned().flatten();
        let node = match (first, second) {
            (None, None) => None,
            (Some(one), None) | (None, Some(one)) => Some(one),
            (Some(a), Some(b)) => {
                let mut acc = fresh(call);
                acc.merge(&a.0)?;
                acc.merge(&b.0)?;
                written(acc.as_ref(), a.1 || b.1)?
            }
        };
        held = held.saturating_add(node.as_ref().map_or(0, |n| n.0.len()));
        if held > bytes {
            return Ok(None);
        }
        if let Some(parent) = nodes.get_mut(at) {
            *parent = node;
        }
    }
    Ok(Some((nodes, size)))
}

/// The value of `call` over the ranges of `frame`, of the tree `nodes`
/// whose leaves are from `size` on: the states covering each range merged
/// first to last into a new accumulator; and whether it left out a null.
fn merged(call: &AggCall, (nodes, size): (&[Node], usize), frame: &Ranges) -> Result<(Value, bool)> {
    let (mut acc, mut nulls) = (fresh(call), false);
    for &(from, to) in frame.iter().filter(|r| **r != EMPTY) {
        let mut take = |at: usize| -> Result<()> {
            if let Some((state, null)) = nodes.get(at).ok_or_else(|| broken("a tree of states has no node of its place"))? {
                acc.merge(state)?;
                nulls |= *null;
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
