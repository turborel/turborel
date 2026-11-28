//! Unit tests of a partition's groups adopted where they lie (RULES.md
//! D1300): a wide partition holds the groups a table merges of the same
//! morsels, each with the same first row and states, in the same order,
//! whenever it went wide, through a freeze between Mades, and of keys
//! that collide.

use std::sync::Arc;

use super::super::super::hash_values;
use super::*;
use crate::functions::Registry;
use crate::logical::Expr;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The seed every table of these tests hashes by.
fn seed() -> Seed {
    Seed::fixed(0x5EED)
}

/// A call of the built-in set function `name` of a column of type `of`,
/// or of rows (COUNT(*)) if none.
fn call(name: &str, of: Option<DataType>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(of.clone()).unwrap();
    AggCall { func, arg: of.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// COUNT(*), SUM, MIN, MAX and AVG of an INTEGER column, and ANY_VALUE
/// of one, which keeps the value of the first row it is given, so what
/// it holds of a group tells the order the group's rows were merged in.
fn calls() -> Vec<AggCall> {
    let of = |name: &str| call(name, Some(INTEGER));
    vec![call("COUNT", None), of("SUM"), of("MIN"), of("MAX"), of("AVG"), of("ANY_VALUE")]
}

/// The key of the number `n`: an integer, and beside it a text if `text`.
fn key(n: i128, text: bool) -> Vec<Value> {
    let number = Value::Exact(Decimal::new(n, 0));
    if text { vec![number, Value::Char(format!("k{n} ").into())] } else { vec![number] }
}

/// A morsel's table of one row a number of `numbers`, the `r`th row
/// numbered `first + r`, keyed by its number, its integer the number and
/// the value ANY_VALUE is given the row's own number.
fn morsel(numbers: &[i128], first: u64, text: bool) -> GroupTable {
    let aggs = calls();
    let mut table = GroupTable::with_room(key(0, text).len(), &aggs, &seed(), 0);
    for (r, &n) in (first..).zip(numbers) {
        let states: Vec<Vec<u8>> = aggs
            .iter()
            .enumerate()
            .map(|(at, agg)| {
                let mut acc = agg.func.0.accumulator(agg.result.clone());
                let value = match at {
                    0 => Value::Null,
                    1..=4 => Value::Exact(Decimal::new(n, 0)),
                    _ => Value::Exact(Decimal::new(i128::from(r), 0)),
                };
                acc.update(&value).unwrap();
                let mut state = Vec::new();
                assert!(acc.write_state(&mut state));
                state
            })
            .collect();
        let values = key(n, text);
        let g = table.found_or_made(hash_values(table.seed(), &values), Key::Values(&values), r);
        table.merge_written(g, (r, &states)).unwrap();
    }
    table
}

/// The groups of `table`, each its first row's number and its row, in
/// the order the table holds them.
fn groups(table: &GroupTable) -> Vec<String> {
    table.finished().unwrap().iter().map(|group| format!("{group:?}")).collect()
}

/// Four morsels' groups split among `parts` partitions: keys that come
/// once, keys a later morsel holds again, and keys a morsel holds twice.
fn mades(text: bool, parts: usize) -> Vec<Arc<Made>> {
    let morsels: [Vec<i128>; 4] = [
        (0..900).collect(),
        (600..1500).rev().collect(),
        (0..300).chain(0..300).chain(1400..1700).collect(),
        (850..950).chain(2000..2400).collect(),
    ];
    let numbered = (0_u64..).step_by(10_000).zip(&morsels);
    numbered.map(|(first, numbers)| Arc::new(morsel(numbers, first, text).made(parts, &[]).unwrap())).collect()
}

/// Partition `part` of `handed` merged: the first `as_table` Mades into
/// a table, which then goes wide, the others into the wide partition,
/// frozen and taken up again before the last if `thaw`; gathered.
fn gathered(handed: &[Arc<Made>], part: usize, (as_table, thaw): (usize, bool)) -> GroupTable {
    let width = handed[0].keys.width();
    let mut first = GroupTable::new(width, &calls(), &seed(), usize::MAX);
    for made in handed.iter().take(as_table) {
        first.absorb(made, part).unwrap();
    }
    let mut wide = Wide::of(first).unwrap();
    for (at, made) in handed.iter().enumerate().skip(as_table) {
        if thaw && at == 3 {
            wide = Wide::thawed(wide.frozen().unwrap(), &calls(), &seed()).unwrap();
        }
        wide.absorb(made, made.part_places(part)).unwrap();
    }
    wide.into_table().unwrap()
}

/// Verifies: [DESIGN] (RULES.md D1300) a partition that goes wide before
/// its first Made, after its first or after its second, and is frozen
/// and taken up again between two Mades or not, gathers into a table that
/// holds what a table merged of the same Mades holds: every group once,
/// in the order of the first rows, with that first row's number, its
/// count, sum, extremes and mean, and the value of its first row, which
/// a merge in any other order would not give, though some groups took
/// one row and stayed where they lay, some took rows of several Mades
/// and some two rows of one. Of keys of integers and of texts beside
/// them.
#[test]
fn a_wide_partition_holds_the_groups_a_table_holds_in_its_order() {
    for (text, parts) in [(false, 4_usize), (true, 4), (false, 1)] {
        let handed = mades(text, parts);
        for part in 0..parts {
            let mut table = GroupTable::new(key(0, text).len(), &calls(), &seed(), usize::MAX);
            for made in &handed {
                table.absorb(made, part).unwrap();
            }
            let expected = groups(&table);
            assert!(expected.len().saturating_mul(parts) > 400, "the keys spread");
            for how in [(0_usize, false), (1, false), (2, true), (0, true)] {
                let wide = gathered(&handed, part, how);
                assert_eq!(groups(&wide), expected, "text keys {text}, part {part} of {parts}, wide after and thawed {how:?}");
                assert!(wide.in_order());
            }
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1300) a Made merged into a wide
/// partition tells how many of its groups were new, and a wide partition
/// between waves counts its share of the Mades it adopted among the bytes
/// it holds, more the more it adopted.
#[test]
fn a_wide_partition_counts_its_new_groups_and_the_bytes_it_adopted() {
    let handed = mades(false, 1);
    let mut wide = Wide::of(GroupTable::new(1, &calls(), &seed(), usize::MAX)).unwrap();
    let new: Vec<usize> = handed.iter().map(|made| wide.absorb(made, made.part_places(0)).unwrap()).collect();
    assert_eq!(new, [900, 600, 200, 400], "the keys no Made before held");
    let all = wide.frozen().unwrap().bytes();
    let mut one = Wide::of(GroupTable::new(1, &calls(), &seed(), usize::MAX)).unwrap();
    one.absorb(&handed[0], handed[0].part_places(0)).unwrap();
    let least = one.frozen().unwrap().bytes();
    assert!(least >= handed[0].bytes(), "a Made adopted whole is held whole: {least}");
    assert!(all > least, "{all} bytes of four Mades, {least} of one");
}

/// Verifies: [DESIGN] (RULES.md D1300, D357) a wide partition whose
/// groups' hashes collide past the longest probe places them by keyed
/// hashes, each key read where its group lies, and still lists every key
/// once, the keys merged again all found.
#[test]
fn colliding_groups_turn_a_wide_partition_keyed() {
    let aggs = calls();
    let mut colliding = GroupTable::new(1, &aggs, &seed(), usize::MAX);
    for n in 0_u64..400 {
        colliding.make(42, Key::Values(&key(i128::from(n), false)), n);
    }
    let made = Arc::new(colliding.frozen().unwrap());
    let places: Vec<u32> = (0..400).collect();
    let mut wide = Wide::of(GroupTable::new(1, &aggs, &seed(), usize::MAX)).unwrap();
    assert_eq!(wide.absorb(&made, &places).unwrap(), 400);
    assert!(wide.is_keyed());
    assert_eq!(wide.absorb(&made, &places).unwrap(), 0);
    assert_eq!(wide.len(), 400);
    let mut table = GroupTable::new(1, &aggs, &seed(), usize::MAX);
    table.absorb_whole(&made).unwrap();
    table.absorb_whole(&made).unwrap();
    assert_eq!(groups(&wide.into_table().unwrap()), groups(&table));
}
