//! Unit tests of the rule for DISTINCT set functions
//! (`optimizer/distinct_aggregates.rs`): when it applies, and the shape
//! of what it makes. Its results are compared with the plans made
//! without it in `test/integration/distinct_aggregates.rs`.

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::logical::Distribution;
use crate::value::{DataType, SortOrder};

/// A database of the built-in functions.
fn state() -> State {
    State::with_builtins(Box::new(MemoryCatalog::default()))
}

/// The call of the built-in set function `name` of column `col`, of
/// result `result`, DISTINCT if so.
fn call(db: &State, name: &str, col: usize, distinct: bool) -> AggCall {
    let func = db.functions.aggregate(name).unwrap();
    let result = func.0.result_type(Some(DataType::Boolean)).unwrap_or(DataType::Boolean);
    AggCall { func, arg: Some(Expr::Column(col)), distinct, filter: None, distribution: None, result, label: name.to_owned() }
}

/// An aggregation of `aggs` over a VALUES of two columns, grouped by
/// the first.
fn aggregation(aggs: Vec<AggCall>) -> LogicalPlan {
    let input = LogicalPlan::Values { rows: Vec::new(), types: vec![DataType::Boolean, DataType::Boolean] };
    LogicalPlan::Aggregate { groups: vec![0], collations: Vec::new(), aggs, seeds: Vec::new(), input: Box::new(input) }
}

/// What the rule makes of `plan`.
fn applied(plan: &LogicalPlan) -> Option<LogicalPlan> {
    let db = state();
    DistinctAggregates.apply(plan, &RuleContext { db: &db, seen: &crate::physical::Seen::default() })
}

/// Verifies: [DESIGN] RULES.md D340: the rule leaves alone a plan not an
/// aggregation, an aggregation of no DISTINCT function, and one of an
/// inverse distribution function.
///
/// Decisions: an aggregation or not; a DISTINCT function or not; an
/// inverse distribution function or not, each changing the outcome alone.
#[test]
fn leaves_plans_it_does_not_rewrite() {
    let db = state();
    assert!(applied(&LogicalPlan::Values { rows: Vec::new(), types: Vec::new() }).is_none());
    assert!(applied(&aggregation(vec![call(&db, "COUNT", 1, false)])).is_none());
    let mut sorted = call(&db, "COUNT", 1, false);
    sorted.distribution = Some(Box::new(Distribution { continuous: false, order: SortOrder::of(false, None), fraction: Expr::Column(1) }));
    assert!(applied(&aggregation(vec![call(&db, "COUNT", 1, true), sorted])).is_none());
    assert!(applied(&aggregation(vec![call(&db, "COUNT", 1, true)])).is_some());
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §10.9 GR7d iv] MAX and MIN
/// lose DISTINCT, in one aggregation; another DISTINCT function keeps it,
/// for the next pass.
#[test]
fn extremes_lose_distinct() {
    let db = state();
    let plan = aggregation(vec![call(&db, "MAX", 1, true), call(&db, "COUNT", 1, true), call(&db, "MIN", 1, true)]);
    let Some(LogicalPlan::Aggregate { aggs, .. }) = applied(&plan) else { panic!("no aggregation made") };
    assert_eq!(aggs.iter().map(|a| a.distinct).collect::<Vec<_>>(), [false, true, false]);
}

/// Verifies: [DESIGN] RULES.md D340, D342: functions of one argument
/// and filter share a slot; another argument is another slot, each row
/// then made once for each; a COUNT(*) is in parts, merged.
#[test]
fn slots_are_shared_and_copied() {
    let db = state();
    let star = AggCall { arg: None, label: "COUNT(*)".into(), ..call(&db, "COUNT", 1, false) };
    let shape = Shape::of(&[call(&db, "COUNT", 1, true), call(&db, "COUNT", 1, true), star.clone()]).unwrap();
    assert_eq!((shape.slots.len(), shape.copies(), shape.split), (1, 1, Split::State));
    let two = Shape::of(&[call(&db, "COUNT", 1, true), call(&db, "COUNT", 0, true), star]).unwrap();
    assert_eq!((two.slots.len(), two.copies()), (2, 2));
    let Some(LogicalPlan::Aggregate { aggs, input, .. }) = applied(&aggregation(two.aggs.clone())) else { panic!("no aggregation") };
    assert_eq!(aggs.iter().map(|a| a.func.0.name().to_owned()).collect::<Vec<_>>(), ["COUNT", "COUNT", crate::functions::MERGED]);
    let LogicalPlan::Aggregate { groups, aggs: parts, .. } = *input else { panic!("no first grouping") };
    assert_eq!((groups.len(), parts.len()), (4, 1));
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D1219: a
/// DISTINCT function whose argument is the sort key of a string under a
/// collation whose key is the text itself has its first grouping group
/// by the string under that collation, no key made of a row; COUNT then
/// counts the strings, and another function takes each group's key, made
/// of the group. Under another collation the key is grouped by as before,
/// under no collation.
///
/// Decisions: a sort key or not; a collation by octets or another; COUNT
/// or another function, each changing the outcome alone.
#[test]
fn a_key_of_octets_is_grouped_as_its_string() {
    use crate::value::collation::{CASE_INSENSITIVE, CollationRef, builtin, default_pad, unpadded_scalar};
    let db = state();
    let folded = CollationRef {
        collation: builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap(),
        pad: default_pad(CASE_INSENSITIVE),
    };
    let keyed = |name: &str, collation: &CollationRef| AggCall {
        arg: Some(Expr::SortKey(Box::new(Expr::Column(1)), collation.clone())),
        ..call(&db, name, 1, true)
    };
    // Whether the second's function takes a sort key, which of the
    // first's grouping columns are under a collation, and whether the
    // first groups by a sort key.
    let levels = |agg: AggCall| {
        let Some(LogicalPlan::Aggregate { aggs, input, .. }) = applied(&aggregation(vec![agg])) else { panic!("no aggregation") };
        let LogicalPlan::Aggregate { groups, collations, input: projected, .. } = *input else { panic!("no first grouping") };
        let LogicalPlan::Project { exprs, .. } = *projected else { panic!("no projection") };
        let is_key = |e: Option<&Expr>| matches!(e, Some(Expr::SortKey(..)));
        let grouped = groups.last().and_then(|&g| exprs.get(g));
        (is_key(aggs[0].arg.as_ref()), collations.iter().map(Option::is_some).collect::<Vec<_>>(), is_key(grouped))
    };
    let under = vec![false, true];
    assert_eq!(levels(keyed("COUNT", &unpadded_scalar())), (false, under.clone(), false), "the string grouped, COUNT counts it");
    assert_eq!(levels(keyed("SUM", &unpadded_scalar())), (true, under, false), "another function takes each group's key");
    assert_eq!(levels(keyed("COUNT", &folded)), (false, Vec::new(), true), "another collation's key is grouped by as made");
    assert_eq!(levels(call(&db, "COUNT", 1, true)), (false, Vec::new(), false), "no sort key, no collation");
}
