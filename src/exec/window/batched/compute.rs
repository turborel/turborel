//! [ISO/IEC 9075-2:2023 §7.15, §6.10] The values of a window's calls over
//! a run of held batches whose every partition is whole (RULES.md D560):
//! each call's arguments set one after another as lanes, then of each
//! partition in turn its peer groups, each call's frames and its values,
//! added in the rows' order to the call's `Built`.

use super::frames::{Keys, Part, frames};
use super::lanes::{Built, Lane};
use super::picks::{offsets, ranks, values};
use super::sets::aggregated;
use super::{Arg, Held};
use crate::error::{Result, SqlState};
use crate::exec::window::frame::{Bounds, Edge};
use crate::exec::window::value::Count;
use crate::exec::{Runner, broken, warn};
use crate::logical::{Window, WindowCall, WindowKind};
use crate::storage::Column;
use crate::value::Value;

/// A window's calls, as prepared for its first partition: each call's
/// frame bounds and count.
pub(in crate::exec::window) type Prepared = Vec<(Bounds, Count)>;

/// The values of `w`'s calls of the rows of `held`, its partitions whole,
/// their calls' frames `prepared`, a tree of at most `bytes` each, warnings
/// heard by `runner`.
///
/// # Errors
///
/// A call's, as a set function or a RANGE bound's arithmetic raises one.
pub(in crate::exec::window) fn computed(
    w: &Window<()>,
    prepared: &Prepared,
    held: &[Held],
    (bytes, runner): (usize, &Runner),
) -> Result<Vec<Built>> {
    let (mut starts, mut peers) = (Vec::new(), Vec::new());
    let mut at = 0_usize;
    for h in held {
        for (r, (&part, &peer)) in h.parts.iter().zip(&h.peers).enumerate() {
            let row = at.saturating_add(r);
            if part || row == 0 {
                starts.push(row);
            }
            if peer || row == 0 {
                peers.push(row);
            }
        }
        at = at.saturating_add(h.batch.rows);
    }
    let len = at;
    let lanes: Vec<Vec<Lane>> = (0..w.calls.len()).map(|c| lanes_of(held, c)).collect();
    let keys = range_keys(w, prepared, held);
    let mut built: Vec<Built> = w.calls.iter().map(|_| Built::default()).collect();
    let mut next_peer = 0_usize;
    for (n, &from) in starts.iter().enumerate() {
        let to = starts.get(n.saturating_add(1)).copied().unwrap_or(len);
        let mut own = Vec::new();
        while let Some(&p) = peers.get(next_peer).filter(|&&p| p < to) {
            own.push(p.saturating_sub(from));
            next_peer = next_peer.saturating_add(1);
        }
        let part = Part::of(to.saturating_sub(from), own);
        let range = keys.as_ref().map(|k| key_block(k, (from, to)));
        for ((call, (bounds, count)), (own, out)) in w.calls.iter().zip(prepared).zip(lanes.iter().zip(built.iter_mut())) {
            let sliced: Vec<Lane> = own.iter().map(|l| l.slice((from, to))).collect();
            let ranged = range.as_ref().map(|(k, nulls)| (k.as_slice(), *nulls));
            value_of(call, (bounds, *count), (&part, &sliced, ranged), (bytes, runner), out)?;
        }
    }
    Ok(built)
}

/// Adds to `out` the values of `call` over `part`, of the frames `bounds`
/// finds and `count`, its arguments `lanes`, the range keys `keys`.
fn value_of(
    call: &WindowCall,
    (bounds, count): (&Bounds, Count),
    (part, lanes, keys): (&Part, &[Lane], Option<Keys<'_>>),
    (bytes, runner): (usize, &Runner),
    out: &mut Built,
) -> Result<()> {
    let first = lanes.first();
    match &call.kind {
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_) => ranks(&call.kind, count, part, out),
        WindowKind::Aggregate(a) => {
            let framed = frames(bounds.edges(), part, keys)?;
            // [ISO/IEC 9075:1992 §6.5 GR1b] a null left out warns.
            if aggregated(a, a.arg.as_ref().and(first), (&framed, bounds.grows()), bytes, out)? {
                warn(runner, SqlState::NULL_VALUE_ELIMINATED);
            }
        }
        WindowKind::Offset(o) => match (first, lanes.get(1)) {
            (Some(extent), Some(defaults)) => offsets(o, (extent, defaults), out),
            (None, _) | (_, None) => return Err(broken("LEAD or LAG with no extent or default")),
        },
        WindowKind::Value(v) => {
            let framed = frames(bounds.edges(), part, keys)?;
            let arg = first.ok_or_else(|| broken("a value function with no argument"))?;
            values((&v.pick, count, v.ignore_nulls), arg, &framed, out);
        }
        WindowKind::Measure(_) => return Err(broken("a row pattern measure computed over batches")),
    }
    Ok(())
}

/// The lanes of call `c`'s arguments over the rows of `held`.
fn lanes_of(held: &[Held], c: usize) -> Vec<Lane> {
    let count = held.first().and_then(|h| h.args.get(c)).map_or(0, Vec::len);
    (0..count)
        .map(|j| {
            let columns: Vec<&Column> = held
                .iter()
                .filter_map(|h| match h.args.get(c)?.get(j)? {
                    Arg::At(at) => h.batch.columns.get(*at),
                    Arg::Made(column) => Some(column),
                })
                .collect();
            Lane::of(&columns)
        })
        .collect()
}

/// The range key of each row of `held`, if a call's frame has a RANGE
/// offset: the value of the window's first ordering key.
fn range_keys(w: &Window<()>, prepared: &Prepared, held: &[Held]) -> Option<Vec<Value>> {
    let ranged = |e: &Edge| matches!(e, Edge::Range { .. });
    if !prepared.iter().any(|(b, _)| ranged(b.edges().0) || ranged(b.edges().1)) {
        return None;
    }
    let column = w.order.first()?.column;
    let columns: Vec<&Column> = held.iter().filter_map(|h| h.batch.columns.get(column)).collect();
    let lane = Lane::of(&columns);
    Some((0..lane.len()).map(|n| lane.value(n)).collect())
}

/// The range keys of the rows from `from` up to `to` of `keys`, and the
/// places of those null among them, a block at their start or end
/// [§10.10 GR1e ii]: `(len, len)` if none is.
fn key_block(keys: &[Value], (from, to): (usize, usize)) -> (Vec<Value>, (usize, usize)) {
    let own = keys.get(from..to).unwrap_or_default().to_vec();
    let len = own.len();
    let first = own.iter().position(Value::is_null);
    let past = own.iter().rposition(Value::is_null).map_or(len, |p| p.saturating_add(1));
    (own, first.map_or((len, len), |f| (f, past)))
}
