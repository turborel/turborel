//! Unit tests of the tables of a pair with a side (RULES.md D1324): a
//! first table's groups put in the order of their first rows, and a
//! second table that takes its plain set functions of the side's groups.

use std::sync::{Arc, Mutex};

use super::super::{GroupTable, Key, Seed, hash_rows, hash_values};
use super::Sides;
use crate::exec::Runner;
use crate::exec::batch::{Taken, lanes};
use crate::functions::{AggregateRef, Merged, PART_TYPE, Partial, Registry};
use crate::kernels::Portable;
use crate::logical::{AggCall, Expr};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A call of the built-in set function `name` of the column at `at` of
/// type INTEGER, or of rows of none.
fn call(name: &str, at: Option<usize>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(at.map(|_| INTEGER)).unwrap();
    let label = name.to_owned();
    AggCall { func, arg: at.map(Expr::Column), distinct: false, result, filter: None, distribution: None, label }
}

/// The part of the call `of`, as the first of two groupings computes it.
fn part(of: &AggCall) -> AggCall {
    let func = AggregateRef(Arc::new(Partial { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, result: PART_TYPE, ..of.clone() }
}

/// The call merging the parts of `of` that the column at `at` holds.
fn merged(of: &AggCall, at: usize) -> AggCall {
    let func = AggregateRef(Arc::new(Merged { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, arg: Some(Expr::Column(at)), ..of.clone() }
}

/// The exact number of `units`.
fn exact(units: i128) -> Value {
    Value::Exact(Decimal::new(units, 0))
}

/// The state `of` writes after it is given `values`, COUNT(*) a row for
/// each, any other those not null.
fn state(of: &AggCall, values: &[Value]) -> Vec<u8> {
    let mut acc = of.func.0.accumulator(of.result.clone());
    for v in values {
        match (&of.arg, v.is_null()) {
            (None, _) => acc.update_rows(1).unwrap(),
            (Some(_), false) => acc.update(v).unwrap(),
            (Some(_), true) => {}
        }
    }
    let mut out = Vec::new();
    assert!(acc.write_state(&mut out));
    out
}

/// The seed every table of a test is hashed by.
fn seed() -> Seed {
    Seed::fixed(0x51DE)
}

/// The first grouping's groups, a group of the second and a value, with
/// their first rows' numbers, not in their order: three of group 1 and
/// two of group 2.
const KEYS: [(i128, i128, u64); 5] = [(1, 12, 12), (2, 10, 3), (1, 10, 0), (2, 13, 9), (1, 11, 6)];

/// A first table of the groups of `KEYS`, holding no set function.
fn first() -> GroupTable {
    let mut table = GroupTable::new(2, &[], &seed(), usize::MAX);
    for (g, u, number) in KEYS {
        let key = [exact(g), exact(u)];
        table.found_or_made(hash_values(table.seed(), &key), Key::Values(&key), number);
    }
    table
}

/// The sides of the groups `given` names, each with the values its rows
/// held, among four partitions, computing the part of each of `of`: a
/// morsel's side table merged into them, each group into the partition
/// its key's hash names.
fn sides(of: &[AggCall], given: &[(i128, Vec<Value>)]) -> Sides {
    let parts: Vec<AggCall> = of.iter().map(part).collect();
    let sides: Sides = (0..4_usize).map(|_| Mutex::new(None)).collect();
    let mut morsel = GroupTable::new(1, &parts, &seed(), usize::MAX);
    for (g, values) in given {
        let key = [exact(*g)];
        let at = morsel.found_or_made(hash_values(morsel.seed(), &key), Key::Values(&key), 0);
        let states: Vec<Vec<u8>> = of.iter().map(|call| state(call, values)).collect();
        morsel.merge_written(at, (0, &states)).unwrap();
    }
    morsel.merge_into_sides(&sides).unwrap();
    sides
}

/// The second grouping's groups of `first()` by its first column, of the
/// set functions SUM and COUNT(*) merged of the side `given` makes and
/// COUNT of the second column fed the first's groups, finished and shown;
/// or the one line of the error merging the side raises.
fn second(given: &[(i128, Vec<Value>)]) -> Vec<String> {
    let of = [call("SUM", Some(0)), call("COUNT", None)];
    let aggs = [merged(&of[0], 2), merged(&of[1], 3), call("COUNT", Some(1))];
    let (taken, from) = ([Taken::Column(2), Taken::Column(3), Taken::Column(1)], [Some(0), Some(1), None]);
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let parts: Vec<AggCall> = of.iter().map(part).collect();
    let (batch, firsts) = first().into_ordered_batch().unwrap();
    let mut table = GroupTable::merging(1, &aggs, &seed(), &GroupTable::part_kinds(&parts, &from));
    let mut hashes = Vec::new();
    hash_rows(&batch, (&[0], None), (&seed(), &Portable), &mut hashes);
    for (n, (&hash, &number)) in hashes.iter().zip(&firsts).enumerate() {
        let g = table.found_or_made(hash, Key::Row(&batch, &[0], n), number);
        table.take(g, (n, n));
    }
    let fed = lanes(&taken, &batch, None);
    table.feed_unmerged(&batch, (&taken, &fed), &from, (&mut Row::new(), &runner)).unwrap();
    if let Err(e) = table.merge_side((&sides(&of, given), &seed()), &from) {
        return vec![format!("error: {e}")];
    }
    table.finished().unwrap().iter().map(|group| format!("{group:?}")).collect()
}

/// Verifies: [DESIGN] RULES.md D1324, D1123: the groups of a first table
/// that are not in the order of their first rows are given as a batch in
/// that order, each key with its number, and a table in that order as it
/// stands.
#[test]
fn groups_are_given_in_the_order_of_their_first_rows() {
    let (batch, firsts) = first().into_ordered_batch().unwrap();
    assert_eq!(firsts, [0, 3, 6, 9, 12]);
    let keys: Vec<String> =
        (0..batch.rows).map(|r| format!("{:?} {:?}", batch.cell(0, r).to_value(), batch.cell(1, r).to_value())).collect();
    let wanted: Vec<String> = [(1, 10), (2, 10), (1, 11), (2, 13), (1, 12)].map(|(g, u)| format!("{:?} {:?}", exact(g), exact(u))).to_vec();
    assert_eq!(keys, wanted);
    let mut ordered = GroupTable::new(1, &[], &seed(), usize::MAX);
    for (number, g) in [(2_u64, 7_i128), (5, 4)] {
        let key = [exact(g)];
        ordered.found_or_made(hash_values(ordered.seed(), &key), Key::Values(&key), number);
    }
    assert_eq!(ordered.into_ordered_batch().unwrap().1, [2, 5]);
}

/// Verifies: [DESIGN] RULES.md D1324, D1280 [ISO/IEC 9075-2:2023 §10.9
/// GR7b i]: a second table over a first that holds keys alone takes SUM
/// and COUNT(*) of each of its groups from the side group of its key,
/// whichever partition's side holds it, and COUNT of the other column of
/// the first's groups it is fed: a group whose side took a null alone
/// sums to null of its rows counted. A group with no side group raises
/// SQLCODE -910, as no row gives a group of the second without one.
#[test]
fn a_second_table_takes_its_plain_functions_of_the_side() {
    let whole = [(1, vec![exact(5), exact(-7), Value::Null, exact(1)]), (2, vec![exact(40), exact(2), Value::Null, exact(9)])];
    let made = second(&whole);
    let wanted = [(0_u64, 1, Some(-1), 4, 3), (3, 2, Some(51), 4, 2)].map(|(number, g, sum, count, values)| {
        let row: Row = vec![exact(g), sum.map_or(Value::Null, exact), exact(count), exact(values)];
        format!("{:?}", (number, row))
    });
    assert_eq!(made, wanted);
    let nulls = second(&[(1, vec![Value::Null]), (2, vec![exact(3)])]);
    assert!(nulls.first().is_some_and(|row| row.contains("Null")), "{nulls:?}");
    let missing = second(&whole[..1]);
    assert!(missing.first().is_some_and(|line| line.starts_with("error:") && line.contains("no side")), "{missing:?}");
}
