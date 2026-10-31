//! Unit tests of `db/assertions/forms.rs`: the forms found of an
//! assertion's bound query, the steps of each, and the shapes of none
//! (RULES.md R-TEST-1, R-TEST-8).

use std::sync::Arc;

use super::*;
use crate::catalog::{ConstraintAttributes, ConstraintDesc, MemoryCatalog, TableDesc};
use crate::engine::rows::ROW_STORE;
use crate::functions::AggregateRef;
use crate::logical::{AggCall, InSet};
use crate::value::{ArithOp, DataType, Decimal, Spelled, Value};

const COUNTED: DataType = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };

fn table() -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: "T".into() }
}

/// A constraint of T of `kind`, deferrable if `deferrable`.
fn constraint(ident: &str, kind: ConstraintKind, deferrable: bool) -> ConstraintDesc {
    let attributes = ConstraintAttributes { deferrable, initially_deferred: false };
    ConstraintDesc { name: QualifiedName { auth: "A".into(), ident: ident.into() }, kind, attributes, domain: None }
}

/// A catalog of T, whose column 0 is NOT NULL, 2 NOT NULL DEFERRABLE, 3
/// its PRIMARY KEY and 4 UNIQUE; column 1 has none.
fn catalog() -> MemoryCatalog {
    let constraints = vec![
        constraint("K0", ConstraintKind::NotNull(0), false),
        constraint("K2", ConstraintKind::NotNull(2), true),
        constraint("K3", ConstraintKind::Unique { columns: vec![3], primary: true }, false),
        constraint("K4", ConstraintKind::Unique { columns: vec![4], primary: false }, false),
    ];
    let desc = TableDesc {
        name: table(),
        columns: Vec::new(),
        constraints,
        engine: ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![desc]);
    catalog
}

fn scan() -> LogicalPlan {
    LogicalPlan::Scan { table: table(), names: vec!["K".into(), "N".into()], needed: vec![0, 1] }
}

fn number(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// `a op b`.
fn compare(a: Expr, op: CompOp, b: Expr) -> Cond {
    Cond::Compare(a, op, b)
}

/// The rows of T where `cond`.
fn filtered(cond: Cond) -> LogicalPlan {
    LogicalPlan::Filter { cond, input: Box::new(scan()) }
}

fn subquery(plan: LogicalPlan) -> Arc<Subquery> {
    Arc::new(Subquery { plan, outer: Vec::new() })
}

/// `NOT EXISTS (SELECT exprs FROM rows)`.
fn none_of(exprs: Vec<Expr>, rows: LogicalPlan) -> Cond {
    Cond::Not(Box::new(Cond::Exists(subquery(LogicalPlan::Project { exprs, input: Box::new(rows) }))))
}

/// `(SELECT f(arg) FROM rows) op 10`, DISTINCT if `distinct`.
fn aggregated(f: AggregateRef, (arg, distinct): (Option<Expr>, bool), rows: LogicalPlan, op: CompOp) -> Cond {
    let agg = AggCall { func: f, arg, distinct, result: COUNTED, label: "F".into() };
    let aggregate = LogicalPlan::Aggregate { groups: Vec::new(), aggs: vec![agg], input: Box::new(rows) };
    let s = subquery(LogicalPlan::Project { exprs: vec![Expr::Column(0)], input: Box::new(aggregate) });
    compare(Expr::Subquery(s, 0), op, number(10))
}

/// `(SELECT f(arg) FROM rows) op 10`.
fn counted(f: AggregateRef, arg: Option<Expr>, rows: LogicalPlan, op: CompOp) -> Cond {
    aggregated(f, (arg, false), rows, op)
}

/// The bound query of an assertion of `condition`.
fn asserting(condition: Cond) -> LogicalPlan {
    let values = LogicalPlan::Values { rows: vec![vec![number(1)]], width: 1 };
    let filter = LogicalPlan::Filter { cond: Cond::Not(Box::new(condition)), input: Box::new(values) };
    LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(filter) }
}

