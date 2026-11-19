//! Tests of row pattern recognition run: matches, skips, rows per match,
//! unmatched and excluded rows, and the values definitions and measures
//! read of a match [ISO/IEC 9075-2:2023 §7.7, §9.41, §6.27, §10.9].

use super::*;
use crate::catalog::MemoryCatalog;
use crate::error::SqlError;
use crate::functions::Registry;
use crate::functions::{AggregateRef, InverseDistribution};
use crate::logical::{
    AggCall, Cond, Distribution, Expr, Matching, Navigated, PatternAggregate, PatternValue, RowsPerMatch, Skip, Slotted, Variable,
};
use crate::pattern::Pattern;
use crate::spill::testing::{Held, areas};
use crate::txn::NO_TRANSACTION;
use crate::value::types::TypeRegistry;
use crate::value::{CompOp, DataType, Decimal, SortOrder, Spelled};

/// The input's width: a partition number and a price.
const WIDTH: usize = 2;
/// The column of a row's classifier, in a value's expressions.
const CLASSIFIER: usize = WIDTH;
/// The column of the match's number, in a value's expressions.
const MATCH_NUMBER: usize = WIDTH + 1;
/// The price column.
const PRICE: usize = 1;

/// A whole number, as a constant.
fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// The variables of the primaries `names`, then the unions `unions` of
/// them, then the universal one.
fn variables(names: &[&str], unions: &[(&str, &[usize])]) -> Vec<Variable> {
    let mut out: Vec<Variable> = names.iter().enumerate().map(|(n, name)| Variable { name: (*name).into(), components: vec![n] }).collect();
    out.extend(unions.iter().map(|(name, of)| Variable { name: (*name).into(), components: of.to_vec() }));
    out.push(Variable { name: String::new(), components: (0..names.len()).collect() });
    out
}

/// The universal variable of `variables`.
fn universal(variables: &[Variable]) -> usize {
    variables.len().checked_sub(1).unwrap()
}

/// A navigation of `expr` of `variable`: FINAL if `final_`, LAST if
/// `last`, of the logical offset `logical`; NEXT if `next`, of the
/// physical offset `physical`.
fn nav(expr: Expr, variable: usize, (final_, last, logical): (bool, bool, Expr), (next, physical): (bool, Expr)) -> PatternValue {
    PatternValue::Navigated(Box::new(Navigated { expr, variable, final_, last, logical, next, physical }))
}

/// RUNNING LAST (`expr` of `variable`): a plain reference.
fn plain(expr: Expr, variable: usize) -> PatternValue {
    nav(expr, variable, (false, true, int(0)), (false, int(0)))
}

/// FINAL LAST (`expr` of `variable`).
fn last(expr: Expr, variable: usize) -> PatternValue {
    nav(expr, variable, (true, true, int(0)), (false, int(0)))
}

/// The set function `name` of `arg`, COUNT(*) of none, DISTINCT if
/// `distinct`, filtered by `filter`.
fn call(name: &str, arg: Option<Expr>, distinct: bool, filter: Option<Cond>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let arg_type = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
    let result = func.0.result_type(arg.as_ref().map(|_| arg_type)).unwrap();
    AggCall { func, arg, distinct, result, filter, distribution: None, label: name.into() }
}

