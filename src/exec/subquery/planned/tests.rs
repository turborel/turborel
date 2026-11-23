//! Unit tests: what a runner keeps of a correlated subquery's plan after
//! each run, and the plan each run is given (RULES.md D675, R-TEST-1,
//! R-TEST-8).

use std::sync::Arc;

use super::*;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::index::Probe;
use crate::logical::Cond;
use crate::physical::same_shape;
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal};

/// A state whose table `A.U` (K INTEGER, W INTEGER) holds 200 rows: K
/// unique if `unique`, else of 10 values, 20 rows each, indexed by the
/// index `UK`; its subqueries decorrelated after more runs than any test's.
fn state(unique: bool) -> Arc<State> {
    state_of(unique, 1_000_000)
}

/// `state(unique)`, its subqueries decorrelated after `after` runs.
fn state_of(unique: bool, after: i128) -> Arc<State> {
    let db = crate::db::Database::new();
    db.set_setting("EXECUTOR.DECORRELATE_AFTER_RUNS", &int(after)).unwrap();
    let key = if unique { ", UNIQUE (K)" } else { "" };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE U (K INTEGER NOT NULL, W INTEGER{key})")).unwrap();
    let s = db.session("A");
    let keys: usize = if unique { 200 } else { 10 };
    let rows: Vec<String> = (0..200_usize).map(|n| format!("({}, {n})", n.checked_rem(keys).unwrap())).collect();
    s.run(&format!("INSERT INTO U VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    if !unique {
        s.run("CREATE INDEX UK ON U (K)").unwrap();
        s.commit().unwrap();
    }
    db.snapshot().unwrap()
}

/// The subquery `SELECT W FROM U WHERE K = ?`, its `?` the outer reference
/// `T.K`.
fn subquery() -> Subquery {
    let table = QualifiedName { auth: "A".into(), ident: "U".into() };
    let scan = LogicalPlan::Scan { table, names: vec!["K".into(), "W".into()], needed: vec![0, 1], located: false };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Outer(0, "T.K".into()));
    let filter = LogicalPlan::Filter { cond, input: Box::new(scan) };
    Subquery::new(LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(filter) }, vec![Expr::Column(0)])
}

/// More runs than a plan learns over before it is kept or not.
const MOST_RUNS: i128 = 20;

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The probe of the index scan of `plan`, under its projection and filter.
fn probe(plan: &Plan) -> Option<&Probe> {
    if let Plan::Project { input, .. } | Plan::Filter { input, .. } = plan {
        return probe(input);
    }
    if let Plan::IndexScan { probe, .. } = plan { Some(probe) } else { None }
}

/// `p` as text, as a probe compares no other way.
fn shown(p: Option<&Probe>) -> String {
    format!("{p:?}")
}

/// Which `Planned` `p` is.
fn named(p: &Planned) -> &'static str {
    match p {
        Planned::Untried => "untried",
        Planned::Learning(..) => "learning",
        Planned::Kept(..) => "kept",
        Planned::Varies => "varies",
    }
}

/// Verifies: [DESIGN] RULES.md D675: a correlated subquery learns its
/// plan with slots over its first runs, each run given the plan made
/// with its values, and once runs in a row agree, after the first, keeps it:
/// each later run is given it filled with its value as it is, a probe of
/// the index by that value, of the shape the plan made with it has, a
/// decimal of another scale than the first value's not cast to its slot's
/// type; a null's run is planned by it alone, the plan still kept.
#[test]
fn a_subquery_keeps_its_plan_once_its_runs_agree() {
    let st = state(false);
    let runner = Runner::new(&st, NO_TRANSACTION);
    let sq = subquery();
    let (mut was, mut runs) = (Planned::Untried, 0);
    while runs < MOST_RUNS && named(&was) != "kept" {
        assert_eq!(named(&was), if runs == 0 { "untried" } else { "learning" }, "run {runs}");
        let (plan, now) = runner.planned(&sq, &[int(runs)], was);
        assert_eq!(shown(probe(&plan)), shown(Some(&Probe::Compare { op: CompOp::Eq, value: int(runs) })), "{plan:?}");
        (was, runs) = (now, runs + 1);
    }
    assert!(named(&was) == "kept" && runs > 1, "kept after {runs} runs, not the first");
    let custom = PhysicalPlanner::new(&st).plan(&runner.optimized(sq.bound(&[int(7)])));
    let (plan, now) = runner.planned(&sq, &[int(7)], was);
    assert!(same_shape(&plan, &custom), "{plan:?}");
    assert_eq!(shown(probe(&plan)), shown(Some(&Probe::Compare { op: CompOp::Eq, value: int(7) })));
    assert_eq!(named(&now), "kept");
    let custom_null = PhysicalPlanner::new(&st).plan(&runner.optimized(sq.bound(&[Value::Null])));
    let (null, now) = runner.planned(&sq, &[Value::Null], now);
    assert_eq!(format!("{null:?}"), format!("{custom_null:?}"), "a null's run planned by it, not probing for nulls");
    assert_eq!(named(&now), "kept");
    let half = Value::Exact(Decimal::new(25, 1));
    let (scaled, _) = runner.planned(&sq, std::slice::from_ref(&half), now);
    assert_eq!(shown(probe(&scaled)), shown(Some(&Probe::Compare { op: CompOp::Eq, value: half })), "of another scale, not cast");
}

