//! Unit tests of kept plans: plans made with no table, and the
//! conditions of keeping one (RULES.md R-TEST-1, R-TEST-8).

use std::fmt::Debug;

use super::*;
use crate::catalog::{Catalog, ColumnDesc, IndexDesc, MemoryCatalog, QualifiedName, TableDesc};
use crate::engine::rows::ROW_STORE;
use crate::index::BTREE_METHOD;
use crate::logical::Truth;
use crate::value::collation::{CASE_INSENSITIVE, CollationRef, Pad, builtin};
use crate::value::{Decimal, Spelled};

const INT: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

fn filled_plan(plan: Plan, values: &[Value]) -> Result<Plan> {
    super::filled_plan(plan, values, None)
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn slot(index: usize) -> Slot {
    Slot { index, data_type: INT, at: 3 }
}

fn table() -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: "T".into() }
}

/// Whether `a` and `b` are written the same, of what compares no
/// other way.
fn same<T: Debug>(a: &T, b: &T) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// A state whose catalog has the table `A.T` (K, L INTEGER) and on it
/// the index `I` on (K, L), unique if `unique`, and `J` on K, by a
/// collation.
fn state(unique: bool) -> State {
    let mut catalog = MemoryCatalog::default();
    let column = |n: &str| ColumnDesc {
        name: n.into(),
        data_type: INT,
        not_null: true,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: table(),
        columns: vec![column("K"), column("L")],
        constraints: vec![],
        engine: ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    catalog.create_schema("A", "A", vec![desc]);
    let index = |n: &str, columns: Vec<usize>, collations: Vec<Option<CollationRef>>| IndexDesc {
        name: n.into(),
        table: table(),
        columns,
        method: BTREE_METHOD.into(),
        unique,
        options: Vec::new(),
        collations,
        nulls: false,
        expressions: Vec::new(),
    };
    catalog.add_index(index("I", vec![0, 1], Vec::new()));
    let folded = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).map(|collation| CollationRef { collation, pad: Pad::Space });
    catalog.add_index(index("J", vec![0], vec![folded]));
    State::with_builtins(Box::new(catalog))
}

/// An index scan of `index` by `probe`, with `slots`.
fn scan(index: &str, probe: Probe, slots: Vec<ProbeSlot>) -> Plan {
    Plan::IndexScan {
        table: table(),
        index: index.into(),
        method: BTREE_METHOD.into(),
        probe,
        slots,
        reverse: false,
        columns: vec![0, 1],
        detail: String::new(),
        est: 1.0_f64,
        cost: 1.0_f64,
        covering: false,
        located: false,
        decided: None,
    }
}

/// A probe of the two values of the key of `I`.
fn whole() -> Probe {
    Probe::Prefix { equal: vec![int(1), int(2)], then: None }
}

/// Verifies: [DESIGN] D34, D45: a function lookup's parameter slot
/// converts values and accepts nulls. A missing parameter or a slot
/// meant for another probe kind raises SQLCODE -910. Function lookups
/// cannot use the plan-reuse shortcut reserved for unique-key lookups.
#[test]
fn predicate_slots_accept_only_their_argument_place() {
    let function = crate::functions::Registry::with_set_functions().scalar("UPPER").unwrap();
    let predicate = Box::new(crate::index::Predicate { function, column: 0, argument: Some(0) });
    let probe = Probe::Predicate { predicate, value: Value::Null };
    let s = ProbeSlot::of(Place::PredicateValue, slot(0));
    let mut filled = probe.clone();
    place(&mut filled, &s, &[Value::Char("12".into())], Placing::Cast).unwrap();
    let Probe::Predicate { value, .. } = &filled else { panic!("not a predicate") };
    assert!(same(value, &int(12)));
    place(&mut filled, &s, &[Value::Null], Placing::Cast).unwrap();
    let Probe::Predicate { value: null, .. } = &filled else { panic!("not a predicate") };
    assert!(null.is_null());
    assert_eq!(place(&mut filled, &s, &[], Placing::Cast).unwrap_err().sqlcode(), -910_i32);
    for position in [Place::Key(0), Place::Low, Place::High] {
        let wrong = ProbeSlot::of(position, slot(0));
        assert_eq!(place(&mut filled, &wrong, &[int(1)], Placing::Cast).unwrap_err().sqlcode(), -910_i32);
    }
    let comparisons = [
        Probe::All,
        Probe::Compare { op: CompOp::Eq, value: int(1) },
        whole(),
        Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, int(1))), high: Box::new((CompOp::Le, int(2))) },
    ];
    for mut comparison in comparisons {
        assert_eq!(place(&mut comparison, &s, &[int(1)], Placing::Cast).unwrap_err().sqlcode(), -910_i32);
    }
    assert!(!keepable(&scan("I", probe, Vec::new()), &state(true)));
}

