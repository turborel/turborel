//! Tests of a window's row pattern recognition: frames narrowed to their
//! designated matches, empty matches and empty frames, skip indicators
//! and SEEK [ISO/IEC 9075-2:2023 §7.15 GR6, §6.10 GR1e].

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::exec::execute;
use crate::functions::Registry;
use crate::logical::{
    AggCall, Bound, Cond, Exclusion, Expr, Frame, Matching, Navigated, PatternAggregate, PatternValue, Pick, Slotted, SortKey, Units,
    Valued, Variable, Window,
};
use crate::pattern::Pattern;
use crate::physical::Plan;
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled};

/// The price column of the input, after its sequence number.
const PRICE: usize = 1;

/// A whole number, as a constant.
fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// A navigation of the price of `variable`: FINAL if `final_`, back
/// `physical` rows from its last row.
fn price(variable: usize, final_: bool, physical: i128) -> PatternValue {
    let n = Navigated { expr: Expr::Column(PRICE), variable, final_, last: true, logical: int(0), next: false, physical: int(physical) };
    PatternValue::Navigated(Box::new(n))
}

/// `A B+`, B's price below the row before's, of `skip`, SEEK if `seek`,
/// its one measure FINAL LAST (B.price).
fn dips(skip: Skip, seek: bool) -> Arc<WindowMatching> {
    let variables = vec![
        Variable { name: "A".into(), components: vec![0] },
        Variable { name: "B".into(), components: vec![1] },
        Variable { name: String::new(), components: vec![0, 1] },
    ];
    let falling =
        Slotted { expr: Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Column(1)), values: vec![price(1, false, 0), price(1, false, 1)] };
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables, defines: vec![None, Some(falling)], skip, seek };
    let measures = Slotted { expr: vec![Expr::Column(0)], values: vec![price(1, true, 0)] };
    Arc::new(WindowMatching { matching, measures, names: vec!["M".into()] })
}

/// `B*`, B's price below the row before's, SEEK if `seek`, its one
/// measure FINAL COUNT(*), 0 of an empty match and null of none.
fn falls(seek: bool) -> Arc<WindowMatching> {
    let variables = vec![Variable { name: "B".into(), components: vec![0] }, Variable { name: String::new(), components: vec![0] }];
    let falling =
        Slotted { expr: Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Column(1)), values: vec![price(0, false, 0), price(0, false, 1)] };
    let pattern = Pattern::Repeat { pattern: Box::new(Pattern::Variable(0)), min: 0, max: None, reluctant: false };
    let matching = Matching { pattern, variables, defines: vec![Some(falling)], skip: Skip::PastLastRow, seek };
    let counted = PatternValue::Aggregate(Box::new(PatternAggregate { call: aggregate("COUNT", None), variable: 1, final_: true }));
    let measures = Slotted { expr: vec![Expr::Column(0)], values: vec![counted] };
    Arc::new(WindowMatching { matching, measures, names: vec!["M".into()] })
}

/// The set function `name` of the price, COUNT(*) if none.
fn aggregate(name: &str, arg: Option<Expr>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let arg_type = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
    let result = func.0.result_type(arg.as_ref().map(|_| arg_type)).unwrap();
    AggCall { func, arg, distinct: false, result, filter: None, distribution: None, label: name.into() }
}

/// The window function of the set function `name` of the price, COUNT(*)
/// if none.
fn call(name: &str, arg: Option<Expr>) -> WindowKind {
    WindowKind::Aggregate(aggregate(name, arg))
}

