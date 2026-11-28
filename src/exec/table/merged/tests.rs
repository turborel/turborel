//! Unit tests of a second grouping that merges the first's columns of
//! states (RULES.md D1280): its groups are those a second fed the parts
//! as values makes, and those the functions' own accumulators make of the
//! same states, and a part that is no such column is given as its value.

use std::sync::Arc;

use super::super::{GroupTable, Key, Seed, hash_values};
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Taken, lanes};
use crate::functions::{Accumulator, AggregateFunction, AggregateRef, Merged, PART_TYPE, Partial, Registry};
use crate::logical::{AggCall, Expr};
use crate::storage::{Column, Row};
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The type of an INTEGER column.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The type of a DOUBLE PRECISION column.
const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::Float };

/// TOTAL, a plug-in's sum of whole numbers, whose accumulator writes its
/// state, and which no column of states holds.
struct Total;

/// TOTAL's sum so far.
struct Totalled(i128);

impl AggregateFunction for Total {
    fn name(&self) -> &'static str {
        "TOTAL"
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(INTEGER)
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Totalled(0))
    }
}

impl Accumulator for Totalled {
    fn update(&mut self, v: &Value) -> Result<()> {
        if let Value::Exact(d) = v {
            self.0 = self.0.saturating_add(d.units);
        }
        Ok(())
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(&self.0.to_le_bytes());
        true
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        self.0 = self.0.saturating_add(<[u8; 16]>::try_from(state).map_or(0, i128::from_le_bytes));
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// A call of the set function `func` of a column of type `arg`, or of
/// rows of none.
fn call_of(func: AggregateRef, arg: Option<DataType>) -> AggCall {
    let result = func.0.result_type(arg.clone()).unwrap();
    let label = func.0.name().to_owned();
    AggCall { func, arg: arg.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label }
}

/// A call of the built-in set function `name` of a column of type `arg`.
fn call(name: &str, arg: Option<DataType>) -> AggCall {
    call_of(Registry::with_set_functions().aggregate(name).unwrap(), arg)
}

/// The part of the call `of`, as the first of two groupings computes it.
fn part(of: &AggCall) -> AggCall {
    let func = AggregateRef(Arc::new(Partial { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, result: PART_TYPE, ..of.clone() }
}

/// The call merging the parts of `of` that the column at `at` holds, as
/// the second of two groupings computes it.
fn merged(of: &AggCall, at: usize) -> AggCall {
    let func = AggregateRef(Arc::new(Merged { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, arg: Some(Expr::Column(at)), ..of.clone() }
}

/// The state `of` writes after it is given `values`, those not null as a
/// grouping gives them, COUNT(*) a row for each.
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
    assert!(acc.write_state(&mut out), "{} writes its state", of.label);
    out
}

/// A group, its first row's number and its row, shown, as two groups
/// alike show alike.
fn shown(group: &(u64, Row)) -> String {
    format!("{group:?}")
}

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// The keys of the first grouping's groups, a group of the second and a
/// value, in the order of their first rows: three of group 1 and two of
/// group 2, the second's groups not together.
const KEYS: [(i128, i128); 5] = [(1, 10), (2, 10), (1, 11), (2, 13), (1, 12)];

/// What each group of the first was given, in the order of `KEYS`: the
/// third nothing but a null, so its sum is null and its count of values 0.
fn given() -> Vec<Vec<Value>> {
    vec![
        vec![exact(5, 0), exact(-7, 0)],
        vec![exact(40, 0)],
        vec![Value::Null],
        vec![exact(2, 0), Value::Null, exact(9, 0)],
        vec![exact(1, 0)],
    ]
}

/// A table of the groups of `KEYS` computing the part of each of `of`,
/// each group given `given()`'s values of its place, its first row three
/// times its place.
fn first(of: &[AggCall]) -> GroupTable {
    let parts: Vec<AggCall> = of.iter().map(part).collect();
    let mut table = GroupTable::new(2, &parts, &Seed::fixed(0x5EED), usize::MAX);
    for (n, ((g, u), values)) in KEYS.iter().zip(given()).enumerate() {
        let key = [exact(*g, 0), exact(*u, 0)];
        let number = u64::try_from(n).unwrap().saturating_mul(3);
        let at = table.found_or_made(hash_values(table.seed(), &key), Key::Values(&key), number);
        let states: Vec<Vec<u8>> = of.iter().map(|call| state(call, &values)).collect();
        table.merge_written(at, (number, &states)).unwrap();
    }
    table
}

/// The second grouping's groups of `first`, by its first column,
/// computing the merge of each of `of`: with the columns `merged_by`
/// names taken as they are if `whole`, else every part given as its
/// value; and which columns were taken.
fn second(first: GroupTable, of: &[AggCall], whole: bool) -> (Vec<Option<usize>>, Vec<String>) {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    let runner = Runner::new(&db, NO_TRANSACTION);
    let aggs: Vec<AggCall> = of.iter().enumerate().map(|(a, call)| merged(call, a.saturating_add(2))).collect();
    let taken: Vec<Taken> = (0..aggs.len()).map(|a| Taken::Column(a.saturating_add(2))).collect();
    let from = if whole { first.merged_by(&taken, &aggs) } else { vec![None; aggs.len()] };
    let (batch, firsts, parts) = first.into_parts(&from).unwrap();
    for (a, part) in from.iter().enumerate() {
        let nulls = matches!(batch.columns.get(a.saturating_add(2)), Some(Column::Nulls(_)));
        assert_eq!(nulls, part.is_some(), "a column taken is no value in the batch, {a}");
    }
    let mut table = GroupTable::merging(1, &aggs, &Seed::fixed(0x5EED), &parts);
    for (n, &number) in firsts.iter().enumerate() {
        let key = [batch.cell(0, n).to_value()];
        let g = table.found_or_made(hash_values(table.seed(), &key), Key::Row(&batch, &[0], n), number);
        table.take(g, (n, n));
    }
    let fed = lanes(&taken, &batch, None);
    table.feed_parts(&batch, (&taken, &fed), &parts, (&mut Row::new(), &runner)).unwrap();
    (from, table.finished().unwrap().iter().map(shown).collect())
}

/// The second grouping's groups as the functions' own accumulators make
/// them: each given every value of its groups of the first, in order.
fn alone(of: &[AggCall]) -> Vec<String> {
    let mut groups = Vec::new();
    for key in [1_i128, 2] {
        let places: Vec<usize> = (0..KEYS.len()).filter(|&n| KEYS[n].0 == key).collect();
        let values: Vec<Value> = places.iter().flat_map(|&n| given()[n].clone()).collect();
        let mut row = vec![exact(key, 0)];
        for call in of {
            let mut acc = call.func.0.accumulator(call.result.clone());
            acc.merge(&state(call, &values)).unwrap();
            row.push(acc.finish().unwrap());
        }
        groups.push(shown(&(u64::try_from(places[0]).unwrap().saturating_mul(3), row)));
    }
    groups
}

/// Verifies: [DESIGN] (RULES.md D1280, D341) a second grouping that takes
/// the first's columns of states as they are, of COUNT(*), COUNT, SUM,
/// AVG, MIN and MAX of integers, gives the groups one fed each part as a
/// value gives, and those the functions' accumulators give of the same
/// values: each numbered by its first group of the first, a group whose
/// values were null among them; every such column is taken, and none is
/// made a value.
#[test]
fn columns_of_parts_merged_as_they_are_give_the_groups_their_values_give() {
    let of = [
        call("COUNT", None),
        call("COUNT", Some(INTEGER)),
        call("SUM", Some(INTEGER)),
        call("AVG", Some(INTEGER)),
        call("MIN", Some(INTEGER)),
        call("MAX", Some(INTEGER)),
    ];
    let (from, whole) = second(first(&of), &of, true);
    assert_eq!(from, (0..of.len()).map(Some).collect::<Vec<_>>());
    let (_, values) = second(first(&of), &of, false);
    assert_eq!(whole, values);
    assert_eq!(whole, alone(&of));
    let counted = [exact(1, 0), exact(4, 0), exact(3, 0), exact(-1, 0)];
    assert!(whole[0].starts_with(shown(&(0, counted.to_vec())).trim_end_matches("])")), "{whole:?}");
    assert!(whole[1].starts_with("(3, "), "{whole:?}");
}

/// Verifies: [DESIGN] (RULES.md D1280) a part that is no column of counts
/// or exact totals is given as its value, as before, and its group's
/// result is its accumulator's: a column that turned to accumulators,
/// given a sum of another scale; a plug-in's set function, which no
/// column holds; and a sum of approximate numbers, which writes no state.
/// A column beside them is still taken as it is.
#[test]
fn a_part_no_column_holds_is_given_as_its_value() {
    let of = [call("SUM", Some(INTEGER)), call_of(AggregateRef(Arc::new(Total)), Some(INTEGER)), call("COUNT", None)];
    let mut table = first(&of);
    // A sum of scale 2 where the column holds scale 0 makes accumulators of it.
    let other = [state(&of[0], &[exact(250, 2)]), state(&of[1], &[]), state(&of[2], &[])];
    table.merge_written(0, (0, &other)).unwrap();
    let (from, whole) = second(table, &of, true);
    assert_eq!(from, [None, None, Some(2)]);
    // The first group's sum is its accumulator's of both states.
    let mut sum = of[0].func.0.accumulator(of[0].result.clone());
    sum.merge(&state(&of[0], &[exact(5, 0), exact(-7, 0), exact(1, 0)])).unwrap();
    sum.merge(&other[0]).unwrap();
    let both = format!("{:?}", sum.finish().unwrap());
    let expected = alone(&of);
    assert_eq!(whole[1], expected[1]);
    assert_eq!(whole[0], expected[0].replacen(&format!("{:?}", exact(-1, 0)), &both, 1));

    let float = part(&call("SUM", Some(DOUBLE)));
    let floats = GroupTable::new(2, std::slice::from_ref(&float), &Seed::fixed(0x5EED), usize::MAX);
    let taking = merged(&call("SUM", Some(DOUBLE)), 2);
    assert_eq!(floats.merged_by(&[Taken::Column(2)], std::slice::from_ref(&taking)), [None]);
}

/// Verifies: [DESIGN] (RULES.md D1280) a column of states is taken only
/// by a function that merges its parts, and once: a function that counts
/// the parts as values, one that takes no column of the first's set
/// functions, and a second function naming a column already taken are
/// each given values.
#[test]
fn only_a_function_merging_a_part_takes_its_column() {
    let of = [call("SUM", Some(INTEGER)), call("COUNT", None)];
    let table = first(&of);
    let counting = AggCall { arg: Some(Expr::Column(2)), ..call("COUNT", Some(PART_TYPE)) };
    let aggs = [merged(&of[0], 2), counting, merged(&of[0], 2), merged(&of[1], 0), merged(&of[1], 3)];
    let taken = [Taken::Column(2), Taken::Column(2), Taken::Column(2), Taken::Column(0), Taken::Column(3)];
    assert_eq!(table.merged_by(&taken, &aggs), [Some(0), None, None, None, Some(1)]);
    let filtered = AggCall { filter: Some(crate::logical::Cond::Value(Expr::Column(0))), ..merged(&of[0], 2) };
    assert_eq!(table.merged_by(&[Taken::Column(2)], &[filtered]), [None]);
}
