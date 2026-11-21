//! Unit tests of the probes an index answers: ranges merged, numbers
//! placed among a column's values, `?`s noted, and probes written
//! (RULES.md R-TEST-1).

use super::*;
use crate::value::Spelled;

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn exact(scale: u32) -> DataType {
    DataType::Exact { precision: 9, scale, spelled: Spelled::Numeric }
}

fn approximate() -> DataType {
    DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }
}

/// The probes of the values `equal` then an OR of `each`, over INTEGER columns.
fn any_of(equal: &[Value], each: Vec<Vec<(CompOp, Value)>>) -> Option<Vec<Probe>> {
    probes_of_any(equal, &exact(0), &Alternatives { at: 0, column: 0, exact: true, each, share: 1.0_f64 })
}

/// How many of `probes` find `n`, as `Probe::answers` says.
fn finding(probes: &[Probe], n: i128) -> usize {
    probes.iter().filter(|p| p.answers([&int(n)])).count()
}

/// The one comparison of `each`.
fn only(each: &[(CompOp, Value)]) -> (CompOp, Value) {
    let [one] = each else { panic!("{each:?}") };
    one.clone()
}

/// Whether `c` is `op` and a value equal to `v`.
fn same(c: &(CompOp, Value), op: CompOp, v: &Value) -> bool {
    c.0 == op && compare(&c.1, v) == Some(Ordering::Equal)
}

/// Verifies: [DESIGN] RULES.md D34: an OR of ranges of one column, an IN
/// list among them, is probed a range at a time, the ranges that overlap
/// or touch joined, so each value is found by one probe at most and
/// exactly by those its operands are true of; equal values once; one
/// range unbounded both ways, or a comparison `<>`, is no probe.
#[test]
fn ranges_of_an_or_are_merged() {
    let eq = |n: i128| vec![(CompOp::Eq, int(n))];
    let probes =
        any_of(&[], vec![eq(3), eq(7), eq(3), vec![(CompOp::Lt, int(2))], vec![(CompOp::Ge, int(7)), (CompOp::Le, int(9))]]).unwrap();
    assert_eq!(probes.len(), 3, "{probes:?}");
    for n in -1..12_i128 {
        let expected = usize::from(n < 2 || n == 3 || (7..=9).contains(&n));
        assert_eq!(finding(&probes, n), expected, "{n}");
    }
    // Touching at a value one of them keeps: one range; at one neither keeps: two.
    let touching = any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Ge, int(5)), (CompOp::Lt, int(8))]]).unwrap();
    assert_eq!(touching.len(), 1, "{touching:?}");
    assert_eq!((finding(&touching, 5), finding(&touching, 8)), (1, 0));
    let apart = any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Gt, int(5))]]).unwrap();
    assert_eq!(apart.len(), 2, "{apart:?}");
    assert_eq!((finding(&apart, 4), finding(&apart, 5), finding(&apart, 6)), (1, 0, 1));
    let nested = any_of(&[], vec![vec![(CompOp::Ge, int(1)), (CompOp::Le, int(9))], vec![(CompOp::Gt, int(2)), (CompOp::Lt, int(4))]]);
    assert_eq!(nested.map(|p| p.len()), Some(1));
    let same = any_of(&[], vec![vec![(CompOp::Gt, int(1)), (CompOp::Ge, int(1)), (CompOp::Le, int(4)), (CompOp::Lt, int(4))]]).unwrap();
    assert_eq!((finding(&same, 1), finding(&same, 2), finding(&same, 4)), (0, 1, 0), "the stricter of two bounds");
    assert!(any_of(&[], vec![vec![(CompOp::Lt, int(5))], vec![(CompOp::Gt, int(3))]]).is_none(), "every value");
    assert!(any_of(&[], vec![eq(1), vec![(CompOp::Ne, int(3))]]).is_none(), "<>");
    assert!(any_of(&[], vec![vec![(CompOp::Gt, int(5)), (CompOp::Lt, int(5))]]).is_none(), "no value");
    // Of an index's next column, after the values of its first.
    let prefixed = any_of(&[int(1)], vec![eq(2), vec![(CompOp::Gt, int(5))], vec![(CompOp::Gt, int(-9)), (CompOp::Lt, int(-5))]]).unwrap();
    let [Probe::Range { .. }, Probe::Prefix { then: None, .. }, Probe::Prefix { then: Some(_), .. }] = prefixed.as_slice() else {
        panic!("{prefixed:?}")
    };
}