/// PERCENTILE_CONT if `continuous`, else PERCENTILE_DISC, of 0.5 WITHIN
/// GROUP (ORDER BY price), DESC if `descending`.
fn percentile((continuous, descending): (bool, bool)) -> AggCall {
    let result = DataType::Approximate { precision: crate::value::DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
    let func = AggregateRef(Arc::new(InverseDistribution { continuous, result: result.clone() }));
    let fraction = Expr::Const(Value::Exact(Decimal::new(5, 1)));
    let distribution = Some(Box::new(Distribution { continuous, order: SortOrder::of(descending, None), fraction }));
    AggCall { func, arg: Some(Expr::Column(PRICE)), distinct: false, result, filter: None, distribution, label: "PERCENTILE".into() }
}

/// The set function `call` over `variable`'s rows, FINAL if `final_`.
fn aggregate(call: AggCall, variable: usize, final_: bool) -> PatternValue {
    PatternValue::Aggregate(Box::new(PatternAggregate { call, variable, final_ }))
}

/// The measures of the values `values`, each its own measure.
fn measures(values: Vec<PatternValue>) -> Slotted<Vec<Expr>> {
    Slotted { expr: (0..values.len()).map(Expr::Column).collect(), values }
}

/// The definition of `variable`: its price below the row before's.
fn falling(variable: usize) -> Slotted<Cond> {
    let values = vec![plain(Expr::Column(PRICE), variable), nav(Expr::Column(PRICE), variable, (false, true, int(0)), (false, int(1)))];
    Slotted { expr: Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Column(1)), values }
}

/// `A B+`, B's price falling, B the second of `variables`.
fn dips(variables: Vec<Variable>, measured: Vec<PatternValue>, rows: RowsPerMatch, skip: Skip) -> Recognize<()> {
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let defines = vec![None, Some(falling(1))];
    recognize(Matching { pattern, variables, defines, skip, seek: false }, measured, rows)
}

/// The recognition of `matching`, partitioned by the first column, of
/// the measures `measured`.
fn recognize(matching: Matching, measured: Vec<PatternValue>, rows: RowsPerMatch) -> Recognize<()> {
    let names = (0..measured.len()).map(|n| format!("M{n}")).collect();
    Recognize { partition: vec![0], order: Vec::new(), matching, measures: measures(measured), names, rows, input: () }
}

/// A value as text: a number's digits, a string, `-` of null.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::Exact(d) => d.to_string(),
        Value::Char(s) => s.clone(),
        Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Row(_)
        | Value::Extension(_)
        | Value::Xml(_) => format!("{v:?}"),
    }
}

/// The rows `r` gives of the rows of partition and price `data`, each as
/// text, held in `space`; or the SQLSTATE of its error.
fn given_in(r: Recognize<()>, data: &[(i128, i128)], space: ListSpace) -> std::result::Result<Vec<String>, String> {
    let db = Arc::new(State::with_builtins(Box::new(MemoryCatalog::default())));
    let made: Vec<Result<Row>> =
        data.iter().map(|&(p, x)| Ok(vec![Value::Exact(Decimal::new(p, 0)), Value::Exact(Decimal::new(x, 0))])).collect();
    let runner = Runner::new(&db, NO_TRANSACTION);
    let state = |e: SqlError| e.sqlstate().as_str().to_owned();
    let recognizer = Recognizer::new(r, Box::new(made.into_iter()), (runner, Work::new(&db)), space).map_err(state)?;
    recognizer.map(|row| row.map(|values| values.iter().map(shown).collect::<Vec<_>>().join(" ")).map_err(state)).collect()
}

/// `given_in`, held in memory.
fn given(r: Recognize<()>, data: &[(i128, i128)]) -> std::result::Result<Vec<String>, String> {
    given_in(r, data, ListSpace::default())
}

/// The rows of partition 1 of the prices `prices`.
fn one(prices: &[i128]) -> Vec<(i128, i128)> {
    prices.iter().map(|&p| (1, p)).collect()
}

/// A space holding a few rows before it spills.
fn small() -> ListSpace {
    let few = 256;
    ListSpace {
        limit: few,
        batch: few,
        areas: Some(areas(Some(Arc::new(Held { broken: false })), usize::MAX)),
        types: TypeRegistry::default(),
    }
}