/// The rows of the prices `prices`, numbered from 1, each with COUNT(*),
/// SUM(price), the measure and LAST_VALUE(price) over the frame ROWS
/// BETWEEN CURRENT ROW AND `end` of `matching`, as text.
fn windowed(prices: &[i128], matching: &Arc<WindowMatching>, end: Bound) -> Vec<String> {
    let frame =
        Frame { units: Units::Rows, start: Bound::CurrentRow, end, exclusion: Exclusion::NoOthers, matching: Some(Arc::clone(matching)) };
    let last = WindowKind::Value(Valued { pick: Pick::Last, arg: Expr::Column(PRICE), ignore_nulls: false });
    let kinds = [call("COUNT", None), call("SUM", Some(Expr::Column(PRICE))), WindowKind::Measure(0), last];
    let calls = kinds.into_iter().map(|kind| WindowCall { kind, frame: frame.clone(), label: String::new(), nested: Vec::new() }).collect();
    let rows = (1_i128..).zip(prices).map(|(n, &p)| vec![int(n), int(p)]).collect();
    let input = Plan::Values { rows, detail: String::new() };
    let window = Window { partition: Vec::new(), order: vec![SortKey::ascending(0)], calls, input };
    let db = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    let shown = |v: &Value| if v.is_null() { "-".to_owned() } else { v.to_string() };
    execute(Plan::Window { window: Box::new(window), detail: String::new() }, &db, NO_TRANSACTION)
        .map(|row| row.unwrap().iter().map(shown).collect::<Vec<_>>().join(" "))
        .collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 D, E II,
/// §6.10 GR1e] each row's frame is narrowed to its designated match, so
/// COUNT, SUM and LAST_VALUE read only its rows, and its measure is
/// evaluated in it; SKIP PAST LAST ROW sets the skip indicators of the
/// match's other rows, whose frames are empty and measures null, as are
/// those of a row of no match.
#[test]
fn frames_are_narrowed_to_designated_matches_7_15() {
    let given = windowed(&[10, 8, 6, 9, 7], &dips(Skip::PastLastRow, false), Bound::UnboundedFollowing);
    let expected = ["1 10 3 24 6 6", "2 8 0 - - -", "3 6 0 - - -", "4 9 2 16 7 7", "5 7 0 - - -"];
    assert_eq!(given, expected.map(String::from).to_vec());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 E I] SKIP TO
/// NEXT ROW sets no skip indicator, so a row inside an earlier match has
/// its own.
#[test]
fn skip_to_next_row_skips_no_row_7_15() {
    let given = windowed(&[10, 8, 6, 9, 7], &dips(Skip::NextRow, false), Bound::UnboundedFollowing);
    let expected = ["1 10 3 24 6 6", "2 8 2 14 6 6", "3 6 0 - - -", "4 9 2 16 7 7", "5 7 0 - - -"];
    assert_eq!(given, expected.map(String::from).to_vec());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 B II, B III]
/// INITIAL takes only a match from the frame's first row; SEEK the first
/// match from any of its rows, the frame narrowed to it, and the rows up to
/// its last skipped.
#[test]
fn seek_finds_a_later_match_initial_does_not_7_15() {
    let prices = [5, 6, 10, 8];
    let seek = ["1 5 2 18 8 8", "2 6 0 - - -", "3 10 0 - - -", "4 8 0 - - -"];
    assert_eq!(windowed(&prices, &dips(Skip::PastLastRow, true), Bound::UnboundedFollowing), seek.map(String::from).to_vec());
    let initial = ["1 5 0 - - -", "2 6 0 - - -", "3 10 2 18 8 8", "4 8 0 - - -"];
    assert_eq!(windowed(&prices, &dips(Skip::PastLastRow, false), Bound::UnboundedFollowing), initial.map(String::from).to_vec());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 A] a match
/// is sought in the row's full frame alone, so it ends at the frame's last
/// row.
#[test]
fn matches_are_sought_within_the_full_frame_7_15() {
    let given = windowed(&[10, 8, 6, 5], &dips(Skip::NextRow, false), Bound::Following(int(1)));
    let expected = ["1 10 2 18 8 8", "2 8 2 14 6 6", "3 6 2 11 5 5", "4 5 0 - - -"];
    assert_eq!(given, expected.map(String::from).to_vec());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 D, §6.10
/// GR1e] an empty designated match narrows the frame to no rows, so COUNT
/// is 0 and SUM and LAST_VALUE null, yet its measure is evaluated in it,
/// COUNT(*) 0 where a row of no match would give null. MC/DC of the
/// narrowing's `designated.filter(len > 0)`: a match of a row (row 2) vs an
/// empty one (rows 1 and 3).
#[test]
fn an_empty_match_narrows_the_frame_to_no_rows_7_15() {
    let given = windowed(&[10, 8, 9], &falls(false), Bound::UnboundedFollowing);
    let expected = ["1 10 0 - 0 -", "2 8 1 8 1 8", "3 9 0 - 0 -"];
    assert_eq!(given, expected.map(String::from).to_vec());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 B, D] a full
/// frame of no rows has no match, of SEEK or not, though `B*` would match
/// empty, so its measure is null, not 0.
#[test]
fn an_empty_frame_has_no_match_7_15() {
    for seek in [false, true] {
        let given = windowed(&[10, 8, 9], &falls(seek), Bound::Preceding(int(1)));
        let expected = ["1 10 0 - - -", "2 8 0 - - -", "3 9 0 - - -"];
        assert_eq!(given, expected.map(String::from).to_vec(), "SEEK {seek}");
    }
}
