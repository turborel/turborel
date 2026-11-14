//! Unit tests of the row pattern recognition clause and the common syntax
//! it shares with a window [ISO/IEC 9075-2:2023 §7.6 to §7.9, §7.15], and
//! of the words of that syntax read as names elsewhere.

use super::*;
use crate::ast::{CursorSpecification, FrameUnits, QueryExpression, RowPattern, ValueExpression, WindowOver, WindowSpecification};

/// The table reference `text` is, parsed whole.
fn reference(text: &str) -> Result<TableReference> {
    let mut p = Parser::new(text)?;
    let r = p.table_reference()?;
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the table reference"));
    }
    Ok(r)
}

/// The row pattern recognition `text` is, which parses as one.
fn recognized(text: &str) -> RecognizedTable {
    match reference(text) {
        Ok(TableReference::Recognized(r)) => *r,
        other => panic!("{text}: {other:?}"),
    }
}

/// The clause of MATCH_RECOGNIZE ( `body` ) after a table T.
fn clause(body: &str) -> RowPatternRecognition {
    recognized(&format!("T MATCH_RECOGNIZE ({body})")).clause
}

/// The SQLCODE MATCH_RECOGNIZE ( `body` ) after a table T fails with.
fn refused(body: &str) -> Option<Code> {
    reference(&format!("T MATCH_RECOGNIZE ({body})")).err().map(|e| e.code)
}

/// The least clause: a pattern of one variable, and its definition.
const LEAST: &str = "PATTERN (A) DEFINE A AS A.X > 0";

/// The window specification of `SUM(X) OVER ( body )`.
fn window(body: &str) -> Result<WindowSpecification> {
    let mut p = Parser::new(&format!("SUM(X) OVER ({body})"))?;
    let value = p.value_expression()?;
    let ValueExpression::Window(w) = value else { return Err(p.syntax("expected a window function")) };
    let WindowOver::Specification(s) = w.over else { return Err(p.syntax("expected a window specification")) };
    Ok(*s)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6] the input's name and
/// columns are its correlation name and columns, the output's after the
/// clause, AS optional, each where it is written; of a table, a derived
/// table, named or not.
#[test]
fn input_and_output_names_and_columns_7_6() {
    let r = recognized("T AS I (A, B) MATCH_RECOGNIZE (PATTERN (A) DEFINE A AS TRUE) AS O (X, Y)");
    let TableReference::Table(t) = &r.input else { panic!("not a table") };
    assert_eq!((t.table.ident.as_str(), t.correlation.as_deref()), ("T", Some("I")));
    assert_eq!(t.columns.as_deref(), Some(&["A".to_owned(), "B".to_owned()][..]));
    assert_eq!((r.correlation.as_deref(), r.columns.as_ref().map(Vec::len), r.at), (Some("O"), Some(2), 14));
    input_and_output_unnamed_7_6();
}

