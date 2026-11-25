//! Unit tests of a morsel's groups merged a column at a time (RULES.md
//! D1110, D1111): each partition's groups found or made, their states
//! merged, as merging them group by group gives; a table made with room;
//! and the groups a condition keeps finished alone.

use super::super::hash_values;
use super::*;
use crate::functions::Registry;
use crate::logical::Expr;
use crate::storage::Row;
use crate::value::codec::put_value;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The exact number `n`.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A call of the built-in set function `name` of an INTEGER column, or
/// of rows (COUNT(*)) if not `of_column`.
fn call(name: &str, of_column: bool) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let arg = of_column.then_some(INTEGER);
    let result = func.0.result_type(arg.clone()).unwrap();
    AggCall { func, arg: arg.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// COUNT(*), SUM, MIN and MAX of an INTEGER column.
fn calls() -> Vec<AggCall> {
    vec![call("COUNT", false), call("SUM", true), call("MIN", true), call("MAX", true)]
}

/// A table with room for `room` groups, of the keys `keys`, each a
/// group of first row `first` on, that took `count` rows of the value its
/// key's first column holds, or of 7 of a key that is no number.
fn morsel(keys: &[Vec<Value>], (first, count, room): (u64, i128, usize)) -> GroupTable {
    let width = keys.first().map_or(0, Vec::len);
    let mut table = GroupTable::with_room(width, &calls(), &Seed::fixed(0x5EED), room);
    for (n, key) in (first..).zip(keys) {
        let g = table.found_or_made(hash_values(table.seed(), key), Key::Values(key), n);
        let units = if let Some(Value::Exact(d)) = key.first() { d.units } else { 7 };
        let (rows, mut total, mut value) = (count.to_le_bytes().to_vec(), Vec::new(), Vec::new());
        put_value(&mut total, &exact(units.saturating_mul(count)));
        put_value(&mut value, &exact(units));
        table.merge_written(g, (n, &[rows, total, value.clone(), value])).unwrap();
    }
    table
}

/// The groups of `table`, finished, in the order of their keys' text.
fn sorted(table: &GroupTable) -> Vec<String> {
    let mut rows: Vec<String> = table.finished().unwrap().iter().map(|(first, row)| format!("{first} {row:?}")).collect();
    rows.sort();
    rows
}

/// Verifies: [DESIGN] (RULES.md D1110) the partitions of two morsels'
/// groups, merged a column at a time into a table each, hold every key
/// once, of integers and of texts beside them, with its first row the
/// earlier and its count, sum, least and greatest merged; as merging the
/// morsels whole gives, and whatever room the morsels' tables were made
/// with.
#[test]
fn partitions_merged_by_columns_hold_what_the_morsels_hold() {
    for text in [false, true] {
        let key = |n: i128| if text { vec![exact(n), Value::Char(format!("k{n} ").into())] } else { vec![exact(n)] };
        let (early, late): (Vec<_>, Vec<_>) = ((0..3000).map(key).collect(), (2000..5000).rev().map(key).collect());
        let width = if text { 2 } else { 1 };
        let mut whole = GroupTable::new(width, &calls(), &Seed::fixed(0x5EED), usize::MAX);
        whole.absorb_whole(&morsel(&early, (0, 2, 0)).frozen().unwrap()).unwrap();
        whole.absorb_whole(&morsel(&late, (10_000, 3, 4000)).frozen().unwrap()).unwrap();
        assert_eq!(whole.len(), 5000);
        let (first, second) = (morsel(&early, (0, 2, 3000)).made(8).unwrap(), morsel(&late, (10_000, 3, 0)).made(8).unwrap());
        assert_eq!((0..8).map(|p| first.groups_of(p)).sum::<usize>(), 3000);
        let mut parted = Vec::new();
        for p in 0..8 {
            let mut table = GroupTable::new(width, &calls(), &Seed::fixed(0x5EED), usize::MAX);
            table.reserve(&first, first.groups_of(p).saturating_add(second.groups_of(p)));
            table.absorb(&first, p).unwrap();
            table.absorb(&second, p).unwrap();
            parted.extend(sorted(&table));
        }
        parted.sort();
        assert_eq!(parted, sorted(&whole), "text keys: {text}");
        let shared = parted.iter().find(|row| row.starts_with("2500 ")).unwrap();
        assert!(shared.contains("Decimal { units: 5, scale: 0 }"), "2 rows and 3: {shared}");
        assert!(shared.contains("units: 12500,"), "2500 five times: {shared}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1110, D357) a table whose keys' hashes
/// collide past the longest probe, merged a column at a time, places its
/// groups by keyed hashes and still holds every key once.
#[test]
fn colliding_groups_merged_by_columns_turn_the_table_keyed() {
    let seed = Seed::fixed(1);
    let mut colliding = GroupTable::new(1, &calls(), &seed, usize::MAX);
    for n in 0_u64..400 {
        colliding.make(42, Key::Values(&[exact(i128::from(n))]), n);
    }
    let made = colliding.frozen().unwrap();
    let mut table = GroupTable::new(1, &calls(), &seed, usize::MAX);
    table.absorb_whole(&made).unwrap();
    assert!(table.is_keyed());
    table.absorb_whole(&made).unwrap();
    assert_eq!(table.len(), 400);
}

/// Verifies: [DESIGN] (RULES.md D1111) the groups a condition keeps are
/// finished as every group is, in their order, and the others not given.
#[test]
fn groups_kept_are_finished_alone() {
    let keys: Vec<Vec<Value>> = (0..100).map(|n| vec![exact(n)]).collect();
    let table = morsel(&keys, (5, 1, 0));
    let all = table.finished().unwrap();
    let units = |row: &Row| if let Some(Value::Exact(d)) = row.first() { d.units } else { 0 };
    let odd = |row: &Row| Ok(units(row) % 2 == 1);
    let kept = table.finished_if(odd).unwrap();
    assert_eq!(kept.len(), 50);
    let expected: Vec<String> = all.iter().skip(1).step_by(2).map(|g| format!("{g:?}")).collect();
    assert_eq!(kept.iter().map(|g| format!("{g:?}")).collect::<Vec<_>>(), expected);
    assert!(table.finished_if(|_| crate::error::fail(crate::error::Code::Corrupt, "[DESIGN] refused")).is_err());
}
