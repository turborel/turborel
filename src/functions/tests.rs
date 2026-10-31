//! Unit tests of the built-in set functions (`functions.rs`).

use super::extremes::Extreme;
use super::*;

/// Verifies: [X3.135-1986 §5.8 F] MAX and MIN take an argument; asked for
/// the type of an argumentless call, they refuse rather than panic.
#[test]
fn max_and_min_need_an_argument() {
    for max in [true, false] {
        assert_eq!(Extreme { max }.result_type(None).err().map(|e| e.code), Some(Code::SyntaxRule));
    }
}

/// Verifies: [X3.135-1986 §5.8 GR4f] AVG of approximate values is approximate.
#[test]
fn approximate_average() {
    let mut avg = Avg.accumulator(DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float });
    for x in [1.0_f64, 2.0_f64] {
        avg.update(&Value::Approx(x)).unwrap();
    }
    assert!(matches!(avg.finish(), Ok(Value::Approx(a)) if (a - 1.5_f64).abs() < f64::EPSILON));
}

/// The results of `function`'s accumulator of `result` fed `values`, at
/// `rows` or all, a value at a time and a vector at a time.
fn both_ways(function: &dyn AggregateFunction, result: DataType, (values, rows): (&[Value], Option<&[usize]>)) -> (String, String) {
    let mut one = function.accumulator(result);
    let picked: Vec<&Value> = match rows {
        Some(rows) => rows.iter().filter_map(|&r| values.get(r)).collect(),
        None => values.iter().collect(),
    };
    let first = picked.iter().filter(|v| !v.is_null()).try_for_each(|v| one.update(v)).and_then(|()| one.finish());
    let mut many = function.accumulator(result);
    let second = many.update_many(values, rows).and_then(|()| many.finish());
    (format!("{first:?}"), format!("{second:?}"))
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.8 GR1, GR4]: COUNT,
/// SUM, AVG, MAX and MIN fed a vector at a time give what they give a
/// value at a time: nulls left out, of the rows asked or all, exact
/// values of other scales, approximate ones among them, a sum out of
/// range refused alike; COUNT(*) counts rows given at once.
#[test]
fn set_functions_fed_vectors_total_as_values() {
    let exact = |u: i128, s: u32| Value::Exact(Decimal::new(u, s));
    let mixed = vec![exact(15, 1), Value::Null, exact(-3, 0), exact(250, 2), Value::Approx(0.5_f64), exact(7, 0), Value::Null];
    let numbers = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 2, spelled: Spelled::Numeric };
    let approx = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };
    let functions: [(&dyn AggregateFunction, DataType); 5] =
        [(&Count, numbers), (&Sum, approx), (&Avg, approx), (&Extreme { max: true }, numbers), (&Extreme { max: false }, numbers)];
    let exacts: Vec<Value> = mixed.iter().filter(|v| !matches!(v, Value::Approx(_))).cloned().collect();
    for (function, result) in functions {
        for values in [&mixed, &exacts] {
            for rows in [None, Some(&[0_usize, 2, 3, 6][..]), Some(&[][..])] {
                let (one, many) = both_ways(function, result, (values, rows));
                assert_eq!(one, many, "{} of {values:?} at {rows:?}", function.name());
            }
        }
    }
    let huge = vec![exact(i128::from(u64::MAX).checked_mul(1_000_000_000_000_000_000).unwrap(), 0); 3];
    let (one, many) = both_ways(&Sum, numbers, (&huge, None));
    assert!(one.contains("Err") && one == many, "out of range alike: {many}");
    let mut count = Count.accumulator(numbers);
    count.update_rows(4).unwrap();
    count.update(&Value::Null).unwrap();
    assert_eq!(count.finish().unwrap().to_string(), "5");
}

/// An accumulator of a plug-in's, of `update` and `finish` alone.
struct Listed(Vec<String>);

impl Accumulator for Listed {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.0.push(v.to_string());
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Char(self.0.join(",")))
    }
}

/// Verifies: [DESIGN] RULES.md D31, extensibility: an accumulator that
/// gives only `update` is fed a vector, or rows counted, a value at a
/// time, nulls left out, of the rows asked.
#[test]
fn an_accumulator_of_update_alone_is_fed_vectors() {
    let one = Value::Exact(Decimal::new(1, 0));
    let mut listed = Listed(Vec::new());
    listed.update_many(&[one.clone(), Value::Null, Value::Char("b".into())], None).unwrap();
    listed.update_many(&[one, Value::Char("c".into())], Some(&[1])).unwrap();
    listed.update_rows(2).unwrap();
    assert_eq!(listed.finish().unwrap().to_string(), "1,b,c,NULL,NULL");
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.8 GR1, GR4, §5.9 SR5]:
/// COUNT, SUM, AVG, MAX and MIN fed approximate numbers as a vector
/// (`Floats`) give what they give a value at a time: nulls left out, of
/// the rows asked or all, -0 and 0 alike, after a sum begun; a sum out of
/// range refused alike, as it is finished.
#[test]
fn set_functions_fed_floats_total_as_values() {
    let numbers = vec![1.5_f64, 0.0_f64, -2.25_f64, 0.1_f64, -0.0_f64, 7.0_f64, 0.0_f64];
    let nulls = vec![false, true, false, false, false, false, true];
    let floats = Floats::new(numbers, Some(nulls));
    let approx = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };
    let functions: [&dyn AggregateFunction; 5] = [&Count, &Sum, &Avg, &Extreme { max: true }, &Extreme { max: false }];
    for function in functions {
        for rows in [None, Some(&[0_usize, 2, 3, 4][..]), Some(&[1_usize, 6][..])] {
            let mut by_values = function.accumulator(approx);
            by_values.update(&Value::Approx(10.0_f64)).unwrap();
            by_values.update_many(floats.values(), rows).unwrap();
            let mut by_floats = function.accumulator(approx);
            by_floats.update(&Value::Approx(10.0_f64)).unwrap();
            by_floats.update_floats(&floats, rows).unwrap();
            assert_eq!(format!("{:?}", by_values.finish()), format!("{:?}", by_floats.finish()), "{} at {rows:?}", function.name());
        }
    }
    let huge = Floats::new(vec![f64::MAX, f64::MAX], None);
    let mut sum = Sum.accumulator(approx);
    sum.update_floats(&huge, None).unwrap();
    let mut by_values = Sum.accumulator(approx);
    by_values.update_many(huge.values(), None).unwrap();
    let refused = sum.finish().err().map(|e| e.code);
    assert_eq!(refused, by_values.finish().err().map(|e| e.code));
    assert!(refused.is_some(), "out of range");
}

