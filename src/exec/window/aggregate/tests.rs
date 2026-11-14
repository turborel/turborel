use std::sync::Arc;

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::exec::HeldRows;
use crate::exec::window::frame::Edge;
use crate::exec::window::reader::Placed;
use crate::functions::Registry;
use crate::logical::{Exclusion, Expr};
use crate::spill::ListSpace;
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;
use crate::value::{DataType, Decimal, Spelled};

/// A step of a xorshift generator from `state`: the same cases every run
/// (RULES.md R-TEST-5).
fn below(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    usize::try_from(state.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
}

/// The set function `name` of an exact argument, COUNT(*) if `star`.
fn call(name: &str, (star, distinct): (bool, bool)) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let arg_type = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
    let result = func.0.result_type(if star { None } else { Some(arg_type) }).unwrap();
    let arg = if star { None } else { Some(Expr::Column(0)) };
    AggCall { func, arg, distinct, result, filter: None, distribution: None, label: name.into() }
}

/// A partition of the arguments `args`, each null if none, held in a space
/// that spills past about four rows if `spills`.
fn partition(args: &[Option<i64>], spills: bool) -> Partition {
    let space = if spills {
        let areas = areas(Some(Arc::new(Held { broken: false })), usize::MAX);
        ListSpace { limit: 160, batch: 4096, areas: Some(areas), types: TypeRegistry::default() }
    } else {
        ListSpace::default()
    };
    let mut keys = HeldRows::new(space);
    for a in args {
        let arg = a.map_or(Value::Null, |n| Value::Exact(Decimal::new(i128::from(n), 0)));
        keys.push(Placed { group: 0, values: vec![Value::Null, arg] }).unwrap();
    }
    Partition { keys, len: args.len(), nulls: (args.len(), args.len()) }
}

/// The value of `call` over the rows of `frame` of `args`, fed to a new
/// accumulator, and whether it left out a null.
fn brute(call: &AggCall, args: &[Option<i64>], frame: &Ranges) -> (String, bool) {
    let (mut acc, mut nulls) = (fresh(call), false);
    let mut seen = Vec::new();
    for &(from, to) in frame {
        for a in &args[from..to] {
            match (call.arg.is_some(), a) {
                (false, _) => acc.update(&Value::Null).unwrap(),
                (true, None) => nulls = true,
                (true, Some(n)) => {
                    if !(call.distinct && seen.contains(n)) {
                        seen.push(*n);
                        acc.update(&Value::Exact(Decimal::new(i128::from(*n), 0))).unwrap();
                    }
                }
            }
        }
    }
    (format!("{:?}", acc.finish().unwrap()), nulls)
}

/// Random ranges of `len` places: up to three, apart and in order.
fn ranges(state: &mut u64, len: usize) -> Ranges {
    let mut points: Vec<usize> = (0_u32..6).map(|_| below(state, len.saturating_add(1))).collect();
    points.sort_unstable();
    let range = |a: usize, b: usize| if a < b { (a, b) } else { EMPTY };
    [range(points[0], points[1]), range(points[2], points[3]), range(points[4], points[5])]
}

/// Random arguments, some null.
fn arguments(state: &mut u64) -> Vec<Option<i64>> {
    let n = below(state, 40);
    (0..n).map(|_| if below(state, 4) == 0 { None } else { Some(i64::try_from(below(state, 50)).unwrap()) }).collect()
}

/// The work area of a new database's operator.
fn work() -> Work {
    Work::new(&State::with_builtins(Box::new(MemoryCatalog::default())))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5, §10.9 GR]; [DESIGN]
