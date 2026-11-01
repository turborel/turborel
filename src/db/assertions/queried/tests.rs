//! Unit tests of `db/assertions/queried.rs`: the queries found of an
//! assertion's bound query, the shapes of none, and the plans made of them
//! over the rows changed (RULES.md R-TEST-1, R-TEST-8).

use std::sync::Arc;

use super::super::forms::{Form, Kind, Source, forms_of};
use super::*;
use crate::catalog::{Catalog, ConstraintAttributes, ConstraintDesc, ConstraintKind, MemoryCatalog, TableDesc};
use crate::engine::rows::ROW_STORE;
use crate::functions::{AggregateRef, Registry};
use crate::logical::{AggCall, OuterJoin, Subquery};
use crate::value::{Decimal, Spelled};

const COUNTED: DataType = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };
const REAL: DataType = DataType::Approximate { precision: 24, spelled: Spelled::Float };

fn named(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// Table `ident` of two columns, the first NOT NULL, the second not.
fn table(ident: &str) -> TableDesc {
    let attributes = ConstraintAttributes { deferrable: false, initially_deferred: false };
    let not_null = ConstraintDesc { name: named(&format!("{ident}0")), kind: ConstraintKind::NotNull(0), attributes, domain: None };
    TableDesc {
        name: named(ident),
        columns: Vec::new(),
        constraints: vec![not_null],
        engine: ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// A catalog of T and U, each as `table` makes it.
fn catalog() -> MemoryCatalog {
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table("T"), table("U")]);
    catalog
}

fn scan(ident: &str) -> LogicalPlan {
    LogicalPlan::Scan { table: named(ident), names: vec!["K".into(), "N".into()], needed: vec![0, 1] }
}

fn number(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

fn product(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Product { left: Box::new(left), right: Box::new(right) }
}

fn filter(cond: Cond, input: LogicalPlan) -> LogicalPlan {
    LogicalPlan::Filter { cond, input: Box::new(input) }
}

/// T's N equal to U's N, of their product: columns 1 and 3.
fn joined() -> LogicalPlan {
    filter(Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Column(3)), product(scan("T"), scan("U")))
}

/// T and U, their product on no condition.
fn crossed() -> LogicalPlan {
    product(scan("T"), scan("U"))
}

/// `SELECT K FROM T GROUP BY K HAVING f(arg) > 1`, of the call `f`.
fn grouped(f: AggregateRef, (arg, result): (Option<Expr>, DataType)) -> LogicalPlan {
    let agg = AggCall { func: f, arg, distinct: false, result, label: "F".into() };
    let aggregate = LogicalPlan::Aggregate { groups: vec![0], aggs: vec![agg], input: Box::new(scan("T")) };
    let having = filter(Cond::Compare(Expr::Column(1), CompOp::Gt, number(1)), aggregate);
    LogicalPlan::Project { exprs: vec![Expr::Column(0)], input: Box::new(having) }
}

/// The forms of an assertion `NOT EXISTS (SELECT 1 FROM rows)`.
fn none_of(rows: LogicalPlan, functions: &Registry) -> Option<Vec<Form>> {
    let s = Arc::new(Subquery { plan: LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(rows) }, outer: Vec::new() });
    asserted(Cond::Not(Box::new(Cond::Exists(s))), functions)
}

/// The forms of an assertion of `condition`.
fn asserted(condition: Cond, functions: &Registry) -> Option<Vec<Form>> {
    let values = LogicalPlan::Values { rows: vec![vec![number(1)]], types: vec![COUNTED] };
    let filter = LogicalPlan::Filter { cond: Cond::Not(Box::new(condition)), input: Box::new(values) };
    let plan = LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(filter) };
    let catalog = catalog();
    forms_of(&plan, &Known { functions, catalog: &catalog })
}

/// The one form found, of a query's rows: its shape's name, and the form.
fn query_form(found: Option<Vec<Form>>) -> (&'static str, Form) {
    let mut found = found.expect("a form");
    assert_eq!(found.len(), 1_usize);
    let form = found.pop().unwrap();
    let Source::Query(query) = &form.source else { panic!("not a query's rows") };
    let shape = match query.shape {
        Shape::Joined => "joined",
        Shape::Grouped { .. } => "grouped",
    };
    (shape, form)
}