/// Verifies: [X3.135-1986 §5.11 GR4] a number of the other kind than its
/// column's is placed among the column's values: one of them if it
/// equals one, else between the two on either side, the operator turned
/// so the comparisons keep the same values; `<>` of one between none.
#[test]
fn numbers_are_placed_among_a_columns_values() {
    // 0.1 is between two f64s; 0.5 is one.
    let tenth = Value::Exact(Decimal::new(1, 1));
    let [eq, lt, gt] = [CompOp::Eq, CompOp::Lt, CompOp::Gt].map(|op| coerce(op, &tenth, &approximate()).unwrap());
    let [(CompOp::Gt, Value::Approx(below)), (CompOp::Lt, Value::Approx(above))] = eq.as_slice() else { panic!("{eq:?}") };
    assert_eq!(below.next_up(), *above);
    assert!(same(&only(&lt), CompOp::Le, &Value::Approx(*below)));
    assert!(same(&only(&gt), CompOp::Ge, &Value::Approx(*above)));
    assert!(coerce(CompOp::Ne, &tenth, &approximate()).is_none());
    let negative = coerce(CompOp::Eq, &Value::Exact(Decimal::new(-3, 1)), &approximate()).unwrap();
    assert_eq!(negative.len(), 2, "{negative:?}");
    let half = only(&coerce(CompOp::Eq, &Value::Exact(Decimal::new(5, 1)), &approximate()).unwrap());
    assert!(same(&half, CompOp::Eq, &Value::Approx(0.5_f64)));
    // Of a column of scale 2: 3.25 is one of its values, 3.3 and -0.001 are not.
    let at = only(&coerce(CompOp::Eq, &Value::Approx(3.25_f64), &exact(2)).unwrap());
    assert_eq!((at.0, compare(&at.1, &Value::Exact(Decimal::new(325, 2)))), (CompOp::Eq, Some(Ordering::Equal)));
    let up_to = only(&coerce(CompOp::Le, &Value::Approx(3.3_f64), &exact(2)).unwrap());
    let past = only(&coerce(CompOp::Gt, &Value::Approx(3.3_f64), &exact(2)).unwrap());
    assert_eq!((up_to.0, past.0), (CompOp::Le, CompOp::Ge));
    assert_eq!(compare(&up_to.1, &Value::Exact(Decimal::new(329, 2))), Some(Ordering::Equal), "{up_to:?}");
    assert_eq!(compare(&past.1, &Value::Exact(Decimal::new(330, 2))), Some(Ordering::Equal), "{past:?}");
    let small = only(&coerce(CompOp::Gt, &Value::Approx(-0.001_f64), &exact(2)).unwrap());
    assert_eq!(compare(&small.1, &int(0)), Some(Ordering::Equal), "{small:?}");
    assert!(coerce(CompOp::Eq, &Value::Approx(f64::INFINITY), &exact(2)).is_none());
    assert!(coerce(CompOp::Eq, &Value::Approx(1e300_f64), &exact(2)).is_none(), "past every exact value");
    assert!(coerce(CompOp::Eq, &Value::Char("A".into()), &exact(2)).is_none());
}

