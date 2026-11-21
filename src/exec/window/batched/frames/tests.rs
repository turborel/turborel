use super::*;
use crate::exec::HeldRows;
use crate::exec::window::frame::{Bounds, Partition};
use crate::exec::window::reader::Placed;
use crate::spill::ListSpace;
use crate::value::Decimal;

/// A step of a xorshift generator from `state`: the same cases every run
/// (RULES.md R-TEST-5).
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    *state
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// Sorted range keys of `len` rows from `state`, a few null at the start
/// or the end, peers among them.
fn keys(state: &mut u64, len: usize, order: SortOrder) -> Vec<Value> {
    let mut ks: Vec<i64> = (0..len).map(|_| i64::try_from(next(state) % 6).unwrap()).collect();
    ks.sort_unstable();
    if order.descending {
        ks.reverse();
    }
    let nulls = usize::try_from(next(state) % 3).unwrap().min(len);
    let mut out: Vec<Value> = ks.into_iter().map(int).collect();
    for v in out.iter_mut().take(nulls) {
        *v = Value::Null;
    }
    if !order.nulls_first {
        out.rotate_left(nulls);
    }
    out
}

/// The partition of `keys` as the rows' form holds it, and as the vectors'.
fn both(keys: &[Value]) -> (Partition, Part) {
    let mut held = HeldRows::new(ListSpace::default());
    let mut starts = Vec::new();
    let mut group = 0_u64;
    for (n, k) in keys.iter().enumerate() {
        if n == 0 || crate::value::sort_cmp(&keys[n - 1], k) != std::cmp::Ordering::Equal {
            group += u64::from(n > 0);
            starts.push(n);
        }
        held.push(Placed { group, values: vec![k.clone()] }).unwrap();
    }
    let len = keys.len();
    let first = keys.iter().position(Value::is_null);
    let past = keys.iter().rposition(Value::is_null).map_or(len, |p| p + 1);
    let nulls = first.map_or((len, len), |f| (f, past));
    (Partition { keys: held, len, nulls }, Part::of(len, starts))
}

/// The edges tried: every kind, offsets of 0 to 2.
fn edges(order: SortOrder) -> Vec<Edge> {
    let mut all = vec![Edge::First, Edge::Last, Edge::Row, Edge::Peers];
    for back in [true, false] {
        for count in 0..3_usize {
            all.push(Edge::Rows { back, count });
            all.push(Edge::Groups { back, count: u64::try_from(count).unwrap() });
            all.push(Edge::Range { back, by: int(i64::try_from(count).unwrap()), order });
        }
    }
    all
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.15 GR5b] each frame found at once,
/// of ROWS, GROUPS and RANGE bounds of every kind, offsets 0 to 2, every
/// exclusion, keys ascending and descending with nulls first and last, is
/// the frame `frame.rs` finds row after row.
#[test]
fn frames_found_at_once_are_those_found_row_after_row() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let exclusions = [Exclusion::NoOthers, Exclusion::CurrentRow, Exclusion::Group, Exclusion::Ties];
    for (descending, nulls_first) in [(false, false), (false, true), (true, false), (true, true)] {
        let order = SortOrder { descending, nulls_first };
        for len in [0_usize, 1, 2, 5, 9] {
            let ks = keys(&mut state, len, order);
            let (mut partition, part) = both(&ks);
            for start in edges(order) {
                for end in edges(order) {
                    for exclusion in exclusions {
                        let found = frames((&start, &end, exclusion), &part, Some((&ks, partition.nulls))).unwrap();
                        let mut bounds = Bounds::new(start.clone(), end.clone(), exclusion);
                        for (row, frame) in found.iter().enumerate() {
                            let mut current = part.current(row);
                            current.key = ks[row].clone();
                            let want = bounds.frame(&current, &mut partition).unwrap();
                            assert_eq!(*frame, want, "{start:?} {end:?} {exclusion:?} row {row} of {ks:?}");
                        }
                    }
                }
            }
        }
    }
}