/// `(SELECT f(x) FROM rows) op 10`.
fn bounded(f: AggregateRef, x: Expr, rows: LogicalPlan, op: CompOp) -> Cond {
    let agg = AggCall { func: f, arg: Some(x), distinct: false, result: COUNTED, label: "F".into() };
    let aggregate = LogicalPlan::Aggregate { groups: Vec::new(), aggs: vec![agg], input: Box::new(rows) };
    let s =
        Arc::new(Subquery { plan: LogicalPlan::Project { exprs: vec![Expr::Column(0)], input: Box::new(aggregate) }, outer: Vec::new() });
    Cond::Compare(Expr::Subquery(s, 0), op, number(10))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5, §11.34 GR2]; [DESIGN]
/// RULES.md D51: NOT EXISTS of inner joins, of a table joined to itself,
/// to constant rows, or of DISTINCT, is a query's rows, of no row passing.
#[test]
fn inner_joins_are_queries_11_34() {
    let functions = Registry::with_set_functions();
    let values = LogicalPlan::Values { rows: vec![vec![number(1), number(2)]], types: vec![COUNTED; 2] };
    let cases = [joined(), product(scan("T"), scan("T")), product(values, scan("T")), LogicalPlan::Distinct { input: Box::new(scan("T")) }];
    for (n, rows) in cases.into_iter().enumerate() {
        let (shape, form) = query_form(none_of(rows, &functions));
        assert_eq!(shape, "joined", "case {n}");
        assert!(matches!(form.kind, Kind::NoRow) && form.steps.is_empty() && form.nulls.is_none(), "case {n}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, §7.7 GR1, §7.8,
/// §11.34 GR2]; [DESIGN] RULES.md D51: NOT EXISTS of a grouping of one
/// table's rows, HAVING of the group, is a query's rows, keyed by its
/// grouping columns over the table's row; its argument may be null unless
/// it is never null, as of a table's form.
#[test]
fn groupings_are_queries_7_7() {
    let functions = Registry::with_set_functions();
    let [count, sum, max] = ["COUNT", "SUM", "MAX"].map(|f| functions.aggregate(f).unwrap());
    let cases = [(count, None, false), (sum, Some(Expr::Column(1)), true), (max, Some(Expr::Column(0)), false)];
    for (n, (f, arg, may_be_null)) in cases.into_iter().enumerate() {
        let (shape, form) = query_form(none_of(grouped(f, (arg, COUNTED)), &functions));
        assert_eq!(shape, "grouped", "case {n}");
        let Source::Query(query) = &form.source else { panic!("case {n}") };
        let Shape::Grouped { table, keys, nullable, steps } = &query.shape else { panic!("case {n}") };
        assert_eq!((table, keys.len(), *nullable, steps.len()), (&named("T"), 1_usize, may_be_null, 0_usize), "case {n}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b iv, §7.5, §7.10,
/// §11.34 GR2]; [DESIGN] RULES.md D15, D51: no query is found of an outer
/// join, a set operator, a grouping of a join or of no grouping column, a
/// set function a plug-in replaced, SUM of approximate numbers, a
/// function in a join's condition, nor of a count, or a greatest reached,
/// of a query's rows.
#[test]
fn other_queries_are_checked_whole_11_34() {
    let functions = Registry::with_set_functions();
    let [count, sum, max] = ["COUNT", "SUM", "MAX"].map(|f| functions.aggregate(f).unwrap());
    let length = functions.scalar("CHAR_LENGTH").unwrap();
    let outer = LogicalPlan::OuterJoin {
        kind: OuterJoin::Left,
        cond: Cond::Const(Truth::True),
        left: Box::new(scan("T")),
        right: Box::new(scan("U")),
    };
    let set = LogicalPlan::SetOp { op: SetOp::Except, all: false, left: Box::new(scan("T")), right: Box::new(scan("U")) };
    let agg = AggCall { func: count.clone(), arg: None, distinct: false, result: COUNTED, label: "F".into() };
    let of_join = LogicalPlan::Aggregate { groups: vec![0], aggs: vec![agg.clone()], input: Box::new(joined()) };
    let of_none = LogicalPlan::Aggregate { groups: Vec::new(), aggs: vec![agg], input: Box::new(scan("T")) };
    let called =
        filter(Cond::Compare(Expr::Call(length, vec![Expr::Column(1)]), CompOp::Eq, Expr::Column(3)), product(scan("T"), scan("U")));
    let mut replaced = Registry::with_set_functions();
    replaced.add_aggregate(Arc::clone(&sum.0)).unwrap();
    let cases = [
        none_of(outer, &functions),
        none_of(set, &functions),
        none_of(of_join, &functions),
        none_of(of_none, &functions),
        none_of(grouped(sum.clone(), (Some(Expr::Column(1)), REAL)), &functions),
        none_of(grouped(sum, (Some(Expr::Column(1)), COUNTED)), &replaced),
        none_of(called, &functions),
        asserted(bounded(count, Expr::Column(1), joined(), CompOp::Le), &functions),
        asserted(bounded(max, Expr::Column(1), joined(), CompOp::Ge), &functions),
    ];
    for (n, found) in cases.into_iter().enumerate() {
        assert!(found.is_none(), "case {n}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2b iii, §11.34
/// GR2]; [DESIGN] RULES.md D51: MAX bounded above, and MIN below, of a
/// join's rows is a query's, `NOT (x op k)` a step of each row; its
/// argument may be null but of a column NOT NULL, or one a filter above
/// it, as a join's equality, is not true of null.
#[test]
fn a_greatest_bounded_of_a_join_is_a_query_6_5() {
    let functions = Registry::with_set_functions();
    let [max, min] = ["MAX", "MIN"].map(|f| functions.aggregate(f).unwrap());
    let present = filter(Cond::IsNull(Expr::Column(1), true), crossed());
    let cases = [
        (bounded(max.clone(), Expr::Column(1), crossed(), CompOp::Le), true),
        (bounded(max.clone(), Expr::Column(1), joined(), CompOp::Le), false),
        (bounded(max, Expr::Column(2), joined(), CompOp::Lt), false),
        (bounded(min.clone(), Expr::Column(1), present, CompOp::Ge), false),
        (bounded(min, number(3), joined(), CompOp::Gt), false),
    ];
    for (n, (condition, may_be_null)) in cases.into_iter().enumerate() {
        let (shape, form) = query_form(asserted(condition, &functions));
        assert_eq!(shape, "joined", "case {n}");
        assert!(matches!(form.kind, Kind::Bound), "case {n}");
        assert_eq!((form.steps.len(), form.nulls.is_some()), (1_usize, may_be_null), "case {n}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1a, GR2a, §11.34 GR2];
/// [DESIGN] RULES.md D51: a column of a query is never null as its source
/// is: a table's column NOT NULL, either side of a product, one a filter
/// above it is not true of null, through a projection, a grouping column
/// of one, or COUNT; not another set function's, nor a constant row's.
#[test]
fn a_querys_columns_never_null_are_traced_6_5() {
    let functions = Registry::with_set_functions();
    let catalog = catalog();
    let known = Known { functions: &functions, catalog: &catalog };
    let [count, max] = ["COUNT", "MAX"].map(|f| functions.aggregate(f).unwrap());
    let calls = [count, max].map(|func| AggCall { func, arg: Some(Expr::Column(1)), distinct: false, result: COUNTED, label: "F".into() });
    let aggregate = LogicalPlan::Aggregate { groups: vec![0], aggs: calls.to_vec(), input: Box::new(scan("T")) };
    let projected = LogicalPlan::Project { exprs: vec![Expr::Column(2), Expr::Column(1)], input: Box::new(crossed()) };
    let values = LogicalPlan::Values { rows: vec![vec![number(1)]], types: vec![COUNTED] };
    let cases = [
        (crossed(), 0_usize, true),
        (crossed(), 1, false),
        (crossed(), 2, true),
        (crossed(), 3, false),
        (joined(), 1, true),
        (joined(), 3, true),
        (projected.clone(), 0, true),
        (projected, 1, false),
        (aggregate.clone(), 0, true),
        (aggregate.clone(), 1, true),
        (aggregate.clone(), 2, false),
        (aggregate, 3, false),
        (values, 0, false),
    ];
    for (n, (plan, at, never)) in cases.into_iter().enumerate() {
        assert_eq!(never_null_in(&plan, &Expr::Column(at), &known), never, "case {n}");
    }
    let sum = Expr::Arith(Box::new(Expr::Column(0)), crate::value::ArithOp::Add, Box::new(number(1)));
    assert!(never_null_in(&joined(), &sum, &known));
    assert!(!never_null_in(&joined(), &Expr::Const(Value::Null), &known));
}

/// Verifies: [DESIGN] RULES.md D51: of a query's scans, counted in order,
/// one is made of rows changed, as constant rows; the others are kept.
#[test]
fn a_scan_is_made_of_the_rows_changed() {
    let rows = vec![vec![number_value(7), Value::Null]];
    let joined_itself = product(scan("T"), scan("T"));
    assert_eq!(scans(&joined_itself), [named("T"), named("T")]);
    let LogicalPlan::Product { left, right } = with_rows(joined_itself, 1, (&rows, &[COUNTED; 2])) else { panic!("not a product") };
    assert!(matches!(*left, LogicalPlan::Scan { .. }), "the first kept");
    let LogicalPlan::Values { rows: made, types } = *right else { panic!("not made of the rows") };
    assert_eq!((made.len(), types.len()), (1_usize, 2_usize));
    assert!(matches!(&made[0][..], [Expr::Const(Value::Exact(_)), Expr::Const(Value::Null)]));
}

fn number_value(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §3.1.3h, §7.7 GR1];
/// [DESIGN] RULES.md D51: a grouping is made of the groups of keys alone:
/// those of no null joined to the keys as constant rows on equalities,
/// those of a null by a condition of IS NULL there, the two together kept;
/// keys of no types, not columns, by a condition of each.
#[test]
fn a_grouping_is_made_of_the_groups_changed_7_7() {
    let functions = Registry::with_set_functions();
    let count = functions.aggregate("COUNT").unwrap();
    let typed: &[DataType] = &[COUNTED];
    let of_typed = |keys: &[Row], types: Option<&[DataType]>| {
        let LogicalPlan::Project { input, .. } = of_groups(grouped(count.clone(), (None, COUNTED)), (keys, types)) else {
            panic!("no projection")
        };
        let LogicalPlan::Filter { input, .. } = *input else { panic!("no HAVING") };
        let LogicalPlan::Aggregate { input, .. } = *input else { panic!("no grouping") };
        *input
    };
    let of = |keys: &[Row]| of_typed(keys, Some(typed));
    let joined_keys = of(&[vec![number_value(1)], vec![number_value(2)]]);
    let LogicalPlan::Project { exprs, input } = joined_keys else { panic!("not projected to the table's columns") };
    assert_eq!(exprs.len(), 2_usize);
    let LogicalPlan::Filter { cond: Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(1)), input } = *input else {
        panic!("no equality")
    };
    assert!(matches!(*input, LogicalPlan::Product { .. }));
    let nulled = of(&[vec![Value::Null]]);
    assert!(matches!(nulled, LogicalPlan::Filter { cond: Cond::IsNull(Expr::Column(0), false), .. }));
    let both = of(&[vec![number_value(1)], vec![Value::Null]]);
    assert!(matches!(both, LogicalPlan::SetOp { op: SetOp::Union, all: true, .. }));
    assert!(matches!(of(&[]), LogicalPlan::Filter { cond: Cond::Const(Truth::False), .. }));
    let untyped = of_typed(&[vec![number_value(1)]], None);
    assert!(matches!(untyped, LogicalPlan::Filter { cond: Cond::Compare(..), .. }), "keys of no types by a condition");
}
