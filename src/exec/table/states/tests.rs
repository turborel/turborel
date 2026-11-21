//! Unit tests of the columns of states (RULES.md D356): each built-in set
//! function kept in a column gives what its accumulator gives of the same
//! values in the same order, its result, its error and its written state.

use std::sync::Arc;

use super::*;
use crate::functions::Registry;
use crate::storage::{Column, Floats, Ints, Mask};
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Spelled, Value};

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
