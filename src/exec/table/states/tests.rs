//! Unit tests of the columns of states (RULES.md D356): each built-in set
//! function kept in a column gives what its accumulator gives of the same
//! values in the same order, its result, its error and its written state.

use std::sync::Arc;

use super::*;
use crate::functions::{AggregateFunction, Registry};
use crate::storage::{Column, Floats, Ints, Mask};
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Spelled, Value};

// Exact totals held in 64 bits and widened (RULES.md D1308).
mod widened;

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The type of a DOUBLE PRECISION column.
const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::Float };

/// A call of the built-in set function `name` of an argument of type
/// `arg`, or of rows (COUNT(*)) of none.
fn call(name: &str, arg: Option<DataType>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(arg.clone()).unwrap();
    AggCall {
        func,
        arg: arg.map(|_| crate::logical::Expr::Column(0)),
        distinct: false,
        result,
        filter: None,
        distribution: None,
        label: name.to_owned(),
    }
}

/// A batch of a column of integers, nulls among them, and one of numbers
/// whose sum depends on the order they are added in.
fn batch() -> Batch {
    let nulls = Mask::from_bools(&[false, false, true, false, false, false]);
    Batch {
        columns: vec![
            Column::Ints(Ints::new(vec![5, -7, 0, 12, i64::MAX, 3], 2, Some(nulls))),
            Column::Floats(Floats::new(vec![1e16, 1.0, -1e16, 1.0, 0.5, 1.0], None)),
        ],
        rows: 6,
    }
}

/// The groups of the batch's rows: rows 0, 2, 3 and 5 of group 0, rows 1
/// and 4 of group 1.
const GROUPS: [u32; 6] = [0, 1, 0, 0, 1, 0];

/// The rows of `batch()` taken, each its group's.
fn pending() -> Pending {
    Pending { given: (0..6).collect(), read: (0..6).collect(), groups: GROUPS.to_vec() }
}

/// An accumulator of `agg` fed group `g`'s rows of `batch()` in order, as
/// the row grouping feeds it, the column at `c`.
fn fed_alone(agg: &AggCall, (c, g): (Option<usize>, u32)) -> Box<dyn crate::functions::Accumulator> {
    let mut acc = agg.func.0.accumulator(agg.result.clone());
    let rows: Vec<u32> = (0_u32..).zip(GROUPS).filter(|&(_, of)| of == g).map(|(r, _)| r).collect();
    match c.and_then(|c| batch().columns.into_iter().nth(c)) {
        Some(Column::Ints(ints)) => acc.update_ints(&ints, Some(&rows), &crate::kernels::Portable).unwrap(),
        Some(Column::Floats(floats)) => acc.update_floats(&floats, Some(&rows)).unwrap(),
        Some(_) => panic!("not a column of the batch"),
        None => acc.update_rows(rows.len()).unwrap(),
    }
    acc
}