/// The measures MATCH_NUMBER, FINAL LAST (A.price), FINAL LAST (B.price)
/// and COUNT(*), of `vars`.
fn summary(vars: &[Variable]) -> Vec<PatternValue> {
    vec![
        PatternValue::MatchNumber,
        last(Expr::Column(PRICE), 0),
        last(Expr::Column(PRICE), 1),
        aggregate(call("COUNT", None, false, None), universal(vars), true),
    ]
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b ii, GR11a] ONE ROW
/// PER MATCH gives one row of each match, the match's first row then its
/// measures evaluated in its last row; SKIP PAST LAST ROW seeks the next
/// match past the last row, so overlapping matches are not retained.
#[test]
fn one_row_per_match_gives_each_retained_match_7_7() {
    let vars = variables(&["A", "B"], &[]);
    let r = dips(vars.clone(), summary(&vars), RowsPerMatch::One, Skip::PastLastRow);
    assert_eq!(given(r, &one(&[10, 8, 6, 9, 7, 5])), Ok(vec!["1 10 1 10 6 3".into(), "1 9 2 9 5 3".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR11b ii 2, §4.23.2] ALL
/// ROWS PER MATCH gives each row of a match then its measures, running ones
/// read up to the row, final ones of the whole match; CLASSIFIER the row's
/// variable.
#[test]
fn all_rows_per_match_reads_running_and_final_values_4_23() {
    let vars = variables(&["A", "B"], &[]);
    let u = universal(&vars);
    let measured = vec![
        plain(Expr::Column(PRICE), u),
        last(Expr::Column(PRICE), 1),
        aggregate(call("COUNT", None, false, None), u, false),
        aggregate(call("COUNT", None, false, None), u, true),
        plain(Expr::Column(CLASSIFIER), u),
    ];
    let r = dips(vars, measured, RowsPerMatch::AllShowEmpty, Skip::PastLastRow);
    let expected = ["1 10 10 6 1 3 A", "1 8 8 6 2 3 B", "1 6 6 6 3 3 B", "1 9 9 7 1 2 A", "1 7 7 7 2 2 B"];
    assert_eq!(given(r, &one(&[10, 8, 6, 9, 7])), Ok(expected.map(String::from).to_vec()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b i, iii] SKIP TO
/// NEXT ROW seeks from the row after a match's first, so matches overlap;
/// SKIP TO LAST B from B's last row; SKIP TO FIRST B from B's first; each
/// match numbered in turn.
#[test]
fn skips_seek_the_next_match_where_they_say_7_7() {
    let vars = variables(&["A", "B"], &[]);
    let measured = || vec![PatternValue::MatchNumber, aggregate(call("COUNT", None, false, None), 2, true)];
    let data = one(&[10, 8, 6, 9, 7, 5]);
    let run = |skip| given(dips(vars.clone(), measured(), RowsPerMatch::One, skip), &data);
    let overlapping = ["1 10 1 3", "1 8 2 2", "1 9 3 3", "1 7 4 2"].map(String::from).to_vec();
    assert_eq!(run(Skip::NextRow), Ok(overlapping.clone()));
    assert_eq!(run(Skip::Last(1)), Ok(vec!["1 10 1 3".into(), "1 9 2 3".into()]));
    assert_eq!(run(Skip::First(1)), Ok(overlapping));
    assert_eq!(run(Skip::PastLastRow), Ok(vec!["1 10 1 3".into(), "1 9 2 3".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR8b iii 1, 2 A] SKIP TO
/// a variable no row of the match is mapped to is SQLSTATE 2202K; SKIP TO a
/// row that is the match's first 2202L.
#[test]
fn skip_to_no_row_or_the_first_row_raises_7_7() {
    let vars = variables(&["A", "B"], &[]);
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 0, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables: vars.clone(), defines: vec![None, Some(falling(1))], skip: Skip::Last(1), seek: false };
    assert_eq!(given(recognize(matching, Vec::new(), RowsPerMatch::One), &one(&[1, 2])), Err("2202K".into()));
    let first = dips(vars.clone(), Vec::new(), RowsPerMatch::One, Skip::First(0));
    assert_eq!(given(first, &one(&[3, 2])), Err("2202L".into()));
    let last_a = dips(vars, Vec::new(), RowsPerMatch::One, Skip::Last(0));
    assert_eq!(given(last_a, &one(&[3, 2])), Err("2202L".into()));
}

/// `B*` of B falling, its measures MATCH_NUMBER, COUNT(*), LAST(price) and
/// CLASSIFIER, giving `rows` per match.
fn maybe_empty(rows: RowsPerMatch, min: u32) -> Recognize<()> {
    let vars = variables(&["B"], &[]);
    let u = universal(&vars);
    let pattern = Pattern::Repeat { pattern: Box::new(Pattern::Variable(0)), min, max: None, reluctant: false };
    let measured = vec![
        PatternValue::MatchNumber,
        aggregate(call("COUNT", None, false, None), u, false),
        plain(Expr::Column(PRICE), u),
        plain(Expr::Column(CLASSIFIER), u),
    ];
    recognize(Matching { pattern, variables: vars, defines: vec![Some(falling(0))], skip: Skip::PastLastRow, seek: false }, measured, rows)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR11a, GR11b ii 1, NOTE
/// 232, NOTE 233] an empty match is numbered and gives, of ONE ROW PER
/// MATCH and of SHOW EMPTY MATCHES, a row of the row it starts at whose
/// measures see no rows: COUNT 0, a navigation and CLASSIFIER null; of OMIT
/// EMPTY MATCHES none; WITH UNMATCHED ROWS shows it too.
#[test]
fn empty_matches_give_rows_of_no_rows_7_7() {
    let data = one(&[10, 8, 9]);
    let shown = ["1 10 1 0 - -", "1 8 2 1 8 B", "1 9 3 0 - -"].map(String::from).to_vec();
    assert_eq!(
        given(maybe_empty(RowsPerMatch::One, 0), &data),
        Ok(vec!["1 10 1 0 - -".into(), "1 8 2 1 8 B".into(), "1 9 3 0 - -".into()])
    );
    assert_eq!(given(maybe_empty(RowsPerMatch::AllShowEmpty, 0), &data), Ok(shown.clone()));
    assert_eq!(given(maybe_empty(RowsPerMatch::AllOmitEmpty, 0), &data), Ok(vec!["1 8 2 1 8 B".into()]));
    assert_eq!(given(maybe_empty(RowsPerMatch::AllWithUnmatched, 0), &data), Ok(shown));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR11b iii] WITH
/// UNMATCHED ROWS gives each row no retained match maps with null measures,
/// and not a row an earlier match maps though no match starts at it;
/// without it no such row is given. MC/DC of `r.rows == AllWithUnmatched &&
/// k >= self.covered`: `r.rows` (WITH UNMATCHED ROWS vs SHOW EMPTY MATCHES,
/// of the row 10 no match maps) with `k >= covered` true; `k >= covered`
/// (the row 9 past every match vs the row 6 of the match from 8, from which
/// no match is found) with WITH UNMATCHED ROWS.
#[test]
fn unmatched_rows_are_those_no_match_maps_7_7() {
    let data = one(&[10, 8, 9]);
    let unmatched = ["1 10 - - - -", "1 8 1 1 8 B", "1 9 - - - -"].map(String::from).to_vec();
    assert_eq!(given(maybe_empty(RowsPerMatch::AllWithUnmatched, 1), &data), Ok(unmatched));
    assert_eq!(given(maybe_empty(RowsPerMatch::AllShowEmpty, 1), &data), Ok(vec!["1 8 1 1 8 B".into()]));
    let vars = variables(&["A", "B"], &[]);
    let r = dips(vars, vec![PatternValue::MatchNumber], RowsPerMatch::AllWithUnmatched, Skip::NextRow);
    let expected = ["1 10 1", "1 8 1", "1 6 1", "1 8 2", "1 6 2", "1 9 -"].map(String::from).to_vec();
    assert_eq!(given(r, &one(&[10, 8, 6, 9])), Ok(expected));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR1, GR5, §6.31] each
/// partition is matched apart, its matches numbered from 1, and a match
/// does not run into the next partition; a set function's argument reads
/// the match's number. The decision of a row's partitioning value equal to
/// the partition's is true of the rows of one partition and false of the
/// first row of the next.
#[test]
fn partitions_are_matched_and_numbered_apart_6_31() {
    let vars = variables(&["A", "B"], &[]);
    let mut measured = summary(&vars);
    measured.push(aggregate(call("MAX", Some(Expr::Column(MATCH_NUMBER)), false, None), universal(&vars), true));
    let r = dips(vars, measured, RowsPerMatch::One, Skip::PastLastRow);
    let data = [(1, 10), (1, 8), (2, 5), (2, 3), (2, 1), (2, 4), (2, 2)];
    assert_eq!(given(r, &data), Ok(vec!["1 10 1 10 8 2 1".into(), "2 5 1 5 1 3 1".into(), "2 4 2 4 2 2 2".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 GR11] FIRST and LAST
/// pick a row of the variable's rows by the logical offset, null past them;
/// PREV and NEXT move from it by the physical offset within the partition,
/// beyond the match too, null past the partition's ends.
#[test]
fn navigation_moves_by_logical_then_physical_offsets_6_27() {
    let vars = variables(&["A", "B"], &[]);
    let price = || Expr::Column(PRICE);
    let measured = vec![
        nav(price(), 1, (true, false, int(1)), (false, int(0))),
        nav(price(), 1, (true, true, int(1)), (false, int(0))),
        nav(price(), 1, (true, false, int(0)), (false, int(1))),
        nav(price(), 0, (true, true, int(0)), (true, int(2))),
        nav(price(), 1, (true, true, int(0)), (true, int(1))),
        nav(price(), 1, (true, false, int(9)), (false, int(0))),
        nav(price(), 0, (true, false, int(0)), (false, int(1))),
    ];
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables: vars.clone(), defines: vec![None, None], skip: Skip::PastLastRow, seek: false };
    assert_eq!(given(recognize(matching, measured, RowsPerMatch::One), &one(&[1, 2, 3, 4, 5, 6])), Ok(vec!["1 1 3 5 1 3 - - -".into()]));
    let pairs = Pattern::Concat(vec![Pattern::Variable(0), Pattern::Variable(1)]);
    let paired = Matching { pattern: pairs, variables: vars, defines: vec![None, None], skip: Skip::PastLastRow, seek: false };
    let around = vec![nav(price(), 1, (true, true, int(0)), (true, int(1))), nav(price(), 0, (true, true, int(0)), (false, int(1)))];
    let expected = ["1 1 3 -", "1 3 5 2", "1 5 - 4"].map(String::from).to_vec();
    assert_eq!(given(recognize(paired, around, RowsPerMatch::One), &one(&[1, 2, 3, 4, 5, 6])), Ok(expected));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 GR2] a logical or
/// physical offset that is negative or null is SQLSTATE 2202J, even where
/// no row is navigated to.
#[test]
fn negative_or_null_offsets_raise_6_27() {
    let vars = variables(&["A", "B"], &[]);
    let null = Expr::Const(Value::Null);
    for (logical, physical) in [(int(-1), int(0)), (int(0), int(-2)), (null.clone(), int(0)), (int(0), null)] {
        let r = dips(
            vars.clone(),
            vec![nav(Expr::Column(PRICE), 1, (true, true, logical), (false, physical))],
            RowsPerMatch::One,
            Skip::PastLastRow,
        );
        assert_eq!(given(r, &one(&[3, 2])), Err("2202J".into()));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33, §6.27 GR11] CLASSIFIER
/// of a union variable is the variable of the last row mapped to one of its
/// components up to the current row; of the universal one the current
/// row's; PREV of it the row before's, null before the partition's first.
#[test]
fn classifiers_name_the_variables_of_rows_6_33() {
    let vars = variables(&["A", "B", "C"], &[("U", &[0, 2])]);
    let u = universal(&vars);
    let classifier = || Expr::Column(CLASSIFIER);
    let measured = vec![plain(classifier(), 3), plain(classifier(), u), nav(classifier(), u, (false, true, int(0)), (false, int(1)))];
    let pattern = Pattern::Concat(vec![Pattern::Variable(0), Pattern::Variable(1), Pattern::Variable(2)]);
    let matching = Matching { pattern, variables: vars, defines: vec![None, None, None], skip: Skip::PastLastRow, seek: false };
    let expected = ["1 1 A A -", "1 2 A B A", "1 3 C C B"].map(String::from).to_vec();
    assert_eq!(given(recognize(matching, measured, RowsPerMatch::AllShowEmpty), &one(&[1, 2, 3])), Ok(expected));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR1a to c, GR4, GR5,
/// §4.23.2] (R030) a set function over a variable's rows: running ones over
/// those up to the current row, null or 0 of none, final ones of the whole
/// match; DISTINCT counts each value once; FILTER leaves out the rows it is
/// not true of, a filter reading the row's classifier. The filter's
/// decisions, `Some(filter)` then its truth not TRUE, are each taken both
/// ways (`dear` true of 10 and 8, not of 6; SUM unfiltered), as are
/// DISTINCT's, `Some(seen)` then the value not new (COUNT(DISTINCT) of the
/// repeated 8, of a new 6; SUM not DISTINCT).
#[test]
fn aggregates_read_running_and_final_rows_10_9() {
    let vars = variables(&["A", "B"], &[]);
    let u = universal(&vars);
    let price = || Some(Expr::Column(PRICE));
    let dear = Cond::Compare(Expr::Column(PRICE), CompOp::Gt, int(7));
    let measured = vec![
        aggregate(call("SUM", price(), false, None), 1, false),
        aggregate(call("SUM", price(), false, None), 1, true),
        aggregate(call("COUNT", price(), true, None), 1, false),
        aggregate(call("SUM", price(), false, Some(dear)), u, true),
        aggregate(
            call("COUNT", None, false, Some(Cond::Compare(Expr::Column(CLASSIFIER), CompOp::Eq, Expr::Const(Value::Char("B".into()))))),
            u,
            false,
        ),
    ];
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables: vars, defines: vec![None, None], skip: Skip::PastLastRow, seek: false };
    let expected = ["1 10 - 22 0 26 0", "1 8 8 22 1 26 1", "1 6 14 22 2 26 2", "1 8 22 22 2 26 3"].map(String::from).to_vec();
    assert_eq!(given(recognize(matching, measured, RowsPerMatch::AllShowEmpty), &one(&[10, 8, 6, 8])), Ok(expected));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR7, §4.22] a
/// definition reads a running set function over the rows mapped so far with
/// its candidate: B AS COUNT(B.*) <= 2 maps at most two rows to B.
#[test]
fn definitions_read_running_aggregates_9_41() {
    let vars = variables(&["A", "B"], &[]);
    let counted = Slotted {
        expr: Cond::Compare(Expr::Column(0), CompOp::Le, int(2)),
        values: vec![aggregate(call("COUNT", None, false, None), 1, false)],
    };
    let pattern = Pattern::Concat(vec![
        Pattern::Variable(0),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables: vars.clone(), defines: vec![None, Some(counted)], skip: Skip::PastLastRow, seek: false };
    let r = recognize(matching, vec![aggregate(call("COUNT", None, false, None), universal(&vars), true)], RowsPerMatch::One);
    assert_eq!(given(r, &one(&[1, 2, 3, 4, 5])), Ok(vec!["1 1 3".into(), "1 4 2".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR11b ii 2 B I] a row
/// between {- and -} gives no row of ALL ROWS PER MATCH, yet is matched, so
/// WITH UNMATCHED ROWS gives no row of it either.
#[test]
fn excluded_rows_give_no_rows_7_7() {
    let vars = variables(&["A", "B"], &[]);
    let pattern = Pattern::Concat(vec![
        Pattern::Excluded(Box::new(Pattern::Variable(0))),
        Pattern::Repeat { pattern: Box::new(Pattern::Variable(1)), min: 1, max: None, reluctant: false },
    ]);
    let matching = Matching { pattern, variables: vars, defines: vec![None, Some(falling(1))], skip: Skip::PastLastRow, seek: false };
    let r = recognize(matching, vec![PatternValue::MatchNumber], RowsPerMatch::AllWithUnmatched);
    assert_eq!(given(r, &one(&[10, 8, 6])), Ok(vec!["1 8 1".into(), "1 6 1".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 GR5 to GR11]; [DESIGN] a
/// partition whose rows outgrow the space they are held in, spilled, gives
/// the rows it gives held in memory.
#[test]
fn spilled_partitions_give_the_same_rows_7_7() {
    let vars = variables(&["A", "B"], &[]);
    let prices: Vec<i128> = (0_i128..300).map(|n| (n * 37) % 23).collect();
    let data = one(&prices);
    let run = |space| given_in(dips(vars.clone(), summary(&vars), RowsPerMatch::AllShowEmpty, Skip::PastLastRow), &data, space);
    let held = run(ListSpace::default()).unwrap();
    assert!(held.len() > 100, "{}", held.len());
    assert_eq!(run(small()), Ok(held));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10, SR14, §4.23.2]
/// (R030) an inverse distribution function over a variable's rows, running
/// over those up to the current row, null of none, and final over the whole
/// match: PERCENTILE_DISC the first value whose share reaches the fraction,
/// in the order of its sort key, ascending or descending, and
/// PERCENTILE_CONT interpolating. MC/DC of the fraction's `Some(inverse) &&
/// inverse.fraction.is_none()`: `Some(inverse)` (PERCENTILE's first row
/// fed, its fraction computed, vs SUM's, which has none) with no fraction
/// yet; `fraction.is_none()` (the running PERCENTILE's first row fed vs its
/// second, the fraction kept) with an inverse distribution function. Its
/// finishing `Some(Inverse)` is true of PERCENTILE and false of SUM, a
/// running one of no row fed computing its fraction of a row of nulls.
#[test]
fn inverse_distributions_read_running_and_final_rows_10_9() {
    let vars = variables(&["A", "B"], &[]);
    let measured = vec![
        aggregate(percentile((false, false)), 1, false),
        aggregate(percentile((false, false)), 1, true),
        aggregate(percentile((true, false)), 1, false),
        aggregate(percentile((true, false)), 1, true),
        aggregate(percentile((false, true)), 1, true),
        aggregate(call("SUM", Some(Expr::Column(PRICE)), false, None), 1, false),
    ];
    let r = dips(vars, measured, RowsPerMatch::AllShowEmpty, Skip::PastLastRow);
    let [x6, x7, x8] = ["Approx(6.0)", "Approx(7.0)", "Approx(8.0)"];
    let expected = [
        format!("1 10 - {x6} - {x7} {x8} -"),
        format!("1 8 {x8} {x6} {x8} {x7} {x8} 8"),
        format!("1 6 {x6} {x6} {x7} {x7} {x8} 14"),
        format!("1 9 - {x7} - {x7} {x7} -"),
        format!("1 7 {x7} {x7} {x7} {x7} {x7} 7"),
    ];
    assert_eq!(given(r, &one(&[10, 8, 6, 9, 7])), Ok(expected.to_vec()));
}
