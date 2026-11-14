use std::sync::Arc;

use super::*;
use crate::spill::ListSpace;
use crate::spill::testing::{Held, areas};
use crate::value::Decimal;
use crate::value::types::TypeRegistry;

/// A step of a xorshift generator from `state`: the same cases every run
/// (RULES.md R-TEST-5).
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    *state
}

/// A number below `n` from `state`.
fn below(state: &mut u64, n: usize) -> usize {
    usize::try_from(next(state).checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
}

/// `n`, signed.
fn signed(n: usize) -> i64 {
    i64::try_from(n).unwrap()
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A key's value: null if none.
fn value(k: Option<i64>) -> Value {
    k.map_or(Value::Null, int)
}

/// The peer group of `row` of `keys`, from 0.
fn group_of(keys: &[Option<i64>], row: usize) -> usize {
    (1..=row).filter(|&j| keys[j.saturating_sub(1)] != keys[j]).count()
}

/// A partition of the range keys `keys`, in order, each null if none,
/// peers those of one key, held in `space`.
fn partition(keys: &[Option<i64>], space: ListSpace) -> Partition {
    let mut held = HeldRows::new(space);
    for (n, k) in keys.iter().enumerate() {
        let group = u64::try_from(group_of(keys, n)).unwrap();
        held.push(Placed { group, values: vec![value(*k)] }).unwrap();
    }
    let len = keys.len();
    let first = keys.iter().position(Option::is_none).unwrap_or(len);
    let last = keys.iter().rposition(Option::is_none).map_or(len, |p| p.saturating_add(1));
    Partition { keys: held, len, nulls: (first, last) }
}

/// A space holding about four rows before it spills, or one that never
/// does.
fn space(spills: bool) -> ListSpace {
    if !spills {
        return ListSpace::default();
    }
    let areas = areas(Some(Arc::new(Held { broken: false })), usize::MAX);
    ListSpace { limit: 160, batch: 4096, areas: Some(areas), types: TypeRegistry::default() }
}

/// The current row `row` of `keys`.
fn current(keys: &[Option<i64>], row: usize) -> Current {
    let key = keys[row];
    let from = keys.iter().position(|k| *k == key).unwrap();
    let to = keys.iter().rposition(|k| *k == key).unwrap().saturating_add(1);
    Current { row, group: u64::try_from(group_of(keys, row)).unwrap(), peers: (from, to), key: value(key) }
}

/// A bound as the brute force reads it: its kind and offset.
#[derive(Debug, Clone, Copy, PartialEq)]
enum B {
    UnboundedPreceding,
    Preceding(i64),
    CurrentRow,
    Following(i64),
    UnboundedFollowing,
}

/// What the brute force reads of a case: the keys, the current row, the
/// units, and the order of the range key.
struct Case<'a> {
    keys: &'a [Option<i64>],
    row: usize,
    units: Units,
    order: SortOrder,
}

/// `at` moved by the offset of `bound`, PRECEDING less.
fn moved_by(at: i64, bound: B, k: i64) -> i64 {
    if matches!(bound, B::Preceding(_)) { at.saturating_sub(k) } else { at.saturating_add(k) }
}

/// Whether `at` is not before `target` for a start, not after it for an
/// end.
fn within(at: i64, target: i64, start: bool) -> bool {
    if start { at >= target } else { at <= target }
}

/// [ISO/IEC 9075-2:2023 §7.15 GR5b i 1] Whether a RANGE bound `bound` of
/// offset `k`, the start if `start`, leaves row `j`.
fn range_keeps(c: &Case<'_>, (bound, offset, start): (B, i64, bool), j: usize) -> bool {
    // Of a current row whose key is null, a start of NULLS LAST and an end
    // of NULLS FIRST remove the rows whose key is not; of any other, the
    // others remove the rows whose key is null.
    let null_side = if start { !c.order.nulls_first } else { c.order.nulls_first };
    let (Some(own), Some(theirs)) = (c.keys[c.row], c.keys[j]) else {
        return if c.keys[c.row].is_none() { !null_side || c.keys[j].is_none() } else { null_side };
    };
    let up = matches!(bound, B::Following(_)) != c.order.descending;
    let target = if up { own.saturating_add(offset) } else { own.saturating_sub(offset) };
    // Descending, a start keeps the keys not above its target, an end those
    // not below it.
    within(theirs, target, start != c.order.descending)
}

/// [ISO/IEC 9075-2:2023 §7.15 GR5b i] Whether the bound `bound`, the
/// start if `start`, leaves row `j`.
fn keeps(c: &Case<'_>, (bound, start): (B, bool), j: usize) -> bool {
    let current = current(c.keys, c.row);
    let k = match bound {
        B::UnboundedPreceding | B::UnboundedFollowing => return true,
        B::CurrentRow => {
            return match c.units {
                Units::Rows => within(signed(j), signed(c.row), start),
                Units::Range | Units::Groups => {
                    if start {
                        j >= current.peers.0
                    } else {
                        j < current.peers.1
                    }
                }
            };
        }
        B::Preceding(k) | B::Following(k) => k,
    };
    match c.units {
        Units::Rows => within(signed(j), moved_by(signed(c.row), bound, k), start),
        Units::Groups => within(signed(group_of(c.keys, j)), moved_by(signed(group_of(c.keys, c.row)), bound, k), start),
        Units::Range => range_keeps(c, (bound, k, start), j),
    }
}

/// [ISO/IEC 9075-2:2023 §7.15 GR5b] The frame of the case `c`, as the
/// rules remove rows from its partition one by one.
fn brute(c: &Case<'_>, (start, end, exclusion): (B, B, Exclusion)) -> Vec<usize> {
    let peers = current(c.keys, c.row).peers;
    let others = |j: usize| j < peers.0 || j >= peers.1;
    (0..c.keys.len())
        .filter(|&j| keeps(c, (start, true), j) && keeps(c, (end, false), j))
        .filter(|&j| match exclusion {
            Exclusion::NoOthers => true,
            Exclusion::CurrentRow => j != c.row,
            Exclusion::Group => others(j),
            Exclusion::Ties => j == c.row || others(j),
        })
        .collect()
}

/// The edge of the brute force's bound `b`.
fn edge_of(b: B, units: Units, order: SortOrder) -> Edge {
    let offset = |back: bool, k: i64| match units {
        Units::Rows => Edge::Rows { back, count: usize::try_from(k).unwrap() },
        Units::Groups => Edge::Groups { back, count: u64::try_from(k).unwrap() },
        Units::Range => Edge::Range { back, by: int(k), order },
    };
    match b {
        B::UnboundedPreceding => Edge::First,
        B::UnboundedFollowing => Edge::Last,
        B::CurrentRow => match units {
            Units::Rows => Edge::Row,
            Units::Range | Units::Groups => Edge::Peers,
        },
        B::Preceding(k) => offset(true, k),
        B::Following(k) => offset(false, k),
    }
}

/// Random keys, in `order`, some null, alike often.
fn keys_of(state: &mut u64, order: SortOrder) -> Vec<Option<i64>> {
    let n = below(state, 24);
    let mut keys: Vec<Option<i64>> = (0..n).map(|_| if below(state, 5) == 0 { None } else { Some(signed(below(state, 9))) }).collect();
    keys.sort_by(|a, b| order.cmp(&value(*a), &value(*b)));
    keys
}

/// A random bound of offsets up to 3.
fn bound_of(state: &mut u64) -> B {
    let k = signed(below(state, 4));
    [B::UnboundedPreceding, B::Preceding(k), B::CurrentRow, B::Following(k), B::UnboundedFollowing][below(state, 5)]
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b] the frames the
/// pointers find row after row are those the General Rules leave of each
/// row's partition, for ROWS, RANGE and GROUPS, every pair of bounds, every
/// exclusion, the key ascending or descending and nulls first or last, of
/// partitions held in memory and spilled, 4,000 fixed cases.
#[test]
fn frames_are_those_the_rules_leave_7_15() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for case in 0_u32..4000 {
        let order = SortOrder { descending: below(&mut state, 2) == 1, nulls_first: below(&mut state, 2) == 1 };
        let keys = keys_of(&mut state, order);
        let units = [Units::Rows, Units::Range, Units::Groups][below(&mut state, 3)];
        let (start, end) = (bound_of(&mut state), bound_of(&mut state));
        let exclusion = [Exclusion::NoOthers, Exclusion::CurrentRow, Exclusion::Group, Exclusion::Ties][below(&mut state, 4)];
        // [SR10a, SR10b] Neither is refused here, but neither is written.
        if start == B::UnboundedFollowing || end == B::UnboundedPreceding {
            continue;
        }
        let mut part = partition(&keys, space(case % 2 == 1));
        let mut bounds = Bounds::new(edge_of(start, units, order), edge_of(end, units, order), exclusion);
        for row in 0..keys.len() {
            let frame = bounds.frame(&current(&keys, row), &mut part).unwrap();
            let found: Vec<usize> = frame.iter().flat_map(|&(a, b)| a..b).collect();
            let want = brute(&Case { keys: &keys, row, units, order }, (start, end, exclusion));
            assert_eq!(found, want, "case {case}: {units:?} {start:?} {end:?} {exclusion:?} {order:?} of {keys:?}, row {row}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b ii] each
/// exclusion takes from the frame what it says and no more: the current
/// row, its peers, or its peers but itself; a frame of none of them is
/// left whole, and what is left is in order, in at most three ranges.
#[test]
fn exclusions_cut_the_frame_7_15() {
    let current = Current { row: 4, group: 1, peers: (3, 6), key: Value::Null };
    assert_eq!(excluded((0, 8), &current, Exclusion::NoOthers), [(0, 8), EMPTY, EMPTY]);
    assert_eq!(excluded((0, 8), &current, Exclusion::CurrentRow), [(0, 4), (5, 8), EMPTY]);
    assert_eq!(excluded((0, 8), &current, Exclusion::Group), [(0, 3), (6, 8), EMPTY]);
    assert_eq!(excluded((0, 8), &current, Exclusion::Ties), [(0, 3), (4, 5), (6, 8)]);
    assert_eq!(excluded((6, 8), &current, Exclusion::Ties), [EMPTY, EMPTY, (6, 8)]);
    assert_eq!(excluded((5, 5), &current, Exclusion::NoOthers), [EMPTY, EMPTY, EMPTY]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5b i 1 B I 1] an
/// offset negative or null is SQLSTATE 22013, of every unit; one of 0 is
/// taken.
#[test]
fn an_offset_negative_or_null_is_22013_7_15() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, crate::txn::NO_TRANSACTION);
    let order = Some(SortOrder::of(false, None));
    for units in [Units::Rows, Units::Range, Units::Groups] {
        for (offset, fails) in [(Value::Null, true), (int(-1), true), (int(0), false)] {
            let frame = Frame {
                units,
                start: Bound::Preceding(crate::logical::Expr::Const(offset)),
                end: Bound::CurrentRow,
                exclusion: Exclusion::NoOthers,
            };
            let found = Bounds::of(&frame, order, &runner);
            assert_eq!(found.is_err(), fails, "{units:?}");
            if let Err(e) = found {
                assert_eq!(e.state, Some(SqlState::INVALID_PRECEDING_OR_FOLLOWING_SIZE));
            }
        }
    }
}