/// The forms of an assertion of `condition`, in a database of `functions`
/// and T.
fn forms(condition: Cond, functions: &Registry) -> Option<Vec<Form>> {
    let catalog = catalog();
    forms_of(&asserting(condition), &Known { functions, catalog: &catalog })
}

/// What `kind` is, by name.
fn named(kind: &Kind) -> &'static str {
    match kind {
        Kind::NoRow => "no row",
        Kind::Bound => "bound",
        Kind::Count(Safe::Fewer) => "fewer",
        Kind::Count(Safe::More) => "more",
        Kind::Count(Safe::Same) => "same",
        Kind::Reach(_) => "reach",
        Kind::Distinct(Safe::Fewer, _) => "distinct fewer",
        Kind::Distinct(Safe::More, _) => "distinct more",
        Kind::Distinct(Safe::Same, _) => "distinct same",
        Kind::Apart { .. } => "apart",
    }
}

fn kinds(condition: Cond, functions: &Registry) -> Option<Vec<&'static str>> {
    forms(condition, functions).map(|forms| forms.iter().map(|f| named(&f.kind)).collect())
}

/// The steps of the one form of `condition`, each as Keep or Compute.
fn steps(condition: Cond, functions: &Registry) -> Vec<String> {
    let mut found = forms(condition, functions).unwrap();
    assert_eq!(found.len(), 1_usize);
    let steps = found.pop().unwrap().steps;
    steps.iter().map(|s| format!("{s:?}")).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN]
