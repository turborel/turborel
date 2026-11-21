//! [ISO/IEC 9075-2:2023 §7.15 GR5, §10.9 GR] A set function over each
//! row's frame of a partition held as vectors, as `aggregate.rs` computes
//! one row after row, by the cheapest way its function and argument
//! allow (RULES.md D560). COUNT, and SUM and AVG of exact numbers of one
//! scale, take differences of prefix sums, exact in 128 bits: each frame's
//! count and total found by a subtraction for each of its ranges, then
//! finished as the set function finishes them. MIN and MAX of such
//! numbers read a segment tree of the least or greatest of each span, a
//! frame the best of at most a few nodes for each range, while the tree
//! fits its bytes. SUM and AVG of approximate numbers add them in the
//! order of the rows, as the set function does, since adding them in
//! another order may round otherwise: once along the partition of a frame
//! that only grows, else again for each frame. Any other is computed as
//! `aggregate.rs` computes it, of the values: an accumulator fed the rows
//! a frame that only grows gains, a tree of states, or each frame fed to a
//! new one.

use super::lanes::{Built, Lane};
use crate::error::Result;
use crate::exec::broken;
use crate::exec::window::frame::Ranges;
use crate::functions::{Accumulator, ColumnarKind, NO_FLOATS, Summed, avg_finished, count_finished, plus_float, sum_finished};
use crate::logical::AggCall;
use crate::value::{Decimal, Value};

/// The bytes a node of a tree takes, near enough.
const NODE_BYTES: usize = size_of::<Option<i64>>();

/// The frames of a call over a partition, and whether each only grows.
pub(in crate::exec::window) type Frames<'a> = (&'a [Ranges], bool);

/// Adds to `out` the value of `call` over each of `frames`, its argument
/// `lane`, none of COUNT(*), a tree held if it fits `bytes`; whether one
/// left out a null.
///
/// # Errors
///
/// The set function's, taking a value or finishing.
pub(in crate::exec::window) fn aggregated(
    call: &AggCall,
    lane: Option<&Lane>,
    frames: Frames<'_>,
    bytes: usize,
    out: &mut Built,
) -> Result<bool> {
    let kind = call.func.0.columnar().map(|c| c.0);
    let fits = NODE_BYTES.saturating_mul(frames.0.len().saturating_mul(2)) <= bytes;
    match (kind, lane, fits) {
        (Some(ColumnarKind::Count), _, _) => counts(lane, frames.0, out),
        (Some(ColumnarKind::Sum | ColumnarKind::Avg), Some(Lane::Ints { units, scale, nulls }), _) => {
            totals(call, (units, *scale, nulls), frames.0, out)
        }
        (Some(ColumnarKind::Sum | ColumnarKind::Avg), Some(Lane::Floats { numbers, nulls }), _) => {
            floats(call, (numbers, nulls), frames, out)
        }
        (Some(ColumnarKind::Min | ColumnarKind::Max), Some(Lane::Ints { units, scale, nulls }), true) => {
            extremes(matches!(kind, Some(ColumnarKind::Max)), (units, *scale, nulls), frames.0, out)
        }
        (Some(_) | None, _, _) => super::states::generic(call, lane, frames, bytes, out),
    }
}

/// Of each of `values`, their sums so far: one more than there are, the
/// first 0.
fn prefix<T: Copy>(values: impl Iterator<Item = T>, zero: T, add: impl Fn(T, T) -> T) -> Vec<T> {
    let mut sums = vec![zero];
    let mut sum = zero;
    for v in values {
        sum = add(sum, v);
        sums.push(sum);
    }
    sums
}

/// The rows of `frame`, its ranges' together.
fn rows_of(frame: &Ranges) -> u64 {
    let rows = frame.iter().fold(0_usize, |n, &(from, to)| n.saturating_add(to.saturating_sub(from)));
    u64::try_from(rows).unwrap_or(u64::MAX)
}

/// The sum over `frame` of the values whose sums so far are `sums`.
fn over<T: Copy>(sums: &[T], frame: &Ranges, zero: T, sub: impl Fn(T, T) -> T, add: impl Fn(T, T) -> T) -> T {
    frame.iter().filter(|(from, to)| from < to).fold(zero, |total, &(from, to)| {
        let (a, b) = (sums.get(from).copied().unwrap_or(zero), sums.get(to).copied().unwrap_or(zero));
        add(total, sub(b, a))
    })
}

/// The counts of rows (`lane` none) or of values not null over each frame
/// [ISO/IEC 9075:1992 §6.5 GR4a]; whether a frame held a null.
fn counts(lane: Option<&Lane>, frames: &[Ranges], out: &mut Built) -> Result<bool> {
    let len = frames.len();
    let taken = prefix((0..len).map(|r| u64::from(lane.is_none_or(|l| !l.is_null(r)))), 0_u64, u64::saturating_add);
    let mut nulls = false;
    for frame in frames {
        let n = over(&taken, frame, 0, u64::saturating_sub, u64::saturating_add);
        nulls |= rows_of(frame) > n;
        out.push(count_finished(i128::from(n))?);
    }
    Ok(nulls)
}

/// Exact numbers: their units, scale and which are null.
type Exact<'a> = (&'a [i64], u32, &'a [bool]);