/// a set function over a frame that moves is its value over the frame's
/// rows, its nulls left out and told of, of a tree of states, a tree given
/// too few bytes to finish, and each frame fed alone; of COUNT(*), COUNT,
/// SUM, AVG, MIN, MAX and ANY_VALUE, whose first value the order of the
/// merges keeps, over frames of up to three ranges, of partitions in
/// memory and spilled, 1,500 fixed cases.
#[test]
fn frames_that_move_are_their_rows_values_7_15() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let work = work();
    let calls = [("COUNT", true), ("COUNT", false), ("SUM", false), ("AVG", false), ("MIN", false), ("MAX", false), ("ANY_VALUE", false)];
    let moving = Bounds::new(Edge::Row, Edge::Last, Exclusion::NoOthers);
    for case in 0_u32..1500 {
        let args = arguments(&mut state);
        let (name, star) = calls[below(&mut state, calls.len())];
        let c = call(name, (star, false));
        let mut part = partition(&args, case % 2 == 1);
        let mut ways = [usize::MAX, 0, 600].map(|bytes| Framed::new(&c, (&moving, 1), &mut part, bytes, &work).unwrap());
        for _ in 0_u32..8 {
            let frame = ranges(&mut state, args.len());
            let want = brute(&c, &args, &frame);
            for (n, way) in ways.iter_mut().enumerate() {
                let (value, nulls) = way.value(frame, &mut part).unwrap();
                assert_eq!((format!("{value:?}"), nulls), want, "case {case}, way {n}: {name} over {frame:?} of {args:?}");
            }
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5a, §10.9 GR]; [DESIGN]
/// a frame that only grows feeds one accumulator the rows its end passes:
/// each frame is its rows' value, of a new partition's accumulator too,
/// and COUNT(DISTINCT X) counts each value once, of the whole partition.
#[test]
fn frames_that_grow_are_their_rows_values_7_15() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let work = work();
    let growing = Bounds::new(Edge::First, Edge::Last, Exclusion::NoOthers);
    for case in 0_u32..600 {
        let args = arguments(&mut state);
        let (name, distinct) = [("SUM", false), ("COUNT", false), ("COUNT", true), ("MIN", false)][below(&mut state, 4)];
        let c = call(name, (false, distinct));
        let mut part = partition(&args, case % 2 == 1);
        let mut framed = Framed::new(&c, (&growing, 1), &mut part, usize::MAX, &work).unwrap();
        let mut to = 0;
        while to < args.len() {
            to = to.saturating_add(below(&mut state, 4)).min(args.len());
            let frame = [if to > 0 { (0, to) } else { EMPTY }, EMPTY, EMPTY];
            let (value, nulls) = framed.value(frame, &mut part).unwrap();
            assert_eq!((format!("{value:?}"), nulls), brute(&c, &args, &frame), "case {case}: {name} up to {to} of {args:?}");
            to = to.saturating_add(1).min(args.len());
        }
    }
}

/// The bytes of a work area whose bag of counts spills past a few values.
const FEW_VALUES: usize = 64;

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR8d ii, §10.9 GR4b,
/// Feature T627]; [DESIGN] COUNT(DISTINCT X) over a frame that moves
/// counts each value of the frame's rows once, its nulls left out and told
/// of: of frames of up to three ranges in any order, so rows are added and
/// taken away, of partitions in memory and spilled, and of a bag of counts
/// in memory and spilled, 1,000 fixed cases.
#[test]
fn count_distinct_over_frames_that_move_7_15() {
    let mut state = 0xD1B5_4A32_D192_ED03_u64;
    let (work, few) = (work(), work().part(FEW_VALUES));
    let c = call("COUNT", (false, true));
    let moving = Bounds::new(Edge::Row, Edge::Last, Exclusion::NoOthers);
    for case in 0_u32..1000 {
        let args = arguments(&mut state);
        let mut part = partition(&args, case % 2 == 1);
        let mut ways = [&work, &few].map(|w| Framed::new(&c, (&moving, 1), &mut part, usize::MAX, w).unwrap());
        for _ in 0_u32..8 {
            let frame = ranges(&mut state, args.len());
            let want = brute(&c, &args, &frame);
            for (n, way) in ways.iter_mut().enumerate() {
                let (value, nulls) = way.value(frame, &mut part).unwrap();
                assert_eq!((format!("{value:?}"), nulls), want, "case {case}, bag {n}: over {frame:?} of {args:?}");
            }
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR8d ii] a DISTINCT set
/// function but COUNT over a frame that moves, which the binder refuses,
/// is SQLCODE -910 here, never a value of the wrong rows.
#[test]
fn distinct_but_count_over_a_moving_frame_is_refused_9_23() {
    let c = call("SUM", (false, true));
    let mut part = partition(&[Some(1)], false);
    let moving = Bounds::new(Edge::Row, Edge::Last, Exclusion::NoOthers);
    let found = Framed::new(&c, (&moving, 1), &mut part, usize::MAX, &work());
    assert_eq!(found.err().map(|e| e.code), Some(crate::error::Code::Api));
}
