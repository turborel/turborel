//! Unit tests of a table's groups written to a spill: read back, merged,
//! they finish as the groups written do, of every kind of state, and a
//! group of several waves keeps its first wave's first row (RULES.md
//! D1226).

use super::super::super::hash_values;
use super::*;
use crate::exec::table::{GroupTable, Key, Seed};
use crate::functions::Registry;
use crate::logical::Expr;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A call of the built-in set function `name` of a column of type `arg`,
/// or of rows if none.
fn call(name: &str, arg: Option<DataType>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(arg.clone()).unwrap();
    AggCall { func, arg: arg.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// A set function of each kind of state an accumulator writes: a count,
/// an exact sum and mean, a text's greatest, written by its accumulator,
/// and a value carried. Approximate sums, whose accumulators write none,
/// are tried of their column alone (`typed/written/tests.rs`).
fn calls() -> Vec<AggCall> {
    vec![
        call("COUNT", None),
        call("SUM", Some(INTEGER)),
        call("AVG", Some(INTEGER)),
        call("MAX", Some(DataType::varying(20))),
        call("ANY_VALUE", Some(INTEGER)),
    ]
}

/// The exact number `n`.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The state of `agg` fed `values`, as its accumulator writes it.
fn state(agg: &AggCall, values: &[Value]) -> Vec<u8> {
    let mut acc = agg.func.0.accumulator(agg.result.clone());
    for v in values {
        acc.update(v).unwrap();
    }
    let mut out = Vec::new();
    assert!(acc.write_state(&mut out), "{}", agg.label);
    out
}

/// A table of the groups of keys `n` in `keys`, the `n`th of first row
/// `first + n`, each fed the values of `n` and of `n` again, a third
/// time in `extra`.
fn table(keys: impl Iterator<Item = i128>, (first, extra): (u64, bool)) -> GroupTable {
    let aggs = calls();
    let mut table = GroupTable::new(2, &aggs, &Seed::fixed(0x5EED), usize::MAX);
    for (at, n) in (first..).zip(keys) {
        let key = vec![exact(n), Value::Char(format!("k{n} ").into())];
        let g = table.found_or_made(hash_values(table.seed(), &key), Key::Values(&key), at);
        let fed = |v: Value| if extra { vec![v.clone(), v.clone(), v] } else { vec![v.clone(), v] };
        let values = [exact(1), exact(n), exact(n), Value::Char(format!("t{n}").into()), exact(n.saturating_mul(3))];
        let states: Vec<Vec<u8>> = aggs.iter().zip(values).map(|(agg, v)| state(agg, &fed(v))).collect();
        table.merge_written(g, (at, &states)).unwrap();
    }
    table
}

/// `made` written in pieces of `piece` groups and read back, each piece.
fn round_trip(made: &Made, piece: usize) -> Vec<Made> {
    let places: Vec<u32> = (0..made.len()).map(narrow).collect();
    let mut pieces = Vec::new();
    for at in places.chunks(piece) {
        let mut record = Vec::new();
        assert!(made.put_groups(at, &mut record));
        pieces.push(Made::read(&record, &calls()).unwrap());
    }
    pieces
}

/// The groups of `table` finished, with their first rows, as strings.
fn finished(table: &GroupTable) -> Vec<String> {
    table.finished().unwrap().iter().map(|(first, row)| format!("{first} {row:?}")).collect()
}

/// Verifies: [DESIGN] (RULES.md D1226) groups of a count, an exact sum
/// and mean, a text's greatest written by its accumulator and a value
/// carried, written in pieces and read
/// back, merged into a table, finish as the groups written merged so do,
/// with their first rows and in their order.
#[test]
fn groups_read_back_finish_as_those_written() {
    let made = table(0..1000, (10, false)).frozen().unwrap();
    let aggs = calls();
    let mut written = GroupTable::new(2, &aggs, &Seed::fixed(0x5EED), usize::MAX);
    written.absorb_whole(&made).unwrap();
    let mut read = GroupTable::new(2, &aggs, &Seed::fixed(0x5EED), usize::MAX);
    for piece in round_trip(&made, 300) {
        read.absorb_whole(&piece).unwrap();
    }
    let rows = finished(&read);
    assert_eq!(rows, finished(&written));
    assert!(rows.first().is_some_and(|row| row.starts_with("10 ") && row.contains("\"t0\"")), "{:?}", rows.first());
}

/// Verifies: [DESIGN] (RULES.md D1226) a partition held in the spill as
/// three waves' runs, read back in their order, holds a group of all
/// three once, its first row its first wave's, the earliest, and its
/// groups in the order of their first rows, as merging the waves in
/// memory gives.
#[test]
fn a_group_of_three_waves_keeps_its_first_waves_first_row() {
    let waves = [table(0..400, (0, false)), table(200..600, (1000, true)), table(100..800, (2000, false))];
    let aggs = calls();
    let mut memory = GroupTable::new(2, &aggs, &Seed::fixed(0x5EED), usize::MAX);
    let mut spilled = GroupTable::new(2, &aggs, &Seed::fixed(0x5EED), usize::MAX);
    for wave in waves {
        let made = wave.frozen().unwrap();
        memory.absorb_whole(&made).unwrap();
        for piece in round_trip(&made, 128) {
            spilled.absorb_whole(&piece).unwrap();
        }
    }
    let rows = finished(&spilled);
    assert_eq!(rows, finished(&memory));
    assert_eq!(rows.len(), 800);
    let shared = rows.iter().find(|row| row.contains("\"k300 \"")).unwrap();
    assert!(shared.starts_with("300 "), "its first wave's first row: {shared}");
    let firsts: Vec<u64> = spilled.finished().unwrap().iter().map(|(first, _)| *first).collect();
    assert!(firsts.is_sorted(), "the groups in the order of their first rows");
}