/// The names and columns of a recognition of a table or derived table
/// whose input or output is unnamed.
fn input_and_output_unnamed_7_6() {
    let bare = recognized(&format!("T MATCH_RECOGNIZE ({LEAST})"));
    assert!(bare.correlation.is_none() && bare.columns.is_none());
    assert!(matches!(&bare.input, TableReference::Table(table) if table.correlation.is_none()));
    let named = recognized(&format!("(SELECT X FROM U) Q MATCH_RECOGNIZE ({LEAST}) O"));
    assert!(matches!(&named.input, TableReference::Derived(d) if d.correlation == "Q") && named.correlation.as_deref() == Some("O"));
    let unnamed = recognized(&format!("(SELECT X FROM U) MATCH_RECOGNIZE ({LEAST})"));
    assert!(matches!(&unnamed.input, TableReference::Derived(d) if d.correlation.is_empty() && d.columns.is_none()));
    assert_eq!(reference("(SELECT X FROM U)").err().map(|e| e.code), Some(Code::Syntax));
    assert_eq!(reference(&format!("(SELECT X FROM U) (A) MATCH_RECOGNIZE ({LEAST})")).err().map(|e| e.code), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6] MATCH_RECOGNIZE before
/// "(" is no correlation name, and before anything else it is one. The
/// output joins as any table does, and a query's FROM reads it.
///
/// MC/DC of `at_match_recognize`, `is_word(MATCH_RECOGNIZE) && "(" next`.
/// The first condition is shown by `T MATCH_RECOGNIZE (...)` against `T X
/// (A)`, a correlation name X and its column A. The second is shown by `T
/// MATCH_RECOGNIZE (...)` against `T MATCH_RECOGNIZE`, a correlation name.
/// The condition `at_match_recognize()` that `correlation()` adds is shown
/// by `T MATCH_RECOGNIZE (...)`, read with no correlation name, against `T
/// X`, whose X is one.
#[test]
fn match_recognize_is_no_correlation_name_7_6() {
    assert!(matches!(reference("T MATCH_RECOGNIZE"), Ok(TableReference::Table(t)) if t.correlation.as_deref() == Some("MATCH_RECOGNIZE")));
    let Ok(TableReference::Table(named)) = reference("T X (A)") else { panic!("not a table") };
    assert_eq!((named.correlation.as_deref(), named.columns.as_deref()), (Some("X"), Some(&["A".to_owned()][..])));
    assert!(matches!(reference("T X"), Ok(TableReference::Table(t)) if t.correlation.as_deref() == Some("X")));
    assert!(recognized(&format!("T MATCH_RECOGNIZE ({LEAST})")).correlation.is_none());
    let joined = reference(&format!("T MATCH_RECOGNIZE ({LEAST}) M JOIN U ON TRUE")).unwrap();
    let TableReference::Joined(j) = joined else { panic!("not a join") };
    assert!(matches!(&j.left, TableReference::Recognized(r) if r.correlation.as_deref() == Some("M")));
    let c: CursorSpecification =
        Parser::new(&format!("SELECT * FROM T MATCH_RECOGNIZE ({LEAST}), U")).unwrap().cursor_specification().unwrap();
    let QueryExpression::Spec(spec) = &c.query else { panic!("not a query specification") };
    assert!(matches!(spec.table_expression.from.as_slice(), [TableReference::Recognized(_), TableReference::Table(_)]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7, §7.8] PARTITION BY,
/// ORDER BY and MEASURES, each in order and each optional, a measure's AS
/// and name required; DEFINE required [§7.9].
#[test]
fn partition_order_and_measures_7_7() {
    let c =
        clause("PARTITION BY K, L COLLATE C ORDER BY T DESC MEASURES FIRST(A.X) AS F, MATCH_NUMBER() AS N PATTERN (A) DEFINE A AS TRUE");
    assert_eq!(c.partition.len(), 2);
    assert!(c.partition.get(1).is_some_and(|p| p.collate.is_some()));
    assert!(c.order.len() == 1 && c.order.first().is_some_and(|o| o.descending));
    let names: Vec<&str> = c.measures.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["F", "N"]);
    assert!(matches!(c.measures.first().map(|m| &m.expr), Some(ValueExpression::Navigation(_))));
    assert_eq!(c.measures.get(1).map(|m| m.at), Some(89));
    let least = clause(LEAST);
    assert!(least.partition.is_empty() && least.order.is_empty() && least.measures.is_empty());
    for wrong in
        ["MEASURES A.X PATTERN (A) DEFINE A AS TRUE", "MEASURES A.X AS PATTERN (A) DEFINE A AS TRUE", "PATTERN (A)", "DEFINE A AS TRUE"]
    {
        assert_eq!(refused(wrong), Some(Code::Syntax), "{wrong}");
    }
    assert_eq!(refused(&format!("ORDER BY T PARTITION BY K {LEAST}")), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.7 SR5, SR6] ONE ROW PER
/// MATCH if none is written, SHOW EMPTY MATCHES if ALL ROWS PER MATCH alone
/// is; each form as written.
#[test]
fn each_rows_per_match_7_7() {
    let rows = |written: &str| clause(&format!("{written} {LEAST}")).rows;
    assert_eq!(rows(""), RowsPerMatch::One);
    assert_eq!(rows("ONE ROW PER MATCH"), RowsPerMatch::One);
    assert_eq!(rows("ALL ROWS PER MATCH"), RowsPerMatch::All(EmptyMatches::Show));
    assert_eq!(rows("ALL ROWS PER MATCH SHOW EMPTY MATCHES"), RowsPerMatch::All(EmptyMatches::Show));
    assert_eq!(rows("ALL ROWS PER MATCH OMIT EMPTY MATCHES"), RowsPerMatch::All(EmptyMatches::Omit));
    assert_eq!(rows("ALL ROWS PER MATCH WITH UNMATCHED ROWS"), RowsPerMatch::All(EmptyMatches::Unmatched));
    for wrong in [
        "ONE ROWS PER MATCH",
        "ALL ROW PER MATCH",
        "ALL ROWS PER MATCH SHOW EMPTY",
        "ALL ROWS MATCH",
        "ONE ROW PER MATCH WITH UNMATCHED ROWS",
    ] {
        assert_eq!(refused(&format!("{wrong} {LEAST}")), Some(Code::Syntax), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR22] ALL ROWS PER MATCH
/// WITH UNMATCHED ROWS of a pattern with `{-` `-}` anywhere is SQLCODE
/// -103. Of a pattern without, and the other handlings of one with, it is
/// accepted.
///
/// MC/DC of `rows == All(Unmatched) && pattern.excludes()`. The first
/// condition is shown by WITH UNMATCHED ROWS of an exclusion, refused,
/// against OMIT EMPTY MATCHES of one, accepted. The second is shown by WITH
/// UNMATCHED ROWS of an exclusion against WITH UNMATCHED ROWS of none.
#[test]
fn unmatched_rows_exclude_nothing_7_9() {
    let excluded = "PATTERN (A (B {- C -})*) DEFINE A AS TRUE";
    assert_eq!(refused(&format!("ALL ROWS PER MATCH WITH UNMATCHED ROWS {excluded}")), Some(Code::SyntaxRule));
    assert!(clause(&format!("ALL ROWS PER MATCH OMIT EMPTY MATCHES {excluded}")).common.pattern.excludes());
    assert!(!clause(&format!("ALL ROWS PER MATCH WITH UNMATCHED ROWS {LEAST}")).common.pattern.excludes());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR23] each `<row pattern
/// skip to>`, SKIP TO a variable read as SKIP TO LAST, and AFTER MATCH SKIP
/// PAST LAST ROW if none is written.
#[test]
fn each_skip_to_and_its_default_7_9() {
    let skip = |written: &str| clause(&format!("AFTER MATCH SKIP {written} {LEAST}")).common.skip;
    assert_eq!(clause(LEAST).common.skip, SkipTo::PastLastRow);
    assert_eq!(skip("PAST LAST ROW"), SkipTo::PastLastRow);
    assert_eq!(skip("TO NEXT ROW"), SkipTo::NextRow);
    assert_eq!(skip("TO FIRST A"), SkipTo::First("A".into(), 45));
    assert_eq!(skip("TO LAST A"), SkipTo::Last("A".into(), 44));
    assert_eq!(skip("TO A"), SkipTo::Last("A".into(), 39));
    for wrong in ["TO NEXT", "PAST LAST", "TO", "TO FIRST", "PAST FIRST ROW", "NEXT ROW"] {
        assert_eq!(refused(&format!("AFTER MATCH SKIP {wrong} {LEAST}")), Some(Code::Syntax), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR3a] a `<row pattern
/// recognition clause>` specifies neither INITIAL nor SEEK, else SQLCODE
/// -103.
#[test]
fn match_recognize_has_no_initial_or_seek_7_9() {
    assert_eq!(refused(&format!("INITIAL {LEAST}")), Some(Code::SyntaxRule));
    assert_eq!(refused(&format!("AFTER MATCH SKIP TO NEXT ROW SEEK {LEAST}")), Some(Code::SyntaxRule));
    assert!(clause(LEAST).common.seek.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9] SUBSET's items, each a
/// name and its variables in order, and DEFINE's definitions, each a name
/// and its search condition, where each begins.
#[test]
fn subsets_and_definitions_7_9() {
    let c = clause("PATTERN (A B+ C) SUBSET U = (A, B), V = (C) DEFINE B AS B.X > PREV(B.X), C AS C.X < A.X");
    let subsets: Vec<(&str, usize)> = c.common.subsets.iter().map(|s| (s.name.as_str(), s.variables.len())).collect();
    assert_eq!(subsets, [("U", 2), ("V", 1)]);
    assert_eq!(c.common.subsets.first().map(|s| s.at), Some(43));
    let defined: Vec<&str> = c.common.defines.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(defined, ["B", "C"]);
    assert!(matches!(&c.common.pattern, RowPattern::Concat(l) if l.len() == 3));
    assert_eq!(c.common.at, 19);
    for wrong in ["PATTERN (A) SUBSET U = A DEFINE A AS TRUE", "PATTERN (A) SUBSET U (A) DEFINE A AS TRUE", "PATTERN (A) DEFINE A TRUE"] {
        assert_eq!(refused(wrong), Some(Code::Syntax), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15] a window frame's
/// measures before its units and its common syntax after its exclusion,
/// INITIAL or SEEK as written, none if neither. A frame of neither has no
/// recognition, and a window of no frame and no measures has neither.
///
/// MC/DC of `window_frame_clause`'s `measures.is_empty() &&
/// at_frame_units().is_none()`. The first condition is shown by `ORDER BY
/// T` against measures and no units (in the next test), which go on to read
/// a frame and fail. The second is shown by `ORDER BY T` against `ORDER BY
/// T ROWS UNBOUNDED PRECEDING`.
#[test]
fn a_window_frame_that_recognizes_7_15() {
    let w = window("PARTITION BY K ORDER BY T MEASURES LAST(A.X) AS L ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING EXCLUDE NO OTHERS AFTER MATCH SKIP TO NEXT ROW SEEK PATTERN (A+) DEFINE A AS TRUE").unwrap();
    assert!(w.frame.as_ref().is_some_and(|f| f.units == FrameUnits::Rows));
    let seeking = w.recognition.unwrap();
    assert!(seeking.measures.len() == 1 && seeking.common.seek == Some(true) && seeking.common.skip == SkipTo::NextRow);
    let plain = window("ROWS BETWEEN CURRENT ROW AND 2 FOLLOWING INITIAL PATTERN (A) DEFINE A AS TRUE").unwrap();
    let initial = plain.recognition.unwrap();
    assert!(initial.measures.is_empty() && initial.common.seek == Some(false));
    let unsaid = window(&format!("ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING {LEAST}")).unwrap();
    assert!(unsaid.recognition.is_some_and(|implied| implied.common.seek.is_none()));
    let framed = window("ORDER BY T ROWS UNBOUNDED PRECEDING").unwrap();
    assert!(framed.frame.is_some() && framed.recognition.is_none());
    let unframed = window("ORDER BY T").unwrap();
    assert!(unframed.frame.is_none() && unframed.recognition.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 SR13] a window's
/// measures need its common syntax, else SQLCODE -103. Measures without a
/// frame are a syntax error, and a window named MEASURES is still a window
/// name.
///
/// MC/DC of the condition `at_window_measures()` that
/// `at_existing_window_name` adds. It is shown by `MEASURES A.X AS M ROWS
/// CURRENT ROW`, measures, against `MEASURES ROWS CURRENT ROW`, a window
/// name, the other conditions false in both.
#[test]
fn a_windows_measures_need_its_common_syntax_7_15() {
    assert_eq!(window("MEASURES A.X AS M ROWS CURRENT ROW").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(window("MEASURES A.X AS M PATTERN (A) DEFINE A AS TRUE").err().map(|e| e.code), Some(Code::Syntax));
    for named in ["MEASURES", "MEASURES ORDER BY T", "MEASURES ROWS CURRENT ROW", "MEASURES PARTITION BY K"] {
        let w = window(named).unwrap_or_else(|e| panic!("{named}: {e:?}"));
        assert!(w.existing.is_some_and(|(n, _)| n == "MEASURES") && w.recognition.is_none(), "{named}");
    }
}

/// The SQLCODE and position `SUM(X) OVER ( body )` fails with.
fn window_refused(body: &str) -> Option<(Code, Option<usize>)> {
    window(body).err().map(|e| (e.code, e.pos))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15] a window frame's
/// common syntax begins at AFTER MATCH, at INITIAL or SEEK before PATTERN,
/// or at PATTERN before "(". Any of those words before anything else is
/// read after the frame as no common syntax, so the window fails where the
/// word is.
///
/// MC/DC of `at_row_pattern_common_syntax`. Each of `after`, `initial` and
/// `pattern` alone makes it true, by AFTER MATCH, INITIAL and SEEK, and
/// PATTERN first, against a frame alone where all three are false. Within
/// `after`, `next_is_kw(MATCH)` is shown by `AFTER MATCH` against AFTER at
/// the end. Within `initial`, `is_word(INITIAL)` and `is_word(SEEK)` are
/// each shown by `INITIAL PATTERN` and `SEEK PATTERN` against the frame
/// alone, and `is_named(PATTERN)` by `SEEK PATTERN` against SEEK at the
/// end. Within `pattern`, `"(" next` is shown by `PATTERN (A)` against
/// PATTERN at the end. A word at the end fails at itself (30), not at the
/// ")" after it, where the common syntax would fail.
#[test]
fn where_the_common_syntax_begins_7_15() {
    let frame = "ROWS CURRENT ROW";
    let common = |written: &str| window(&format!("{frame} {written}")).unwrap().recognition.map(|r| r.common);
    assert!(common("AFTER MATCH SKIP TO NEXT ROW PATTERN (A) DEFINE A AS TRUE").is_some_and(|c| c.skip == SkipTo::NextRow));
    assert!(common("INITIAL PATTERN (A) DEFINE A AS TRUE").is_some_and(|c| c.seek == Some(false)));
    assert!(common("SEEK PATTERN (A) DEFINE A AS TRUE").is_some_and(|c| c.seek == Some(true)));
    assert!(common(LEAST).is_some_and(|c| c.seek.is_none()));
    assert!(common("").is_none());
    for word in ["AFTER", "SEEK", "INITIAL", "PATTERN"] {
        assert_eq!(window_refused(&format!("{frame} {word}")), Some((Code::Syntax, Some(30))), "{word}");
    }
}
