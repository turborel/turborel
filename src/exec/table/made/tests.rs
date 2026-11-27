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
        let (first, second) = (morsel(&early, (0, 2, 3000)).made(8, &[]).unwrap(), morsel(&late, (10_000, 3, 0)).made(8, &[]).unwrap());
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

/// Verifies: [DESIGN] (RULES.md D1260) a morsel whose rows are each made
/// a group of their own, none looked for, a key held three times among
/// them, gives its partitions' tables the groups a morsel grouped in a
/// table gives them: every key once, its first row the earliest, its
/// counts and sums merged, of integers and of texts, whole and split
/// among partitions.
#[test]
fn rows_appended_apart_merge_into_the_groups_a_table_makes() {
    for text in [false, true] {
        let keyed = |n: i128| if text { vec![exact(n % 700), Value::Char(format!("k{} ", n % 700).into())] } else { vec![exact(n % 700)] };
        let keys: Vec<Vec<Value>> = (0..2100).map(keyed).collect();
        let (width, seed) = (if text { 2 } else { 1 }, Seed::fixed(0x5EED));
        let (mut apart, mut tabled) = (GroupTable::unplaced(width, &calls(), &seed, 0), GroupTable::with_room(width, &calls(), &seed, 0));
        for (n, key) in (100_u64..).zip(&keys) {
            let hash = hash_values(&seed, key);
            let own = apart.appended(hash, Key::Values(key), n);
            fed(&mut apart, own, (n, key));
            let found = tabled.found_or_made(hash, Key::Values(key), n);
            fed(&mut tabled, found, (n, key));
        }
        assert_eq!((apart.len(), tabled.len()), (2100, 700), "a group a row, and a group a key");
        let (handed, grouped) = (apart.made(8, &[]).unwrap(), tabled.made(8, &[]).unwrap());
        for p in 0..8 {
            let merged = |made: &Made| {
                let mut table = GroupTable::new(width, &calls(), &seed, usize::MAX);
                table.reserve(made, made.groups_of(p));
                table.absorb(made, p).unwrap();
                table.finished().unwrap().iter().map(|(first, row)| format!("{first} {row:?}")).collect::<Vec<_>>()
            };
            assert_eq!(merged(&handed), merged(&grouped), "partition {p}, text keys: {text}");
        }
    }
}

/// A row to merge: its key's hash, its key, and its number.
type Given = (u64, Vec<Value>, u64);

/// Feeds group `g` of `table` the row `n` of `key`: one row counted, its
/// key's number summed and kept as least and greatest.
fn fed(table: &mut GroupTable, g: u32, (n, key): (u64, &Vec<Value>)) {
    let units = if let Some(Value::Exact(d)) = key.first() { d.units } else { 7 };
    let (mut total, mut value) = (Vec::new(), Vec::new());
    put_value(&mut total, &exact(units));
    put_value(&mut value, &exact(units));
    table.merge_written(g, (n, &[1_i128.to_le_bytes().to_vec(), total, value.clone(), value])).unwrap();
}

/// `rows` each a group of its own, none looked for, frozen whole: groups
/// that may share a key.
fn handed(rows: &[Given], seed: &Seed) -> Made {
    let mut table = GroupTable::unplaced(1, &calls(), seed, 0);
    for (hash, key, n) in rows {
        let g = table.appended(*hash, Key::Values(key), *n);
        fed(&mut table, g, (*n, key));
    }
    table.frozen().unwrap()
}

/// Verifies: [DESIGN] (RULES.md D1275, D357) groups merged in one pass
/// are the groups that finding or making each in turn gives, with the
/// same numbers, first rows and states: of groups no two of a key, of
/// groups that share keys among themselves, of groups whose keys the
/// table already holds, and of groups whose hashes collide, so that a
/// probe grows past the longest mid-way and the table places the rest,
/// keys held twice among them, by keyed hashes.
#[test]
fn groups_merged_in_one_pass_are_those_found_or_made_in_turn() {
    let seed = Seed::fixed(0x5EED);
    let given = |key: i128, n: u64, collides: bool| {
        let key = vec![exact(key)];
        (if collides { 42 } else { hash_values(&seed, &key) }, key, n)
    };
    let apart: Vec<Given> = (0..1500).map(|n| given(i128::from(n), n, false)).collect();
    let shared: Vec<Given> = (0..3000).map(|n| given(i128::from(n * 7 % 2500), 5000 + n, false)).collect();
    let mut colliding: Vec<Given> = (0..300).map(|n| given(i128::from(3000 + n % 200), 9000 + n, false)).collect();
    colliding.extend((0..600).map(|n| given(i128::from(4000 + n % 150), 9300 + n, true)));
    colliding.extend((0..300).map(|n| given(i128::from(n * 11 % 5000), 9900 + n, false)));
    let (mut merged, mut in_turn) = (GroupTable::new(1, &calls(), &seed, usize::MAX), GroupTable::new(1, &calls(), &seed, usize::MAX));
    for (rows, keyed) in [(&apart, false), (&shared, false), (&colliding, true)] {
        merged.absorb_whole(&handed(rows, &seed)).unwrap();
        for (hash, key, n) in rows {
            let g = in_turn.found_or_made(*hash, Key::Values(key), *n);
            fed(&mut in_turn, g, (*n, key));
        }
        assert_eq!((merged.is_keyed(), in_turn.is_keyed()), (keyed, keyed));
        let shown =
            |table: &GroupTable| table.finished().unwrap().iter().map(|(first, row)| format!("{first} {row:?}")).collect::<Vec<_>>();
        assert_eq!(shown(&merged), shown(&in_turn));
    }
    let keys: std::collections::BTreeSet<String> =
        [&apart, &shared, &colliding].iter().flat_map(|rows| rows.iter()).map(|(_, key, _)| format!("{key:?}")).collect();
    assert_eq!(merged.len(), keys.len(), "every key once");
}

/// Verifies: [DESIGN] (RULES.md D1113) groups split by the hashes of
/// their first lane alone put every group of a value of it in one
/// partition, in each morsel alike, of integers and of texts, and the
/// partitions, merged, still hold every key once with its states merged.
#[test]
fn groups_split_by_a_lane_put_each_of_its_values_in_one_partition() {
    for text in [false, true] {
        let first = |n: i128| if text { Value::Char(format!("k{} ", n % 50).into()) } else { exact(n % 50) };
        let key = |n: i128| vec![first(n), exact(n)];
        let (early, late): (Vec<_>, Vec<_>) = ((0..1000).map(key).collect(), (500..1500).rev().map(key).collect());
        let made = [morsel(&early, (0, 1, 0)).made(8, &[0]).unwrap(), morsel(&late, (10_000, 1, 0)).made(8, &[0]).unwrap()];
        let mut owner: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut groups = 0;
        for p in 0..8 {
            let mut table = GroupTable::new(2, &calls(), &Seed::fixed(0x5EED), usize::MAX);
            for m in &made {
                table.absorb(m, p).unwrap();
            }
            for (_, row) in table.finished().unwrap() {
                let held = owner.entry(format!("{:?}", row.first())).or_insert(p);
                assert_eq!(*held, p, "a value of the lane in two partitions, text keys: {text}");
            }
            groups += table.len();
        }
        assert_eq!(groups, 1500, "text keys: {text}");
        assert!(owner.values().collect::<std::collections::HashSet<_>>().len() > 1, "the values spread");
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