/// Verifies: [DESIGN] RULES.md D45: a plan is kept if it is one probe
/// fixing every column of a unique index, under a filter or a
/// projection or neither. MC/DC pairs, from such a probe kept: the
/// index not unique; the index collated; the probe not of its whole
/// key: each alone forbids it. A scan, a sort over the probe, or an
/// index the catalog lacks is never kept.
#[test]
fn a_probe_of_a_whole_unique_key_alone_is_kept() {
    let st = state(true);
    assert!(keepable(&scan("I", whole(), Vec::new()), &st));
    let filtered = Plan::Filter {
        cond: Cond::Const(Truth::True),
        detail: String::new(),
        est: 0.0_f64,
        input: Box::new(scan("I", whole(), Vec::new())),
        of_read: false,
    };
    let projected = Plan::Project { exprs: vec![Expr::Column(0)], detail: String::new(), input: Box::new(filtered) };
    assert!(keepable(&projected, &st));
    assert!(!keepable(&scan("I", whole(), Vec::new()), &state(false)), "not unique");
    assert!(!keepable(&scan("J", Probe::Compare { op: CompOp::Eq, value: int(1) }, Vec::new()), &st), "collated");
    assert!(!keepable(&scan("I", Probe::Prefix { equal: vec![int(1)], then: None }, Vec::new()), &st), "a prefix of the key");
    assert!(!keepable(&scan("I", Probe::All, Vec::new()), &st), "every key");
    let seq = Plan::SeqScan {
        table: table(),
        columns: vec![0],
        rows: 0,
        cost: 0.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: false,
    };
    assert!(!keepable(&seq, &st));
    let sorted = Plan::Sort { keys: Vec::new(), detail: String::new(), input: Box::new(scan("I", whole(), Vec::new())), most: None };
    assert!(!keepable(&sorted, &st));
    assert!(!keepable(&scan("X", whole(), Vec::new()), &st), "no such index");
}

/// Verifies: [DESIGN] RULES.md D45: a probe fixing a key by `?`s and a
/// constant is made of the equalities of the key's columns, a `?`
/// written on either side, a null standing in for each `?`; none if a
/// column is not held equal, if it is held equal to a `?` of a type
/// the index places otherwise than as it is, or by another comparison.
#[test]
fn a_point_probe_is_made_of_equalities_with_slots_and_constants() {
    let equal = |column: usize, e: Expr| Cond::Compare(Expr::Column(column), CompOp::Eq, e);
    let conds = vec![equal(1, Expr::Dynamic(slot(0))), Cond::Compare(Expr::Dynamic(slot(1)), CompOp::Eq, Expr::Column(0))];
    let both = point_probe(&conds, &[], (&[0, 1], &[INT, INT])).unwrap();
    assert!(same(&both.0, &Probe::Prefix { equal: vec![Value::Null, Value::Null], then: None }), "{both:?}");
    assert_eq!(both.1, vec![ProbeSlot::of(Place::Key(0), slot(1)), ProbeSlot::of(Place::Key(1), slot(0))]);
    assert_eq!(both.2, vec![1, 0]);
    let constant = Compared { collation: None, at: 1, exact: true, column: 0, op: CompOp::Eq, value: int(7), slot: None, expr: None };
    let mixed = point_probe(&conds, std::slice::from_ref(&constant), (&[0, 1], &[INT, INT])).unwrap();
    assert!(same(&mixed.0, &Probe::Prefix { equal: vec![int(7), Value::Null], then: None }), "{mixed:?}");
    assert_eq!((mixed.1, mixed.2), (vec![ProbeSlot::of(Place::Key(1), slot(0))], vec![1, 0]));
    let one = point_probe(&conds, &[], (&[1], &[INT])).unwrap();
    assert!(same(&one.0, &Probe::Compare { op: CompOp::Eq, value: Value::Null }), "{one:?}");
    assert_eq!(one.1, vec![ProbeSlot::of(Place::Key(0), slot(0))]);
    assert!(point_probe(&conds[..1], &[], (&[0, 1], &[INT, INT])).is_none(), "K not held equal");
    let text = DataType::varying(4);
    assert!(point_probe(&conds, &[], (&[0, 1], &[INT, text])).is_none(), "a number in a text column");
    let less = Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Dynamic(slot(0)));
    assert!(point_probe(&[less], &[], (&[0], &[INT])).is_none(), "not an equality");
    let between =
        Compared { collation: None, at: 0, exact: true, column: 0, op: CompOp::Eq, value: Value::Approx(0.5_f64), slot: None, expr: None };
    assert!(point_probe(&[], std::slice::from_ref(&between), (&[0], &[INT])).is_none(), "a number between the column's");
}

