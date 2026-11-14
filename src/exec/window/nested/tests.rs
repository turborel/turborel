use std::sync::Arc;

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::exec::window::frame::Edge;
use crate::functions::Registry;
use crate::logical::{Bound, Exclusion, Frame, Units};
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;
use crate::value::{ArithOp, DataType, Spelled};

/// A step of a xorshift generator from `state`: the same cases every run
/// (RULES.md R-TEST-5).
fn below(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    usize::try_from(state.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
}

/// The exact number `n`, null if none.
fn exact(n: Option<i128>) -> Value {
    n.map_or(Value::Null, |n| Value::Exact(Decimal::new(n, 0)))
}

/// The markers, each by itself.
const MARKERS: [Marker; 6] =
    [Marker::BeginPartition, Marker::BeginFrame, Marker::CurrentRow, Marker::FrameRow, Marker::EndFrame, Marker::EndPartition];

/// A `<row marker delta>` of a case, as written.
#[derive(Debug, Clone, Copy)]
enum Delta {
    /// None written.
    Unwritten,
    /// Of a null offset.
    Null,
    /// Of the offset, signed.
    By(i128),
}

/// A nested window function of a case, as written: its marker; its delta;
/// and of VALUE_OF, whether its value expression adds the value of the
/// nested function before it to V, and whether it has a default, W.
#[derive(Debug, Clone, Copy)]
struct Written {
    marker: Marker,
    delta: Delta,
    value: Option<(bool, bool)>,
}

/// The input's width: V and W.
const WIDTH: usize = 2;

/// `a` + `b`.
fn plus(a: Expr, b: Expr) -> Expr {
    Expr::Arith(Box::new(a), ArithOp::Add, Box::new(b))
}

/// The column of the `k`th nested function in a context row.
fn nested_column(k: usize) -> Expr {
    Expr::Column(WIDTH.checked_add(k).unwrap())
}

/// The nested function `w`, the `k`th, as bound.
fn bound(w: Written, k: usize) -> Nested {
    let delta = match w.delta {
        Delta::Unwritten => None,
        Delta::Null => Some((false, Expr::Const(Value::Null))),
        Delta::By(d) => Some((d < 0, Expr::Const(exact(Some(d.abs()))))),
    };
    let value = w.value.map(|(inner, defaulted)| {
        let v = if inner { plus(Expr::Column(0), nested_column(k.checked_sub(1).unwrap())) } else { Expr::Column(0) };
        (v, if defaulted { Expr::Column(1) } else { Expr::Const(Value::Null) })
    });
    Nested { marker: w.marker, delta, value }
}

/// SUM or COUNT, DISTINCT if `distinct`, of V plus each of `written`, as
/// bound.
fn call(written: &[Written], (name, distinct): (&str, bool)) -> WindowCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(Some(DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric })).unwrap();
    let arg = (0..written.len()).fold(Expr::Column(0), |e, k| plus(e, nested_column(k)));
    let agg = AggCall { func, arg: Some(arg), distinct, result, filter: None, distribution: None, label: name.into() };
    let frame = Frame { units: Units::Rows, start: Bound::UnboundedPreceding, end: Bound::CurrentRow, exclusion: Exclusion::NoOthers };
    let nested = written.iter().enumerate().map(|(k, &w)| bound(w, k)).collect();
    WindowCall { kind: WindowKind::Aggregate(agg), frame, label: name.into(), nested }
}

/// \[GR3b\] The row number of `marker`, R1 the `r1`th row and R2 the `r2`th
/// of `len`, of `frame`, as GR3b reads it.
fn number(marker: Marker, (r1, r2): (usize, usize), frame: &Ranges, len: usize) -> i128 {
    let rows: Vec<usize> = frame.iter().flat_map(|&(a, b)| a..b).collect();
    let n = match marker {
        Marker::BeginPartition => 0,
        Marker::EndPartition => len.checked_sub(1).unwrap(),
        Marker::CurrentRow => r1,
        Marker::FrameRow => r2,
        Marker::BeginFrame => rows[0],
        Marker::EndFrame => *rows.last().unwrap(),
    };
    i128::try_from(n).unwrap().checked_add(1).unwrap()
}

/// [GR1, GR3, GR4] The argument at R2 of R1's frame, of `rows` (V, W),
/// as the General Rules read it.
fn brute_argument(written: &[Written], rows: &[(Option<i128>, Option<i128>)], (r1, r2, frame): (usize, usize, &Ranges)) -> Option<i128> {
    let mut values: Vec<Option<i128>> = Vec::new();
    for w in written {
        let n = number(w.marker, (r1, r2), frame, rows.len());
        let found = match w.value {
            None => Some(n),
            Some((inner, defaulted)) => {
                let m = match w.delta {
                    Delta::Unwritten => Some(n),
                    Delta::Null => None,
                    Delta::By(d) => n.checked_add(d),
                };
                match m.filter(|&m| m >= 1 && m <= i128::try_from(rows.len()).unwrap()) {
                    None => defaulted.then_some(rows[r2].1).flatten(),
                    Some(m) => {
                        let v = rows.get(usize::try_from(m.checked_sub(1)?).ok()?)?.0;
                        if inner { v.zip(*values.last()?).and_then(|(a, b)| a.checked_add(b)) } else { v }
                    }
                }
            }
        };
        values.push(found);
    }
    values.into_iter().try_fold(rows[r2].0?, |sum, v| v.and_then(|v| sum.checked_add(v)))
}

