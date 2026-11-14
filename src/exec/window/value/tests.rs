use std::sync::Arc;

use super::*;
use crate::logical::{Expr, Offset, Valued};
use crate::spill::ListSpace;
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;

/// A step of a xorshift generator from `state`, then a number below `n`:
/// the same cases every run (RULES.md R-TEST-5).
fn below(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    usize::try_from(state.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A space holding about four rows before it spills if `spills`, or one
/// that never does.
fn space(spills: bool) -> ListSpace {
    if !spills {
        return ListSpace::default();
    }
    let areas = areas(Some(Arc::new(Held { broken: false })), usize::MAX);
    ListSpace { limit: 160, batch: 4096, areas: Some(areas), types: TypeRegistry::default() }
}

/// Row `row`'s default: a value of its own, so a default of another row
/// is told apart.
fn default_of(row: usize) -> Value {
    int((-1000_i64).checked_sub(i64::try_from(row).unwrap()).unwrap())
}

/// A partition of the arguments `args`, each null if none, of one call at
/// slot 1, as `exec/window.rs` holds it: the argument, the row's default if
/// `defaulted`, and the count of arguments not null before the row if
/// `ignoring`; and the call's sequence.
fn partition(args: &[Option<i64>], (defaulted, ignoring): (bool, bool), spills: bool) -> (Partition, Sequence) {
    let (mut keys, mut not_null) = (HeldRows::new(space(spills)), NotNull { values: HeldRows::new(space(spills)), len: 0 });
    for (row, a) in args.iter().enumerate() {
        let arg = a.map_or(Value::Null, int);
        let mut values = vec![Value::Null, arg.clone()];
        if defaulted {
            values.push(default_of(row));
        }
        if ignoring {
            values.push(counted(not_null.len));
            if !arg.is_null() {
                not_null.values.push(Placed { group: 0, values: vec![arg] }).unwrap();
                not_null.len = not_null.len.checked_add(1).unwrap();
            }
        }
        keys.push(Placed { group: 0, values }).unwrap();
    }
    let sequence = if ignoring { Sequence::NotNull(not_null) } else { Sequence::Each };
    (Partition { keys, len: args.len(), nulls: (args.len(), args.len()) }, sequence)
}

/// Random arguments, about one in three null.
fn arguments(state: &mut u64) -> Vec<Option<i64>> {
    let len = below(state, 12);
    (0..len).map(|_| if below(state, 3) == 0 { None } else { Some(i64::try_from(below(state, 50)).unwrap()) }).collect()
}

/// The values of the rows `places`, in order, of `args`, the nulls left
/// out if `ignoring`.
fn sequence_of(args: &[Option<i64>], places: impl Iterator<Item = usize>, ignoring: bool) -> Vec<Value> {
    places.filter(|&p| !ignoring || args[p].is_some()).map(|p| args[p].map_or(Value::Null, int)).collect()
}

/// [ISO/IEC 9075-2:2023 §6.10 GR1b] LEAD or LAG of `row`, as the rule
/// reads.
fn brute_offset(args: &[Option<i64>], row: usize, (lead, offset, ignoring): (bool, usize, bool)) -> Value {
    if offset == 0 {
        return args[row].map_or(Value::Null, int);
    }
    if lead {
        let tx = sequence_of(args, row.checked_add(1).unwrap()..args.len(), ignoring);
        tx.get(offset.checked_sub(1).unwrap()).cloned().unwrap_or_else(|| default_of(row))
    } else {
        let tx = sequence_of(args, 0..row, ignoring);
        tx.len().checked_sub(offset).map_or_else(|| default_of(row), |at| tx[at].clone())
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1b] LEAD and LAG take
/// the extent of the row `offset` rows after or before the current one,
/// counting only rows whose extent is not null under IGNORE NULLS; offset
/// 0 takes the current row's own extent, null or not; past the
/// partition's end, the current row's default. Against the rule read
/// directly, over random partitions in memory and spilled.
#[test]
fn lead_and_lag_take_the_row_offset_rows_away_6_10() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for case in 0_u32..1200 {
        let (lead, ignoring, spills) = (case % 2_u32 == 0, case % 3_u32 == 0, case % 5_u32 == 0);
        let offset = below(&mut state, 5);
        let args = arguments(&mut state);
        let kind = WindowKind::Offset(Offset {
            lead,
            extent: Expr::Column(0),
            offset: u32::try_from(offset).unwrap(),
            default: Expr::Const(Value::Null),
            ignore_nulls: ignoring,
        });
        let (mut part, sequence) = partition(&args, (true, ignoring), spills);
        let mut picked = Picked::new(&kind, 1, Count::Unused, sequence);
        for row in 0..args.len() {
            let found = picked.offset(row, &mut part).unwrap();
            let expected = brute_offset(&args, row, (lead, offset, ignoring));
            assert_eq!(format!("{found:?}"), format!("{expected:?}"), "{args:?} row {row} lead {lead} offset {offset} ignoring {ignoring}");
        }
    }
}

/// A random frame of a partition of `len` rows: at most three ranges in
/// order, apart, some empty.
fn frame(state: &mut u64, len: usize) -> Ranges {
    let mut points: Vec<usize> = (0_u32..6).map(|_| below(state, len.checked_add(1).unwrap())).collect();
    points.sort_unstable();
    let mut ranges = [EMPTY; 3];
    for (n, range) in ranges.iter_mut().enumerate() {
        if below(state, 4) != 0 {
            let from = n.checked_mul(2).unwrap();
            *range = (points[from], points[from.checked_add(1).unwrap()]);
        }
    }
    ranges
}

/// [ISO/IEC 9075-2:2023 §6.10 GR1c, GR1d] The value `pick` takes of
/// `frame`, as the rules read.
fn brute_pick(args: &[Option<i64>], frame: &Ranges, (pick, n, ignoring): (&Pick, Count, bool)) -> Value {
    let places = frame.iter().filter(|r| **r != EMPTY).flat_map(|&(from, to)| from..to);
    let tx = sequence_of(args, places, ignoring);
    let at = match (pick, n) {
        (Pick::First, _) => 0,
        (Pick::Last, _) => tx.len().wrapping_sub(1),
        (Pick::Nth { from_last: false, .. }, Count::Of(n)) => n.checked_sub(1).unwrap(),
        (Pick::Nth { from_last: true, .. }, Count::Of(n)) => tx.len().wrapping_sub(n),
        (Pick::Nth { .. }, Count::Null | Count::Unused) => return Value::Null,
    };
    tx.get(at).cloned().unwrap_or(Value::Null)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1c, GR1d]
/// FIRST_VALUE, LAST_VALUE and NTH_VALUE take the first, the last and the
/// nth argument of the frame's rows, from its first or its last, over the
/// at most three ranges an exclusion leaves, nulls left out under IGNORE
/// NULLS, null of no such row. Against the rules read directly, over
/// random partitions and frames in memory and spilled.
#[test]
fn value_functions_take_their_row_of_the_frame_6_10() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for case in 0_u32..1500 {
        let (ignoring, spills) = (case % 2_u32 == 0, case % 7_u32 == 0);
        let n = below(&mut state, 5).checked_add(1).unwrap();
        let pick = match case % 4_u32 {
            0_u32 => Pick::First,
            1_u32 => Pick::Last,
            2_u32 => Pick::Nth { row: Expr::Const(Value::Null), from_last: false },
            _ => Pick::Nth { row: Expr::Const(Value::Null), from_last: true },
        };
        let count = if matches!(pick, Pick::Nth { .. }) { Count::Of(n) } else { Count::Unused };
        let args = arguments(&mut state);
        let kind = WindowKind::Value(Valued { pick: pick.clone(), arg: Expr::Column(0), ignore_nulls: ignoring });
        let (mut part, sequence) = partition(&args, (false, ignoring), spills);
        let mut picked = Picked::new(&kind, 1, count, sequence);
        for _ in 0..args.len().max(1) {
            let f = frame(&mut state, args.len());
            let found = picked.framed(f, &mut part).unwrap();
            let expected = brute_pick(&args, &f, (&pick, count, ignoring));
            assert_eq!(format!("{found:?}"), format!("{expected:?}"), "{args:?} {f:?} {pick:?} {count:?} ignoring {ignoring}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1d ii 1] NTH_VALUE of
/// an `<nth row>` null is null, whatever the frame.
#[test]
fn nth_value_of_a_null_row_is_null_6_10() {
    let kind = WindowKind::Value(Valued {
        pick: Pick::Nth { row: Expr::Const(Value::Null), from_last: false },
        arg: Expr::Column(0),
        ignore_nulls: false,
    });
    let (mut part, sequence) = partition(&[Some(1), Some(2)], (false, false), false);
    let mut picked = Picked::new(&kind, 1, Count::Null, sequence);
    assert!(picked.framed([(0, 2), EMPTY, EMPTY], &mut part).unwrap().is_null());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 GR1a ii 3] NTILE's tile
/// of each row is the i whose STARTi ≤ ROWNUM ≤ ENDi, the tiles' sizes NQi
/// built as GR1a ii 3 F and G build them: of every partition of up to 30
/// rows cut into 1 to 35 tiles.
#[test]
fn ntile_cuts_the_first_tiles_longer_6_10() {
    for len in 0..=30_usize {
        for tiles in 1..=35_usize {
            let sizes: Vec<usize> = (1..=tiles)
                .map(|i| if len % tiles == 0 || i > len % tiles { len.div_euclid(tiles) } else { len.div_ceil(tiles) })
                .collect();
            let mut end = 0;
            for (i, size) in sizes.iter().enumerate() {
                let start = end + 1;
                end += size;
                for rownum in start..=end {
                    assert_eq!(tile(rownum - 1, len, tiles), i + 1, "row {rownum} of {len} in {tiles} tiles");
                }
            }
            assert_eq!(end, len);
        }
    }
}