/// Verifies: [DESIGN] RULES.md D45: a column compared with a `?` is
/// compared with its value, cast to the slot's type, its slot noted;
/// the value is placed in a probe only as it is, a `?` of a type the
/// column's values are placed otherwise than as it is in none, as
/// another execution's value could fall between two; a constant's as
/// it falls. MC/DC pairs of refusing: a `?` or a constant, its type
/// placed as it is or not, each alone.
#[test]
fn a_slots_value_is_placed_only_as_it_is() {
    let first = |data_type| Slot { index: 0, data_type, at: 1 };
    let cond = Cond::Compare(Expr::Dynamic(first(exact(0))), CompOp::Gt, Expr::Column(2));
    let (compared, _) =
        comparisons(&[cond], Known { stats: None, cost: &crate::physical::CostModel::default() }, Some(&[Value::Char("4".into())]));
    let [c] = compared.as_slice() else { panic!("{} compared", compared.len()) };
    assert_eq!((c.column, c.op, c.slot.clone().map(|s| s.index)), (2, CompOp::Lt, Some(0)), "flipped, the slot noted");
    assert_eq!(compare(&c.value, &int(4)), Some(Ordering::Equal), "the value cast to the slot's type");
    let tenth = |slot: Option<Slot>| Compared {
        collation: None,
        at: 0,
        exact: true,
        column: 0,
        op: CompOp::Eq,
        value: Value::Approx(0.1_f64),
        slot,
        expr: None,
    };
    assert!(tenth(Some(first(approximate()))).placed(&approximate()).is_some(), "a ? of the column's kind");
    assert!(tenth(Some(first(approximate()))).placed(&exact(1)).is_none(), "a ? of another kind");
    assert_eq!(tenth(None).placed(&exact(1)).map(|each| each.len()), Some(2), "a constant between two values");
}

/// Verifies: [DESIGN] RULES.md D613: a column compared with a `?` less a
/// constant is compared with their difference, the slot and the
/// expression noted to be evaluated again as each execution places it; a
/// product of them is not compared.
#[test]
fn a_slot_shifted_by_a_constant_is_compared_with_its_value() {
    let slot = Slot { index: 0, data_type: exact(0), at: 1 };
    let shifted = |op| Expr::Arith(Box::new(Expr::Dynamic(slot.clone())), op, Box::new(Expr::Const(int(20))));
    let cost = crate::physical::CostModel::default();
    let known = || Known { stats: None, cost: &cost };
    let cond = Cond::Compare(Expr::Column(2), CompOp::Ge, shifted(ArithOp::Sub));
    let (compared, _) = comparisons(&[cond], known(), Some(&[int(30)]));
    let [c] = compared.as_slice() else { panic!("{} compared", compared.len()) };
    assert_eq!((c.column, c.op, c.slot.clone().map(|s| s.index)), (2, CompOp::Ge, Some(0)));
    assert_eq!(compare(&c.value, &int(10)), Some(Ordering::Equal), "the difference of the value and the constant");
    assert!(c.expr.is_some(), "the expression kept to place again");
    let product = Cond::Compare(Expr::Column(2), CompOp::Ge, shifted(ArithOp::Mul));
    let (products, _) = comparisons(&[product], known(), Some(&[int(30)]));
    assert!(products.iter().all(|p| p.slot.is_none()), "a product compared with no slot");
}

/// Verifies: [DESIGN] a probe is shown as the comparisons it makes of
/// the index's columns.
#[test]
fn probes_are_written_as_comparisons() {
    let names = ["A".to_string(), "B".to_string()];
    let range = Probe::Range { equal: vec![int(1)], low: Box::new((CompOp::Gt, int(2))), high: Box::new((CompOp::Le, int(4))) };
    assert_eq!(render_probe(&range, &[1, 0], &names), "B = 1 AND A > 2 AND A <= 4");
    assert_eq!(render_probe(&Probe::Prefix { equal: vec![int(1)], then: None }, &[1, 0], &names), "B = 1");
    assert_eq!(render_probe(&Probe::All, &[0], &names), "all keys, in order");
}