/// Verifies: [DESIGN] RULES.md D675: a plan with slots that is one probe of
/// a whole unique key is kept from the first run; a run whose value is of
/// another kind than its slot's is planned alone, by its value, and the
/// plan stays kept. MC/DC pairs of fitting: a value of the kind fits; a
/// null, or one of another kind, not.
#[test]
fn a_unique_probe_is_kept_at_once_and_a_value_of_another_kind_is_planned_alone() {
    let st = state(true);
    let runner = Runner::new(&st, NO_TRANSACTION);
    let sq = subquery();
    let (_, kept) = runner.planned(&sq, &[int(3)], Planned::Untried);
    assert_eq!(named(&kept), "kept");
    let Planned::Kept(run) = &kept else { panic!("kept") };
    assert!(run.fits(&[int(4)]) && !run.fits(&[Value::Null]));
    assert!(!run.fits(&[Value::Char("4".into())]) && !run.fits(&[]), "another kind, or no value");
    let (plan, still) = runner.planned(&sq, &[Value::Approx(4.0_f64)], kept);
    assert_eq!(named(&still), "kept");
    let custom = PhysicalPlanner::new(&st).plan(&runner.optimized(sq.bound(&[Value::Approx(4.0_f64)])));
    assert_eq!(format!("{:?}", probe(&plan)), format!("{:?}", probe(&custom)), "planned by its value");
}

/// Verifies: [DESIGN] RULES.md D675: a run whose value is null leaves the
/// subquery untried, as a null tells no kind; one of a kind no slot takes,
/// or of a plan holding a slot of its own, has it planned at each run;
/// once planned at each run, it stays so.
#[test]
fn a_null_waits_and_a_kind_no_slot_takes_plans_each_run() {
    let st = state(false);
    let runner = Runner::new(&st, NO_TRANSACTION);
    let sq = subquery();
    assert_eq!(named(&runner.planned(&sq, &[Value::Null], Planned::Untried).1), "untried");
    let array = Value::Array(Arc::from(vec![int(1)]));
    assert_eq!(named(&runner.planned(&sq, &[array], Planned::Untried).1), "varies");
    let slot = Expr::Dynamic(Slot { index: 0, data_type: DataType::Boolean, at: 0 });
    let own = Subquery::new(LogicalPlan::Project { exprs: vec![slot], input: Box::new(sq.plan.clone()) }, vec![Expr::Column(0)]);
    assert_eq!(named(&runner.planned(&own, &[int(1)], Planned::Untried).1), "varies");
    assert_eq!(named(&runner.planned(&sq, &[int(1)], Planned::Varies).1), "varies");
}

/// Verifies: [DESIGN] RULES.md D675: a subquery decorrelated after fewer
/// runs than a plan learns over is planned at each run until then, as its
/// learning would cost more than it saves. MC/DC pairs of waiting for
/// decorrelation: decorrelated soon, planned at each run; one that cannot
/// be decorrelated, its outer reference in a comparison no key takes,
/// learns; decorrelated after many runs, learns (as the tests above).
#[test]
fn a_subquery_decorrelated_soon_is_planned_at_each_run() {
    let st = state_of(false, 8);
    let runner = Runner::new(&st, NO_TRANSACTION);
    assert_eq!(named(&runner.planned(&subquery(), &[int(1)], Planned::Untried).1), "varies");
    let LogicalPlan::Project { exprs, input } = subquery().plan else { panic!("a projection") };
    let LogicalPlan::Filter { input: scan, .. } = *input else { panic!("a filter") };
    let cond = Cond::Compare(Expr::Column(0), CompOp::Ge, Expr::Outer(0, "T.K".into()));
    let ranged = LogicalPlan::Project { exprs, input: Box::new(LogicalPlan::Filter { cond, input: scan }) };
    let ranged = Subquery::new(ranged, vec![Expr::Column(0)]);
    assert_eq!(named(&runner.planned(&ranged, &[int(1)], Planned::Untried).1), "learning");
    let later = Runner::new(&state(false), NO_TRANSACTION);
    assert_eq!(named(&later.planned(&subquery(), &[int(1)], Planned::Untried).1), "learning");
}