/// RULES.md D51: NOT EXISTS of filters of a row's conditions over a
/// table, and COUNT(*) of them compared with a constant, either side,
/// are found, each conjunct of an AND.
#[test]
fn forms_are_found_of_each_conjunct() {
    let functions = Registry::with_set_functions();
    let count = functions.aggregate("COUNT").unwrap();
    let row = Cond::And(
        Box::new(compare(Expr::Neg(Box::new(Expr::Column(1))), CompOp::Lt, number(0))),
        Box::new(Cond::Is(Box::new(Cond::IsNull(Expr::Column(0), true)), Truth::True, false)),
    );
    let listed = Cond::In(Expr::Column(0), Arc::new(InSet::of(vec![Value::Exact(Decimal::new(1, 0))])));
    let row = Cond::And(Box::new(row), Box::new(listed));
    let case = Expr::Case(vec![(Cond::Const(Truth::True), Expr::Column(0))], Box::new(number(0)));
    let like = Cond::Like(Expr::Column(1), number(1), Some(number(2)));
    let Cond::Compare(s, op, k) = counted(count.clone(), None, scan(), CompOp::Lt) else { panic!("a comparison") };
    let flipped = Cond::Compare(k, op, s);
    let condition = [
        none_of(vec![number(1)], filtered(row)),
        counted(count.clone(), None, filtered(Cond::Or(Box::new(like), Box::new(compare(case, CompOp::Eq, number(3))))), CompOp::Le),
        counted(count.clone(), None, scan(), CompOp::Ne),
        flipped,
    ]
    .into_iter()
    .reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))
    .unwrap();
    assert_eq!(kinds(condition, &functions), Some(vec!["no row", "fewer", "same", "more"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2b iii, §11.34
/// GR2]; [DESIGN] RULES.md D51: COUNT, MAX and MIN of an argument never
/// null have forms: COUNT a count of the rows where it is not null; MAX
/// below a bound and MIN above one, none of the rows past it, whether
/// DISTINCT or not; MAX above a bound and MIN below one, a row past it.
#[test]
fn set_functions_of_values_never_null_have_forms() {
    let functions = Registry::with_set_functions();
    let [count, max, min] = ["COUNT", "MAX", "MIN"].map(|f| functions.aggregate(f).unwrap());
    let k = || Some(Expr::Column(0));
    let condition = [
        counted(count, k(), scan(), CompOp::Le),
        counted(max.clone(), k(), scan(), CompOp::Lt),
        counted(max.clone(), k(), scan(), CompOp::Le),
        counted(min.clone(), k(), scan(), CompOp::Gt),
        aggregated(min.clone(), (k(), true), scan(), CompOp::Ge),
        counted(max.clone(), k(), scan(), CompOp::Gt),
        aggregated(max, (k(), true), scan(), CompOp::Ge),
        counted(min.clone(), k(), scan(), CompOp::Lt),
        counted(min, k(), scan(), CompOp::Le),
    ]
    .into_iter()
    .reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))
    .unwrap();
    let found = kinds(condition, &functions);
    assert_eq!(found, Some(vec!["fewer", "bound", "bound", "bound", "bound", "reach", "reach", "reach", "reach"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, §11.34 GR2];
/// [DESIGN] RULES.md D51, R-TEST-8: a set function's argument is never
/// null, so its form keeps no argument to watch for a null, as a column of a NOT NULL or PRIMARY KEY
/// constraint not deferrable, one a filter's conjunct rejects null of
/// (compared, IS NOT NULL, LIKE, IN, under AND or IS TRUE, through
/// arithmetic), a constant not null, or arithmetic of those; not a
/// nullable column, one deferrable or only UNIQUE, a null, one a filter
/// lets be null (OR, NOT, IS NULL, IS FALSE), nor a CASE.
#[test]
fn a_set_functions_argument_is_never_null() {
    let functions = Registry::with_set_functions();
    let max = functions.aggregate("MAX").unwrap();
    let found = |arg: Expr, rows: LogicalPlan| {
        let mut found = forms(counted(max.clone(), Some(arg), rows, CompOp::Lt), &functions).unwrap();
        found.pop().unwrap().nulls.is_none()
    };
    let plus = |a: Expr, b: Expr| Expr::Arith(Box::new(a), ArithOp::Add, Box::new(b));
    let n = || Expr::Column(1);
    let listed = || Arc::new(InSet::of(vec![Value::Exact(Decimal::new(1, 0))]));
    let null = Expr::Const(Value::Null);
    let rejecting = [
        compare(plus(n(), number(1)), CompOp::Gt, number(0)),
        compare(number(0), CompOp::Lt, Expr::Neg(Box::new(n()))),
        Cond::IsNull(n(), true),
        Cond::Like(n(), number(1), None),
        Cond::In(n(), listed()),
        Cond::Is(Box::new(compare(n(), CompOp::Eq, number(1))), Truth::True, false),
        Cond::And(Box::new(Cond::Const(Truth::True)), Box::new(Cond::IsNull(n(), true))),
    ];
    for (at, cond) in rejecting.into_iter().enumerate() {
        assert!(found(n(), filtered(cond)), "rejecting {at}");
    }
    for column in [0_usize, 3_usize] {
        assert!(found(Expr::Column(column), scan()), "declared {column}");
    }
    assert!(found(plus(Expr::Neg(Box::new(Expr::Column(0))), number(2)), scan()), "arithmetic of those");
    let letting = [
        Cond::Or(Box::new(Cond::IsNull(n(), true)), Box::new(Cond::Const(Truth::True))),
        Cond::Not(Box::new(Cond::IsNull(n(), false))),
        Cond::IsNull(n(), false),
        Cond::Is(Box::new(compare(n(), CompOp::Eq, number(1))), Truth::False, false),
        Cond::Is(Box::new(compare(n(), CompOp::Eq, number(1))), Truth::True, true),
        compare(Expr::Column(0), CompOp::Gt, number(0)),
        compare(Expr::Case(vec![(Cond::Const(Truth::True), n())], Box::new(number(0))), CompOp::Gt, number(0)),
    ];
    for (at, cond) in letting.into_iter().enumerate() {
        assert!(!found(n(), filtered(cond)), "letting {at}");
    }
    for column in [1_usize, 2_usize, 4_usize] {
        assert!(!found(Expr::Column(column), scan()), "undeclared {column}");
    }
    assert!(!found(null.clone(), scan()), "a null");
    assert!(!found(plus(Expr::Column(0), null), scan()), "arithmetic of a null");
    let case = Expr::Case(vec![(Cond::Const(Truth::True), Expr::Column(0))], Box::new(number(0)));
    assert!(!found(case, scan()), "a CASE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2b iii, iv,
/// §11.34 GR2]; [DESIGN] RULES.md D51: no form is found of a function, a
/// CAST, a subquery in a filter, an outer reference, a select list that can
/// raise, a join, a grouping, MAX of no argument, SUM or AVG, a COUNT a
/// plug-in replaced, an OR of conjuncts, nor of a query not an assertion's.
#[test]
fn other_shapes_have_no_form() {
    let functions = Registry::with_set_functions();
    let [count, max, sum, avg] = ["COUNT", "MAX", "SUM", "AVG"].map(|f| functions.aggregate(f).unwrap());
    let length = functions.scalar("CHAR_LENGTH").unwrap();
    let cast = Expr::Cast(Box::new(Expr::Column(0)), COUNTED);
    let call = Expr::Call(length, vec![Expr::Column(1)]);
    let inner = Cond::Exists(subquery(scan()));
    let raising = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Div, Box::new(number(0)));
    let joined = LogicalPlan::Product { left: Box::new(scan()), right: Box::new(scan()) };
    let grouped = LogicalPlan::Aggregate { groups: vec![0], aggs: Vec::new(), input: Box::new(scan()) };
    let k = || Some(Expr::Column(0));
    let cases = [
        none_of(vec![number(1)], filtered(compare(call, CompOp::Eq, number(1)))),
        none_of(vec![number(1)], filtered(Cond::IsNull(cast.clone(), false))),
        none_of(vec![number(1)], filtered(inner)),
        none_of(vec![raising], scan()),
        none_of(vec![number(1)], joined),
        none_of(vec![number(1)], LogicalPlan::Distinct { input: Box::new(scan()) }),
        Cond::Not(Box::new(Cond::Exists(Arc::new(Subquery { plan: scan(), outer: vec![Expr::Column(0)] })))),
        Cond::Not(Box::new(Cond::Const(Truth::True))),
        counted(count.clone(), None, grouped, CompOp::Le),
        counted(max.clone(), None, scan(), CompOp::Lt),
        counted(max, Some(cast), scan(), CompOp::Lt),
        counted(sum, k(), scan(), CompOp::Le),
        counted(avg, k(), scan(), CompOp::Le),
        Cond::Or(Box::new(counted(count.clone(), None, scan(), CompOp::Le)), Box::new(counted(count.clone(), None, scan(), CompOp::Ge))),
    ];
    for (n, condition) in cases.into_iter().enumerate() {
        assert!(kinds(condition, &functions).is_none(), "case {n}");
    }
    let mut replaced = Registry::with_set_functions();
    replaced.add_aggregate(Arc::clone(&functions.aggregate("SUM").unwrap().0)).unwrap();
    let kept = replaced.aggregate("COUNT").unwrap();
    assert!(kinds(counted(kept, None, scan(), CompOp::Le), &replaced).is_some(), "its COUNT, another set function replaced");
    let mut other = Registry::with_set_functions();
    other.add_aggregate(Arc::clone(&count.0)).unwrap();
    assert!(kinds(counted(count, None, scan(), CompOp::Le), &other).is_none(), "another registry's COUNT");
    let catalog = catalog();
    let known = Known { functions: &functions, catalog: &catalog };
    assert!(forms_of(&scan(), &known).is_none());
    let unfiltered = LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(scan()) };
    assert!(forms_of(&unfiltered, &known).is_none());
    let over_rows = LogicalPlan::Project {
        exprs: vec![number(1)],
        input: Box::new(LogicalPlan::Filter { cond: Cond::Not(Box::new(Cond::Const(Truth::True))), input: Box::new(scan()) }),
    };
    assert!(forms_of(&over_rows, &known).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2];
/// [DESIGN] RULES.md D15, D51: the rows of a view not merged into the
/// query pass its filter, then its projection's expressions that can raise,
/// evaluated, then the query's filter over them, rewritten over the
/// table's row; a set function's argument too. A projection of a function
/// has no form.
#[test]
fn a_views_projection_is_a_step() {
    let functions = Registry::with_set_functions();
    let twice = Expr::Arith(Box::new(Expr::Column(1)), ArithOp::Mul, Box::new(number(2)));
    let view =
        |exprs: Vec<Expr>| LogicalPlan::Project { exprs, input: Box::new(filtered(compare(Expr::Column(0), CompOp::Lt, number(100)))) };
    let above = |rows: LogicalPlan| LogicalPlan::Filter { cond: compare(Expr::Column(1), CompOp::Gt, number(5)), input: Box::new(rows) };
    let found = steps(none_of(vec![number(1)], above(view(vec![Expr::Column(0), twice.clone()]))), &functions);
    let expected = [
        format!("{:?}", Step::Keep(compare(Expr::Column(0), CompOp::Lt, number(100)))),
        format!("{:?}", Step::Compute(vec![twice.clone()])),
        format!("{:?}", Step::Keep(compare(twice.clone(), CompOp::Gt, number(5)))),
    ];
    assert_eq!(found, expected);
    let plain = steps(none_of(vec![number(1)], above(view(vec![Expr::Column(0), Expr::Column(1)]))), &functions);
    assert_eq!(plain.len(), 2_usize, "nothing to compute: {plain:?}");
    let max = functions.aggregate("MAX").unwrap();
    // MAX(D) of the rows D > 5 of the view's (K, N * 2 AS D): never null.
    let nested = LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(above(view(vec![Expr::Column(0), twice.clone()]))) };
    let of_view = counted(max, Some(Expr::Column(0)), nested, CompOp::Lt);
    let bounded = steps(of_view, &functions);
    let bound = Cond::Not(Box::new(compare(twice.clone(), CompOp::Lt, number(10))));
    assert_eq!(bounded.last(), Some(&format!("{:?}", Step::Keep(bound))), "{bounded:?}");
    let length = functions.scalar("CHAR_LENGTH").unwrap();
    let called = view(vec![Expr::Call(length, vec![Expr::Column(1)])]);
    assert!(kinds(none_of(vec![number(1)], called), &functions).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, GR2, GR2b iii,
/// §8.2 GR1a, §11.34 GR2]; [DESIGN] RULES.md D51: MAX or MIN equal to a
/// bound is two forms, below and reaching it; unequal one, apart from it;
/// COUNT DISTINCT a count of values; and a set function of an argument
/// that may be null keeps it, with the steps before its own.
#[test]
fn equal_unequal_distinct_and_nullable_have_forms() {
    let functions = Registry::with_set_functions();
    let [count, max, min] = ["COUNT", "MAX", "MIN"].map(|f| functions.aggregate(f).unwrap());
    let k = || Some(Expr::Column(0));
    let condition = [
        counted(max.clone(), k(), scan(), CompOp::Eq),
        counted(min.clone(), k(), scan(), CompOp::Eq),
        counted(max.clone(), k(), scan(), CompOp::Ne),
        counted(min, k(), scan(), CompOp::Ne),
        aggregated(count.clone(), (k(), true), scan(), CompOp::Le),
        aggregated(count.clone(), (k(), true), scan(), CompOp::Gt),
        aggregated(count.clone(), (k(), true), scan(), CompOp::Eq),
    ]
    .into_iter()
    .reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))
    .unwrap();
    let found = kinds(condition, &functions);
    let expected = ["bound", "reach", "bound", "reach", "apart", "apart", "distinct fewer", "distinct more", "distinct same"];
    assert_eq!(found, Some(expected.to_vec()));
    let rows = filtered(compare(Expr::Column(0), CompOp::Gt, number(0)));
    let mut nullable = forms(counted(count, Some(Expr::Column(1)), rows.clone(), CompOp::Le), &functions).unwrap();
    let form = nullable.pop().unwrap();
    assert_eq!(
        (form.steps.len(), form.nulls.map(|(at, x)| (at, format!("{x:?}")))),
        (2_usize, Some((1_usize, format!("{:?}", Expr::Column(1)))))
    );
    let mut reached = forms(counted(max, Some(Expr::Column(1)), rows, CompOp::Gt), &functions).unwrap();
    assert_eq!(reached.pop().unwrap().nulls.map(|(at, _)| at), Some(1_usize), "no step of its own");
}