/// SUM or COUNT, DISTINCT if `distinct`, over `frame`, of R1 the `r1`th
/// row, as the General Rules read it, and whether it left out a null.
fn brute(
    written: &[Written],
    rows: &[(Option<i128>, Option<i128>)],
    (r1, frame): (usize, &Ranges),
    (name, distinct): (&str, bool),
) -> (String, bool) {
    let (mut taken, mut nulls) = (Vec::new(), false);
    for r2 in frame.iter().flat_map(|&(a, b)| a..b) {
        match brute_argument(written, rows, (r1, r2, frame)) {
            None => nulls = true,
            Some(a) => {
                if !(distinct && taken.contains(&a)) {
                    taken.push(a);
                }
            }
        }
    }
    let value = match name {
        "COUNT" => Some(i128::try_from(taken.len()).unwrap()),
        _ => (!taken.is_empty()).then(|| taken.iter().sum()),
    };
    (exact(value).to_string(), nulls)
}

/// A space that spills past about four rows if `spills`.
fn space(spills: bool) -> ListSpace {
    if spills {
        let areas = areas(Some(Arc::new(Held { broken: false })), usize::MAX);
        ListSpace { limit: 160, batch: 4096, areas: Some(areas), types: TypeRegistry::default() }
    } else {
        ListSpace::default()
    }
}

/// The partition of `rows` (V, W) as a call of `shape` reads it, its
/// values from slot 1, held in `space`.
fn partition(rows: &[(Option<i128>, Option<i128>)], shape: &Shape, space: ListSpace) -> Partition {
    let mut keys = HeldRows::new(space);
    for &(v, w) in rows {
        let mut values = vec![Value::Null];
        shape.take_from(&vec![exact(v), exact(w)], &mut values).unwrap();
        keys.push(Placed { group: 0, values }).unwrap();
    }
    Partition { keys, len: rows.len(), nulls: (rows.len(), rows.len()) }
}

/// Random nested functions, one to three, the first's value expression V
/// alone.
fn written(state: &mut u64) -> Vec<Written> {
    let n = below(state, 3).checked_add(1).unwrap();
    (0..n)
        .map(|k| {
            let delta = match below(state, 4) {
                0 => Delta::Unwritten,
                1 => Delta::Null,
                _ => Delta::By(i128::try_from(below(state, 7)).unwrap().checked_sub(3).unwrap()),
            };
            let value = (below(state, 3) != 0).then(|| (k > 0 && below(state, 2) == 0, below(state, 2) == 0));
            Written { marker: MARKERS[below(state, MARKERS.len())], delta: if value.is_some() { delta } else { Delta::Unwritten }, value }
        })
        .collect()
}

/// Random rows (V, W), some null.
fn rows(state: &mut u64) -> Vec<(Option<i128>, Option<i128>)> {
    let value = |s: &mut u64| (below(s, 5) != 0).then(|| i128::try_from(below(s, 30)).unwrap());
    (0..below(state, 24)).map(|_| (value(state), value(state))).collect()
}

/// Random ranges of `len` places: up to three, apart and in order.
fn ranges(state: &mut u64, len: usize) -> Ranges {
    let mut points: Vec<usize> = (0_u32..6).map(|_| below(state, len.checked_add(1).unwrap())).collect();
    points.sort_unstable();
    let range = |a: usize, b: usize| if a < b { (a, b) } else { EMPTY };
    [range(points[0], points[1]), range(points[2], points[3]), range(points[4], points[5])]
}

/// A new database, its operators' work area, and a runner of no
/// transaction.
fn database() -> (Work, Runner) {
    let db = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    (Work::new(&db), Runner::new(&db, crate::txn::NO_TRANSACTION))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR1 to GR4, NOTE 170]