/// Verifies: [DESIGN] RULES.md D675: the slot of each kind an index places
/// as it is takes a type of that kind, and of a null or another kind none.
#[test]
fn outer_values_type_their_slots_by_kind() {
    let date = crate::value::datetime::Datetime { moment: Moment::Date, utc: 0, zone: 0, zoned: false, precision: 0 };
    let stamp = crate::value::datetime::Datetime { moment: Moment::Timestamp, precision: 6, ..date };
    let time = crate::value::datetime::Datetime { moment: Moment::Time, precision: 2, zoned: true, ..date };
    assert!(matches!(slot_type(&Value::Char("a".into())), Some(DataType::Varying { .. })));
    assert!(matches!(slot_type(&Value::Exact(Decimal::new(25, 1))), Some(DataType::Exact { scale: 1, .. })));
    assert!(matches!(slot_type(&Value::Approx(1.0_f64)), Some(DataType::Approximate { .. })));
    assert!(matches!(slot_type(&Value::Bool(true)), Some(DataType::Boolean)));
    assert_eq!(slot_type(&Value::Datetime(date)), Some(DataType::Date));
    assert_eq!(slot_type(&Value::Datetime(stamp)), Some(DataType::Timestamp { precision: 6, zoned: false }));
    assert_eq!(slot_type(&Value::Datetime(time)), Some(DataType::Time { precision: 2, zoned: true }));
    assert_eq!(slot_type(&Value::Null), None);
    assert_eq!(slot_type(&Value::Row(Arc::from(vec![int(1)]))), None);
}

/// Verifies: [DESIGN] RULES.md D675: a kept plan fits a later value of its
/// slot's kind whatever its precision, scale or length, as an index places
/// each such value as it is, and no value of another kind: a date's slot
/// takes no timestamp nor time, a timestamp's no date, a number's no
/// string. MC/DC pairs of fitting by kind: a timestamp of another
/// precision fits a timestamp's slot, and one of another kind, a date's,
/// not.
#[test]
fn a_kept_plan_fits_values_of_its_slots_kinds_alone() {
    let date = crate::value::datetime::Datetime { moment: Moment::Date, utc: 0, zone: 0, zoned: false, precision: 0 };
    let stamp = |precision: u8| Value::Datetime(crate::value::datetime::Datetime { moment: Moment::Timestamp, precision, ..date });
    let time = Value::Datetime(crate::value::datetime::Datetime { moment: Moment::Time, ..date });
    let slotted = |first: &Value| Slotted { types: vec![slot_type(first).unwrap()], plan: subquery().plan };
    let of_stamp = slotted(&stamp(0));
    assert!(of_stamp.fits(&[stamp(6)]) && !of_stamp.fits(&[Value::Datetime(date)]) && !of_stamp.fits(std::slice::from_ref(&time)));
    let of_date = slotted(&Value::Datetime(date));
    assert!(of_date.fits(&[Value::Datetime(date)]) && !of_date.fits(&[stamp(0)]) && !of_date.fits(&[time]));
    let of_number = slotted(&int(1));
    assert!(of_number.fits(&[Value::Exact(Decimal::new(25, 1))]) && !of_number.fits(&[Value::Char("1".into())]));
    let of_text = slotted(&Value::Char("ab".into()));
    assert!(of_text.fits(&[Value::Char("abcdef".into())]) && !of_text.fits(&[int(1)]));
}

/// Verifies: [DESIGN] RULES.md D892, D418: outer expressions naming the
/// same slot, column or outer reference take the first's slot, and any
/// other its own; a slot and a column of one number are not the same.
#[test]
fn a_variable_named_twice_is_one_slot() {
    use crate::logical::Slot;
    use crate::value::{DataType, Spelled};
    let slot = |index: usize| {
        Expr::Dynamic(Slot { index, data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, at: 0 })
    };
    let outer =
        [slot(0), slot(1), slot(2), slot(2), Expr::Column(2), Expr::Column(2), Expr::Outer(1, "V".into()), Expr::Outer(1, "V".into())];
    assert_eq!(places(&outer), [0, 1, 2, 2, 4, 4, 6, 6]);
    let constants = [Expr::Const(int(1)), Expr::Const(int(1))];
    assert_eq!(places(&constants), [0, 1], "a constant is no name");
}