/// Verifies: [DESIGN] RULES.md D45: filling a kept plan puts each value,
/// cast to its slot's type, in its place in the probe, and in the
/// filter and projection over it, a range's equal values and bounds
/// and a prefix's comparison among them; a value its slot's type takes
/// none of fails as CAST fails; a `?` with no place in its probe, and a
/// plan of an operator no kept plan holds, a join's, are refused.
#[test]
fn a_kept_plan_is_filled_with_each_executions_values() {
    let slots = vec![ProbeSlot::of(Place::Key(1), slot(0))];
    let standing = Probe::Prefix { equal: vec![int(7), Value::Null], then: None };
    let judged = Cond::Compare(Expr::Column(1), CompOp::Lt, Expr::Dynamic(slot(1)));
    let filtered =
        Plan::Filter { cond: judged, detail: String::new(), est: 0.0_f64, input: Box::new(scan("I", standing, slots)), of_read: false };
    let plan = Plan::Project { exprs: vec![Expr::Dynamic(slot(1))], detail: String::new(), input: Box::new(filtered) };
    let filled = filled_plan(plan, &[Value::Char(" 12 ".into()), int(3)]).unwrap();
    let Plan::Project { exprs, input, .. } = filled else { panic!("{filled:?}") };
    assert!(same(&exprs, &vec![Expr::Const(int(3))]), "{exprs:?}");
    let Plan::Filter { cond, input, .. } = *input else { panic!("{input:?}") };
    assert!(same(&cond, &Cond::Compare(Expr::Column(1), CompOp::Lt, Expr::Const(int(3)))), "{cond:?}");
    let Plan::IndexScan { probe, .. } = *input else { panic!("{input:?}") };
    assert!(same(&probe, &Probe::Prefix { equal: vec![int(7), int(12)], then: None }), "the string cast to INTEGER: {probe:?}");
    let one = scan("I", Probe::Compare { op: CompOp::Eq, value: Value::Null }, vec![ProbeSlot::of(Place::Key(0), slot(0))]);
    let Plan::IndexScan { probe: first, .. } = filled_plan(one, &[int(5)]).unwrap() else { panic!("not a scan") };
    assert!(same(&first, &Probe::Compare { op: CompOp::Eq, value: int(5) }), "{first:?}");
    let failed = filled_plan(scan("I", Probe::All, vec![ProbeSlot::of(Place::Key(0), slot(0))]), &[Value::Char("X".into())]).unwrap_err();
    assert_eq!(failed.code, Code::Data, "CAST fails first");
    let misplaced = filled_plan(scan("I", Probe::All, vec![ProbeSlot::of(Place::Key(0), slot(0))]), &[int(1)]).unwrap_err();
    assert_eq!(misplaced.code, Code::Api);
    let ranged =
        Probe::Range { equal: vec![Value::Null], low: Box::new((CompOp::Ge, Value::Null)), high: Box::new((CompOp::Lt, Value::Null)) };
    let bounds = [Place::Key(0), Place::Low, Place::High].map(|place| ProbeSlot::of(place, slot(0)));
    let Plan::IndexScan { probe: range, .. } = filled_plan(scan("I", ranged, bounds.to_vec()), &[int(4)]).unwrap() else {
        panic!("not a scan")
    };
    let filled_range = Probe::Range { equal: vec![int(4)], low: Box::new((CompOp::Ge, int(4))), high: Box::new((CompOp::Lt, int(4))) };
    assert!(same(&range, &filled_range), "{range:?}");
    let then = Probe::Prefix { equal: vec![int(1)], then: Some((CompOp::Gt, Value::Null)) };
    let Plan::IndexScan { probe: after, .. } =
        filled_plan(scan("I", then, vec![ProbeSlot::of(Place::Key(1), slot(0))]), &[int(6)]).unwrap()
    else {
        panic!("not a scan")
    };
    assert!(same(&after, &Probe::Prefix { equal: vec![int(1)], then: Some((CompOp::Gt, int(6))) }), "{after:?}");
    let low_of_prefix = scan("I", Probe::Prefix { equal: vec![int(1)], then: None }, vec![ProbeSlot::of(Place::Low, slot(0))]);
    assert_eq!(filled_plan(low_of_prefix, &[int(1)]).unwrap_err().code, Code::Api, "a bound of no range");
    let sorted = Plan::Sort { keys: Vec::new(), detail: String::new(), input: Box::new(scan("I", Probe::All, Vec::new())), most: None };
    assert!(filled_plan(Plan::Distinct { input: Box::new(sorted), est: 0.0_f64, streamed: false }, &[]).is_ok());
    let probed = || scan("I", Probe::Prefix { equal: vec![Value::Null], then: None }, vec![ProbeSlot::of(Place::Key(0), slot(0))]);
    let product = Plan::Product { left: Box::new(probed()), right: Box::new(probed()), est: 0.0_f64 };
    let Plan::Product { right, .. } = filled_plan(product, &[int(3)]).unwrap() else { panic!("not a product") };
    let Plan::IndexScan { probe: right, .. } = *right else { panic!("not a scan") };
    assert!(same(&right, &Probe::Prefix { equal: vec![int(3)], then: None }), "a product's inputs filled (D418)");
    let other = Plan::Values { rows: Vec::new(), detail: String::new() };
    assert_eq!(filled_plan(other, &[]).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] A prepared multidimensional probe replaces each
/// comparison independently, including two on the same key column.
/// A missing comparison and every incompatible place fail with -910.
#[test]
fn independent_probe_slots_replace_each_bound_and_reject_missing_places() {
    use crate::index::ColumnComparison;
    let make = || {
        Probe::Columns(vec![
            ColumnComparison { column: 0, op: CompOp::Ge, value: Value::Null },
            ColumnComparison { column: 0, op: CompOp::Lt, value: Value::Null },
            ColumnComparison { column: 1, op: CompOp::Eq, value: Value::Null },
        ])
    };
    let slots = (0..3).map(|n| ProbeSlot::of(Place::Column(n), slot(n))).collect();
    let Plan::IndexScan { probe, .. } = filled_plan(scan("I", make(), slots), &[int(1), int(3), int(7)]).unwrap() else {
        panic!("not a scan")
    };
    assert!(probe.answers(&[int(2), int(7)]));
    for key in [[int(0), int(7)], [int(3), int(7)], [int(2), int(6)]] {
        assert!(!probe.answers(&key));
    }
    for place in [Place::Column(3), Place::Key(0), Place::Low, Place::High, Place::PredicateValue] {
        let plan = scan("I", make(), vec![ProbeSlot::of(place, slot(0))]);
        assert_eq!(filled_plan(plan, &[int(1)]).unwrap_err().code, Code::Api);
    }
    let wrong = scan("I", whole(), vec![ProbeSlot::of(Place::Column(0), slot(0))]);
    assert_eq!(filled_plan(wrong, &[int(1)]).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] RULES.md D613: a hash join is of the shape of one by
/// the same keys and more, as a plan made with values joins by a key two
/// columns equal to one value give, and not of one by fewer or other keys.
#[test]
fn a_hash_join_is_of_the_shape_of_one_by_more_keys() {
    let key = |left, right| crate::physical::JoinKey { left, right, nulls: false };
    let join = |keys| Plan::HashJoin {
        left: Box::new(scan("I", whole(), Vec::new())),
        right: Box::new(scan("I", whole(), Vec::new())),
        keys,
        detail: String::new(),
        est: 1.0_f64,
    };
    let (two, three) = (join(vec![key(4, 1), key(0, 0)]), join(vec![key(4, 1), key(0, 0), key(1, 0)]));
    assert!(same_shape(&two, &three), "the same keys and one more");
    assert!(!same_shape(&three, &two), "fewer keys");
    assert!(!same_shape(&two, &join(vec![key(4, 1), key(2, 0)])), "another key");
}