/// a set function of nested window functions over a frame that moves is
/// its value over the frame's rows of the argument the General Rules
/// give, each marker's row its row number, VALUE_OF the value at the
/// marker moved or the default, of a null offset too, inner functions in
/// outer ones' value expressions; whether computed once for each row or
/// for each row of each frame, the frame of the row before reused; over
/// frames of up to three ranges, some empty; of SUM and COUNT; of
/// partitions in memory and spilled; 3,000 fixed cases.
#[test]
fn nested_functions_are_their_general_rules_6_11() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let (work, runner) = database();
    let moving = Bounds::new(Edge::Row, Edge::Last, Exclusion::NoOthers);
    for case in 0_u32..3000 {
        let (written, rows) = (written(&mut state), rows(&mut state));
        let name = if below(&mut state, 2) == 0 { ("SUM", false) } else { ("COUNT", false) };
        let c = call(&written, name);
        let shape = Shape::of(&c, WIDTH, &runner).unwrap().unwrap();
        let spills = case % 2 == 1;
        let mut part = partition(&rows, &shape, space(spills));
        let mut nest = Nest::new((&c, &shape), (&moving, 1), &mut part, (&space(spills), usize::MAX, &work), &runner).unwrap();
        for _ in 0_u32..6 {
            let frame = ranges(&mut state, rows.len());
            let r1 = below(&mut state, rows.len().max(1));
            let (value, nulls) = nest.value(frame, r1, &mut part, &runner).unwrap();
            let want = brute(&written, &rows, (r1, &frame), name);
            assert_eq!((value.to_string(), nulls), want, "case {case}: {written:?} of R1 {r1} over {frame:?} of {rows:?}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR, §9.23 SR8d] a
/// DISTINCT set function of nested window functions over a frame that
/// grows takes each value once of each frame, its values taken apart for
/// each frame where the argument is of the current row; in memory and
/// spilled; 600 fixed cases.
#[test]
fn a_distinct_set_function_of_nested_functions_6_11() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let (work, runner) = database();
    let growing = Bounds::new(Edge::First, Edge::Row, Exclusion::NoOthers);
    for case in 0_u32..600 {
        let (written, rows) = (written(&mut state), rows(&mut state));
        let c = call(&written, ("COUNT", true));
        let shape = Shape::of(&c, WIDTH, &runner).unwrap().unwrap();
        let spills = case % 2 == 1;
        let mut part = partition(&rows, &shape, space(spills));
        let mut nest = Nest::new((&c, &shape), (&growing, 1), &mut part, (&space(spills), usize::MAX, &work), &runner).unwrap();
        for r1 in 0..rows.len() {
            let frame = [(0, r1.checked_add(1).unwrap()), EMPTY, EMPTY];
            let (value, nulls) = nest.value(frame, r1, &mut part, &runner).unwrap();
            let want = brute(&written, &rows, (r1, &frame), ("COUNT", true));
            assert_eq!((value.to_string(), nulls), want, "case {case}: {written:?} of R1 {r1} of {rows:?}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR4d, GR4e i] row M is
/// the marker's row moved by the offset, none if the offset is null, below
/// 1 or past the partition; offsets past any place are none, not an
/// overflow.
#[test]
fn row_m_is_the_marker_moved_6_11() {
    assert_eq!(place(3, Some(0), 5), Some(2));
    assert_eq!(place(3, Some(2), 5), Some(4));
    assert_eq!(place(3, Some(3), 5), None);
    assert_eq!(place(3, Some(-2), 5), Some(0));
    assert_eq!(place(3, Some(-3), 5), None);
    assert_eq!(place(3, None, 5), None);
    assert_eq!(place(1, Some(i128::MAX), 5), None);
    assert_eq!(place(1, Some(i128::MIN), 5), None);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 GR3b] BEGIN_FRAME and
/// END_FRAME are the first and last rows of the frame as exclusion leaves
/// it, of its first and last ranges not empty; an empty frame has none.
#[test]
fn frame_markers_are_of_the_ranges_not_empty_6_11() {
    let frame = [EMPTY, (2, 4), (6, 7)];
    assert_eq!(row_number(Marker::BeginFrame, (0, 0), &frame, 9).unwrap(), 3);
    assert_eq!(row_number(Marker::EndFrame, (0, 0), &frame, 9).unwrap(), 7);
    assert_eq!(row_number(Marker::EndPartition, (0, 0), &frame, 9).unwrap(), 9);
    assert!(row_number(Marker::BeginFrame, (0, 0), &NO_FRAME, 9).is_err());
}

/// Verifies: [DESIGN] a call reads only the input's columns its
/// expressions name, in order, once each; its offsets evaluated once,
/// signed, a null one none.
#[test]
fn a_shape_reads_the_columns_named_6_11() {
    let (_, runner) = database();
    let w = |marker, delta| Written { marker, delta, value: Some((false, true)) };
    let c = call(
        &[w(Marker::FrameRow, Delta::By(-2)), w(Marker::CurrentRow, Delta::Null), w(Marker::EndFrame, Delta::Unwritten)],
        ("SUM", false),
    );
    let shape = Shape::of(&c, WIDTH, &runner).unwrap().unwrap();
    assert_eq!(shape.used, [0, 1]);
    assert_eq!(shape.deltas, [Some(-2), None, Some(0)]);
    assert_eq!(readers(&c.nested), 3, "a reader of each VALUE_OF's row");
    let alone = call(&[w(Marker::FrameRow, Delta::Unwritten)], ("SUM", false));
    assert_eq!(readers(&alone.nested), LIST_SEGMENTS.checked_add(1).unwrap(), "and the list of an argument computed once");
    let plain = WindowCall { nested: Vec::new(), ..c };
    assert!(Shape::of(&plain, WIDTH, &runner).unwrap().is_none());
}