/// SUM or AVG, `call`, of exact numbers over each frame, by prefix sums
/// [ISO/IEC 9075:1992 §6.5 GR2b]; whether a frame left out a null.
fn totals(call: &AggCall, (units, scale, nulls): Exact<'_>, frames: &[Ranges], out: &mut Built) -> Result<bool> {
    let sums = prefix(units.iter().map(|&u| i128::from(u)), 0_i128, i128::saturating_add);
    let taken = prefix(nulls.iter().map(|&n| u64::from(!n)), 0_u64, u64::saturating_add);
    let avg = matches!(call.func.0.columnar().map(|c| c.0), Some(ColumnarKind::Avg));
    let mut left_out = false;
    for frame in frames {
        let n = over(&taken, frame, 0, u64::saturating_sub, u64::saturating_add);
        left_out |= rows_of(frame) > n;
        let total = if n == 0 {
            Summed::Empty
        } else {
            Summed::Exact(Decimal::new(over(&sums, frame, 0, i128::saturating_sub, i128::saturating_add), scale))
        };
        out.push(if avg { avg_finished(total, i128::from(n), &call.result)? } else { sum_finished(total, &call.result)? });
    }
    Ok(left_out)
}

/// SUM or AVG, `call`, of approximate numbers over each frame, added in the
/// order of the rows; whether a frame left out a null.
fn floats(call: &AggCall, (numbers, nulls): (&[f64], &[bool]), (frames, grows): Frames<'_>, out: &mut Built) -> Result<bool> {
    let avg = matches!(call.func.0.columnar().map(|c| c.0), Some(ColumnarKind::Avg));
    let finish = |sum, taken: u64, built: &mut Built| -> Result<()> {
        let total = if taken == 0 { Summed::Empty } else { Summed::Approx(sum) };
        built.push(if avg { avg_finished(total, i128::from(taken), &call.result)? } else { sum_finished(total, &call.result)? });
        Ok(())
    };
    let (mut sum, mut n, mut fed, mut left_out) = (NO_FLOATS, 0_u64, 0_usize, false);
    for frame in frames {
        if grows {
            let to = frame.first().map_or(0, |r| r.1);
            for (&f, &null) in numbers.iter().zip(nulls).take(to).skip(fed) {
                left_out |= null;
                if !null {
                    (sum, n) = (plus_float(sum, f), n.saturating_add(1));
                }
            }
            fed = fed.max(to);
            finish(sum, n, out)?;
            continue;
        }
        let (mut each, mut count) = (NO_FLOATS, 0_u64);
        for &(from, to) in frame {
            for (&f, &null) in numbers.iter().zip(nulls).take(to).skip(from) {
                left_out |= null;
                if !null {
                    (each, count) = (plus_float(each, f), count.saturating_add(1));
                }
            }
        }
        finish(each, count, out)?;
    }
    Ok(left_out)
}

/// MAX if `max`, else MIN, of exact numbers over each frame, of a segment
/// tree of the best of each span [ISO/IEC 9075:1992 §6.5 GR2b iii];
/// whether a frame left out a null.
fn extremes(max: bool, (units, scale, nulls): Exact<'_>, frames: &[Ranges], out: &mut Built) -> Result<bool> {
    let best = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (Some(x), Some(y)) => Some(if max { x.max(y) } else { x.min(y) }),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let size = units.len().max(1).checked_next_power_of_two().ok_or_else(|| broken("a partition too large for a tree"))?;
    let mut nodes: Vec<Option<i64>> = vec![None; size.saturating_mul(2)];
    for ((node, &u), &null) in nodes.iter_mut().skip(size).zip(units).zip(nulls) {
        *node = (!null).then_some(u);
    }
    for at in (1..size).rev() {
        let pair =
            (nodes.get(at.saturating_mul(2)).copied().flatten(), nodes.get(at.saturating_mul(2).saturating_add(1)).copied().flatten());
        if let Some(node) = nodes.get_mut(at) {
            *node = best(pair.0, pair.1);
        }
    }
    let taken = prefix(nulls.iter().map(|&n| u64::from(!n)), 0_u64, u64::saturating_add);
    let mut left_out = false;
    for frame in frames {
        let mut found = None;
        for &(from, to) in frame.iter().filter(|(from, to)| from < to) {
            let (mut low, mut high) = (from.saturating_add(size), to.saturating_add(size));
            while low < high {
                if low & 1 == 1 {
                    found = best(found, nodes.get(low).copied().flatten());
                    low = low.saturating_add(1);
                }
                if high & 1 == 1 {
                    high = high.saturating_sub(1);
                    found = best(found, nodes.get(high).copied().flatten());
                }
                (low, high) = (low >> 1_u32, high >> 1_u32);
            }
        }
        left_out |= rows_of(frame) > over(&taken, frame, 0, u64::saturating_sub, u64::saturating_add);
        out.push(found.map_or(Value::Null, |u| Value::Exact(Decimal::new(i128::from(u), scale))));
    }
    Ok(left_out)
}

/// A new accumulator of `call`, fed nothing.
pub(in crate::exec::window) fn fresh(call: &AggCall) -> Box<dyn Accumulator> {
    call.func.0.accumulator(call.result.clone())
}