/// The result of `function`'s accumulator of `result` fed `values` whole,
/// and of one fed those before `at` that merged the state of one fed the
/// rest.
fn merged(function: &dyn AggregateFunction, result: DataType, values: &[Value], at: usize) -> (String, String) {
    let (mut all, mut before, mut after) = (function.accumulator(result), function.accumulator(result), function.accumulator(result));
    all.update_many(values, None).unwrap();
    before.update_many(&values[..at], None).unwrap();
    after.update_many(&values[at..], None).unwrap();
    let mut state = Vec::new();
    assert!(after.write_state(&mut state));
    before.merge(&state).unwrap();
    (format!("{:?}", all.finish()), format!("{:?}", before.finish()))
}

/// Verifies: [DESIGN] RULES.md D53, D15 [ISO/IEC 9075:1992 §6.5 GR2b]:
/// COUNT, SUM, AVG, MAX and MIN of exact numbers, and SUM and AVG of
/// intervals, write their states, and one that merges another's gives
/// what one given every value gives, wherever the values are split: a sum
/// on the way past 18 digits is not judged, nor a total past them until
/// it is finished; MAX and MIN of ties keep the value given first, 'a'
/// before 'a  '. SUM and AVG of approximate numbers, MAX and MIN of a
/// plug-in's type, and a plug-in's accumulator by default write none; a
/// state not the function's own is refused, SQLCODE -916.
#[test]
fn set_functions_merge_their_states_as_their_values() {
    use crate::value::datetime::Field;
    use crate::value::interval::{Interval, Qualifier, unit};
    use crate::value::types::{ExtensionType, TypeParams, UNREGISTERED_KIND};
    let exact = |u: i128| Value::Exact(Decimal::new(u, 2_u32));
    let most = crate::value::pow10(18_u32).unwrap() - 1_i128;
    let numbers = [exact(most), exact(1_i128), Value::Null, exact(-1_i128), exact(-250_i128), exact(7_i128)];
    let money = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 2, spelled: Spelled::Numeric };
    let functions: [&dyn AggregateFunction; 5] = [&Count, &Sum, &Avg, &Extreme { max: true }, &Extreme { max: false }];
    for function in functions {
        for at in 0..=numbers.len() {
            let (all, split) = merged(function, money, &numbers, at);
            assert_eq!(all, split, "{} split at {at}", function.name());
        }
    }
    let past = [exact(most), exact(most)];
    assert_eq!(merged(&Sum, money, &past, 1_usize).1, merged(&Sum, money, &past, 0_usize).0, "judged as finished");
    let q = Qualifier::new(Field::Day, Field::Day, 9, 0);
    let days = |n: i128| Value::Interval(Interval { units: n * unit(Field::Day), qualifier: q });
    let intervals = [days(999_999_999_i128), days(1_i128), days(-1_i128)];
    let sums: [&dyn AggregateFunction; 2] = [&Sum, &Avg];
    for function in sums {
        let (all, split) = merged(function, DataType::Interval(q), &intervals, 1_usize);
        assert_eq!(all, split, "{} of intervals", function.name());
    }
    let ties = [Value::Char("a".to_owned()), Value::Char("a  ".to_owned())];
    let text = DataType::Varying { length: 3 };
    assert_eq!(merged(&Extreme { max: true }, text, &ties, 1_usize).1, "Ok(Char(\"a\"))");
    let double = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };
    let mut none = Vec::new();
    assert!(!Sum.accumulator(double).write_state(&mut none) && !Avg.accumulator(double).write_state(&mut none));
    let point = DataType::Extension(ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::default() });
    assert!(!Extreme { max: true }.accumulator(point).write_state(&mut none));
    assert!(!Listed(Vec::new()).write_state(&mut none));
    assert_eq!(none.len(), 0_usize, "nothing written");
    assert_eq!(Listed(Vec::new()).merge(&[]).err().map(|e| e.code), Some(Code::Corrupt));
    for function in functions {
        let refused = function.accumulator(money).merge(&[0xff_u8]).err().map(|e| e.code);
        assert_eq!(refused, Some(Code::Corrupt), "{}", function.name());
    }
}