/// Verifies: [DESIGN] (RULES.md D356) COUNT(*), COUNT, SUM, AVG, MIN and
/// MAX of integers of a scale, nulls among them, and SUM and AVG of
/// numbers whose sum depends on their order, kept in columns, each give
/// their accumulator's result and written state of each group's rows in
/// order; and merged with an accumulator's state, its merged result.
#[test]
fn columns_of_states_are_their_accumulators() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let cases = [
        (call("COUNT", None), None),
        (call("COUNT", Some(INTEGER)), Some(0)),
        (call("SUM", Some(INTEGER)), Some(0)),
        (call("AVG", Some(INTEGER)), Some(0)),
        (call("MIN", Some(INTEGER)), Some(0)),
        (call("MAX", Some(INTEGER)), Some(0)),
        (call("SUM", Some(DOUBLE)), Some(1)),
        (call("AVG", Some(DOUBLE)), Some(1)),
    ];
    for (agg, c) in cases {
        let mut states = States::of(&agg);
        assert!(matches!(states, States::Typed(_)), "{} kept in a column", agg.label);
        (0..2_u32).for_each(|_| states.grow());
        let take = c.map_or(Taken::Count, Taken::Column);
        states.feed((&pending(), &agg), (&batch(), &take, &None), (&mut Vec::new(), &runner)).unwrap();
        assert!(matches!(states, States::Typed(_)), "{} still a column", agg.label);
        for g in 0..2_u32 {
            let alone = fed_alone(&agg, (c, g));
            let at = usize::try_from(g).unwrap();
            assert_eq!(format!("{:?}", states.finish(at)), format!("{:?}", alone.finish()), "{} of group {g}", agg.label);
            let (mut written, mut expected) = (Vec::new(), Vec::new());
            assert_eq!(states.write(at, &mut written), alone.write_state(&mut expected));
            assert_eq!(written, expected, "{} written", agg.label);
            if !expected.is_empty() {
                let mut merged = fed_alone(&agg, (c, g));
                merged.merge(&expected).unwrap();
                states.merge_written((at, &agg), &expected).unwrap();
                assert_eq!(format!("{:?}", states.finish(at)), format!("{:?}", merged.finish()), "{} merged", agg.label);
            }
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D356) a sum kept in a column that passes
/// 128 bits raises SQLCODE -900 as its accumulator does; and a column fed
/// values it does not hold, an integer of another scale, turns into
/// accumulators that give the same result.
#[test]
fn columns_raise_and_widen_as_accumulators() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let sum = call("SUM", Some(INTEGER));
    let mut states = States::of(&sum);
    states.grow();
    let mut huge = Vec::new();
    crate::value::codec::put_value(&mut huge, &Value::Exact(Decimal::new(i128::MAX, 2)));
    states.merge_written((0, &sum), &huge).unwrap();
    let mut acc = sum.func.0.accumulator(sum.result.clone());
    acc.merge(&huge).unwrap();
    let mut one = Vec::new();
    crate::value::codec::put_value(&mut one, &Value::Exact(Decimal::new(1, 2)));
    let (ours, theirs) = (states.merge_written((0, &sum), &one), acc.merge(&one));
    assert_eq!(ours.err().map(|e| e.code), theirs.err().map(|e| e.code));
    let mut widened = States::of(&sum);
    (0..2_u32).for_each(|_| widened.grow());
    widened.feed((&pending(), &sum), (&batch(), &Taken::Column(0), &None), (&mut Vec::new(), &runner)).unwrap();
    let other = Batch { columns: vec![Column::Ints(Ints::new(vec![1; 6], 0, None))], rows: 6 };
    widened.feed((&pending(), &sum), (&other, &Taken::Column(0), &None), (&mut Vec::new(), &runner)).unwrap();
    assert!(matches!(widened, States::Boxed(_)));
    let mut alone = fed_alone(&sum, (Some(0), 1));
    alone.update_ints(&Ints::new(vec![1; 2], 0, None), None, &crate::kernels::Portable).unwrap();
    assert_eq!(format!("{:?}", widened.finish(1)), format!("{:?}", alone.finish()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.7 GR1e]; [DESIGN] (RULES.md D1201)
/// the value a group has of a column its grouping columns determine,
/// kept in a column of values read in place, is its accumulator's of a
/// row of each value: the first row's, a null kept as a null, of
/// integers and of character strings; written and merged as the
/// accumulator's state, a group that has a value keeping it; gathered in
/// another order; and, given what is no column read in place, turned
/// into accumulators that give the same.
#[test]
fn a_column_of_values_kept_is_its_accumulator() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let texts = crate::storage::Texts::new(
        "abbcccdde".to_owned(),
        vec![1, 3, 6, 6, 8, 9],
        Some(Mask::from_bools(&[false, true, false, false, false, false])),
    );
    let mut both = batch();
    both.columns.push(Column::Texts(texts));
    for (c, t) in [(0, INTEGER), (2, DataType::varying(10))] {
        let agg = crate::binder::group_value(c, &t, "V".to_owned());
        let mut states = States::of(&agg);
        (0..3_u32).for_each(|_| states.grow());
        states.feed((&pending(), &agg), (&both, &Taken::Kept(c), &None), (&mut Vec::new(), &runner)).unwrap();
        assert!(matches!(states, States::Carried(_)), "kept in a column");
        let row = |r: usize| Value::Row(vec![both.cell(c, r).to_value()].into());
        let (mut written, mut expected) = (Vec::new(), Vec::new());
        for (g, first) in [(0_usize, Some(0_usize)), (1, Some(1)), (2, None)] {
            let mut alone = agg.func.0.accumulator(agg.result.clone());
            if let Some(r) = first {
                alone.update(&row(r)).unwrap();
            }
            assert_eq!(format!("{:?}", states.finish(g)), format!("{:?}", alone.finish()), "group {g}");
            written.clear();
            expected.clear();
            assert_eq!(states.write(g, &mut written), alone.write_state(&mut expected));
            assert_eq!(written, expected, "group {g} written");
        }
        // Group 2 takes group 0's state; group 1, which has a null, keeps it.
        written.clear();
        states.write(0, &mut written);
        states.merge_written((2, &agg), &written).unwrap();
        states.merge_written((1, &agg), &written).unwrap();
        assert_eq!(format!("{:?}", states.finish(2)), format!("{:?}", states.finish(0)));
        assert!(states.finish(1).unwrap().is_null() == (c == 2), "a null kept is not replaced");
        let sent = states.sent().unwrap().gathered(&[2, 1]);
        let mut thawed = States::thawed(sent, &agg, 2).unwrap();
        assert_eq!(format!("{:?}", thawed.finish(0)), format!("{:?}", Ok::<_, crate::error::SqlError>(both.cell(c, 0).to_value())));
        thawed.feed((&Pending::default(), &agg), (&both, &Taken::Count, &None), (&mut Vec::new(), &runner)).unwrap();
        assert!(matches!(thawed, States::Boxed(_)), "accumulators, of what is no column read in place");
        assert_eq!(format!("{:?}", thawed.finish(0)), format!("{:?}", Ok::<_, crate::error::SqlError>(both.cell(c, 0).to_value())));
    }
}

/// The part of the call `of` (RULES.md D341), as the first of two
/// groupings computes it.
fn part(of: &AggCall) -> AggCall {
    let func = crate::functions::AggregateRef(Arc::new(crate::functions::Partial { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, result: crate::functions::PART_TYPE, label: format!("PARTIAL {}", of.label), ..of.clone() }
}

/// Verifies: [DESIGN] (RULES.md D1186, D341) a part of COUNT(*), COUNT,
/// SUM, AVG, MIN and MAX of integers, and of SUM and AVG of approximate
/// numbers, is kept in its function's column, and each group gives the
/// bytes the part's accumulator gives of the same rows in order: its state
/// after its tag, or its result where it writes none; its written state
/// is the function's, and merged it gives the accumulator's merged part;
/// the function's merge of the parts gives the function's result; sent to
/// another thread and gathered in another order it is a part's column
/// still.
#[test]
fn a_part_kept_in_a_column_is_its_accumulators_part() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let shown = |v: Result<Value>| format!("{v:?}");
    let cases =
        [("COUNT", None, None), ("COUNT", Some(INTEGER), Some(0)), ("SUM", Some(INTEGER), Some(0)), ("AVG", Some(INTEGER), Some(0))];
    let more =
        [("MIN", Some(INTEGER), Some(0)), ("MAX", Some(INTEGER), Some(0)), ("SUM", Some(DOUBLE), Some(1)), ("AVG", Some(DOUBLE), Some(1))];
    for (name, arg, c) in cases.into_iter().chain(more) {
        let whole = call(name, arg);
        let agg = part(&whole);
        let mut states = States::of(&agg);
        (0..2_u32).for_each(|_| states.grow());
        let take = c.map_or(Taken::Count, Taken::Column);
        states.feed((&pending(), &agg), (&batch(), &take, &None), (&mut Vec::new(), &runner)).unwrap();
        assert!(matches!(states, States::Part(_)), "PARTIAL {name} kept in a column");
        let mut merged =
            crate::functions::Merged { func: whole.func.clone(), result: whole.result.clone() }.accumulator(whole.result.clone());
        for g in 0..2_usize {
            let alone = fed_alone(&agg, (c, u32::try_from(g).unwrap()));
            assert_eq!(shown(states.finish(g)), shown(alone.finish()), "PARTIAL {name} of group {g}");
            merged.update(&states.finish(g).unwrap()).unwrap();
            let (mut written, mut expected) = (Vec::new(), Vec::new());
            assert_eq!(states.write(g, &mut written), alone.write_state(&mut expected));
            assert_eq!(written, expected, "PARTIAL {name} written");
        }
        let mut all = fed_alone(&whole, (c, 0));
        let rest = fed_alone(&whole, (c, 1));
        let mut state = Vec::new();
        if rest.write_state(&mut state) {
            all.merge(&state).unwrap();
            assert_eq!(shown(merged.finish()), shown(all.finish()), "MERGED {name}");
        }
        let before = [shown(states.finish(1)), shown(states.finish(0))];
        let thawed = States::thawed(states.sent().unwrap().gathered(&[1, 0]), &agg, 2).unwrap();
        assert!(matches!(thawed, States::Part(_)), "PARTIAL {name} sent");
        assert_eq!([shown(thawed.finish(0)), shown(thawed.finish(1))], before, "PARTIAL {name} gathered");
    }
}

/// Verifies: [DESIGN] (RULES.md D1186) a part's column fed integers of
/// another scale turns into the part's accumulators, each holding the
/// function's as the column held it, which give the parts accumulators
/// fed the same rows give.
#[test]
fn a_parts_column_widens_into_the_parts_accumulators() {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let shown = |v: Result<Value>| format!("{v:?}");
    let agg = part(&call("SUM", Some(INTEGER)));
    let mut widened = States::of(&agg);
    (0..2_u32).for_each(|_| widened.grow());
    widened.feed((&pending(), &agg), (&batch(), &Taken::Column(0), &None), (&mut Vec::new(), &runner)).unwrap();
    let other = Batch { columns: vec![Column::Ints(Ints::new(vec![1; 6], 0, None))], rows: 6 };
    widened.feed((&pending(), &agg), (&other, &Taken::Column(0), &None), (&mut Vec::new(), &runner)).unwrap();
    assert!(matches!(widened, States::Boxed(_)), "accumulators, of integers of another scale");
    let mut alone = fed_alone(&agg, (Some(0), 1));
    alone.update_ints(&Ints::new(vec![1; 2], 0, None), None, &crate::kernels::Portable).unwrap();
    assert_eq!(shown(widened.finish(1)), shown(alone.finish()));
}
