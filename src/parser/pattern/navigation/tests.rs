//! Unit tests of the expressions only row pattern matching reads
//! [ISO/IEC 9075-2:2023 §6.9, §6.10, §6.27, §6.31, §6.33, §10.9], and of
//! the syntax of their words elsewhere, which they leave as it was.

use super::*;
use crate::ast::{CursorSpecification, Literal, QueryExpression, SelectList, SelectSublist, ValueSpecification, WindowOver};

/// The value expression `text` is, parsed whole.
fn parsed(text: &str) -> Result<ValueExpression> {
    let mut p = Parser::new(text)?;
    let value = p.value_expression()?;
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the expression"));
    }
    Ok(value)
}

/// The navigation `text` is, which parses as one.
fn navigated(text: &str) -> Navigation {
    match parsed(text) {
        Ok(ValueExpression::Navigation(n)) => *n,
        other => panic!("{text}: {other:?}"),
    }
}

/// The whole number the offset `o` is a literal of.
fn literal(o: Option<&ValueExpression>) -> Option<i128> {
    let Some(ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(d)))) = o else { return None };
    Some(d.units)
}

/// The name of the column `e` is.
fn column(e: &ValueExpression) -> String {
    let ValueExpression::Column(c) = e else { panic!("not a column: {e:?}") };
    format!("{}.{}", c.qualifier.as_ref().map(|q| q.ident.as_str()).unwrap_or_default(), c.column)
}

/// The cursor specification `sql`, parsed.
fn cursor(sql: &str) -> CursorSpecification {
    Parser::new(sql).and_then(|mut p| p.cursor_specification()).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// The first select item of the query `sql`.
fn first_item(sql: &str) -> ValueExpression {
    let c = cursor(sql);
    let QueryExpression::Spec(spec) = &c.query else { panic!("not a query specification") };
    let SelectList::Values(items) = &spec.select_list else { panic!("no select items") };
    let Some(SelectSublist::Derived(item)) = items.first() else { panic!("no derived column") };
    item.value.clone()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27 SR5a, SR7, SR8] PREV
/// and NEXT alone are a physical step of no logical one, FIRST and LAST
/// alone a logical step of no physical one, RUNNING unless FINAL is
/// written; each offset kept if written, none if not; where each begins.
#[test]
fn each_simple_navigation_6_27() {
    let prev = navigated("PREV(A.X)");
    assert!(prev.logical.is_none() && prev.at == 0);
    assert!(matches!(&prev.physical, Some(Physical { next: false, offset: None })));
    assert_eq!(column(&prev.expr), "A.X");
    let next = navigated("NEXT(A.X + 1, 2)");
    assert!(matches!(next.expr, ValueExpression::Dyadic(..)));
    let p = next.physical.unwrap();
    assert!(p.next && literal(p.offset.as_deref()) == Some(2));
    let first = navigated("FIRST(A.X)");
    assert!(first.physical.is_none());
    assert!(matches!(&first.logical, Some(Logical { final_: false, last: false, offset: None })));
    let last = navigated("FINAL LAST(B.Y, 3)");
    let l = last.logical.unwrap();
    assert!(l.final_ && l.last && literal(l.offset.as_deref()) == Some(3));
    assert!(!navigated("RUNNING FIRST(A.X)").logical.unwrap().final_);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27] the compound form:
/// PREV or NEXT of FIRST or LAST, RUNNING or FINAL, each step's offset its
/// own.
#[test]
fn a_compound_navigation_has_both_steps_6_27() {
    let n = navigated("NEXT(FINAL FIRST(A.X, 1), 2)");
    let (l, p) = (n.logical.unwrap(), n.physical.unwrap());
    assert!(l.final_ && !l.last && literal(l.offset.as_deref()) == Some(1));
    assert!(p.next && literal(p.offset.as_deref()) == Some(2));
    assert_eq!(column(&n.expr), "A.X");
    let prev = navigated("PREV(LAST(A.X))");
    assert!(prev.logical.is_some_and(|step| step.last && !step.final_ && step.offset.is_none()));
    assert!(prev.physical.is_some_and(|step| !step.next && step.offset.is_none()));
    let running = navigated("PREV(RUNNING LAST(A.X, 0), 1)");
    assert!(running.logical.is_some_and(|step| step.last && literal(step.offset.as_deref()) == Some(0)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27] an offset is a
/// `<simple value specification>`, or of a statement being prepared a
/// `<dynamic parameter specification>`; any other expression is a syntax
/// error, and so is a missing ")" or argument.
#[test]
fn an_offset_is_a_simple_value_specification_6_27() {
    let mut p = Parser::for_prepare("PREV(FIRST(A.X, ?), ?)", false).unwrap();
    let ValueExpression::Navigation(n) = p.value_expression().unwrap() else { panic!("not a navigation") };
    assert_eq!(p.dynamic_parameters(), 2);
    assert!(matches!(n.logical.unwrap().offset.as_deref(), Some(ValueExpression::Value(ValueSpecification::Dynamic { index: 0, .. }))));
    for wrong in ["PREV(A.X, B.Y)", "PREV(A.X, 1 + 1)", "PREV(A.X", "FIRST()", "PREV(FIRST(A.X) + 1)", "NEXT(A.X,)"] {
        assert!(parsed(wrong).is_err(), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.14, §7.17, §10.10] NEXT
/// VALUE FOR, NULLS FIRST, NULLS LAST, FETCH FIRST and FETCH NEXT read as
/// they did, and columns named PREV, RUNNING and FINAL are columns.
#[test]
fn next_first_and_last_elsewhere_read_as_they_did_6_14() {
    assert!(matches!(parsed("NEXT VALUE FOR S"), Ok(ValueExpression::NextValue { .. })));
    let c = cursor("SELECT A FROM T ORDER BY A NULLS FIRST, B NULLS LAST FETCH FIRST 2 ROWS ONLY");
    let o = c.query.ordered().unwrap();
    assert!(o.order_by.len() == 2 && o.fetch.is_some());
    assert!(cursor("SELECT A FROM T FETCH NEXT ROW ONLY").query.ordered().is_some_and(|next| next.fetch.is_some()));
    for name in ["PREV", "RUNNING", "FINAL", "CLASSIFIER", "MATCH_NUMBER"] {
        assert!(matches!(parsed(name), Ok(ValueExpression::Column(_))), "{name}");
    }
    assert!(matches!(first_item("SELECT RUNNING FROM (SELECT 1 AS RUNNING FROM T) AS Q"), ValueExpression::Column(_)));
    assert!(matches!(parsed("FIRST"), Err(e) if e.code == Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31, §6.33] CLASSIFIER of a
/// variable or none, and MATCH_NUMBER of none; anything else in its
/// parentheses a syntax error.
#[test]
fn classifier_and_match_number_6_33() {
    assert!(matches!(parsed("CLASSIFIER()"), Ok(ValueExpression::Classifier { variable: None, at: 0 })));
    assert!(matches!(parsed("CLASSIFIER(A)"), Ok(ValueExpression::Classifier { variable: Some(v), .. }) if v == "A"));
    assert!(matches!(parsed("1 + MATCH_NUMBER()"), Ok(ValueExpression::Dyadic(..))));
    assert!(matches!(parsed("MATCH_NUMBER( )"), Ok(ValueExpression::MatchNumber { at: 0 })));
    for wrong in ["MATCH_NUMBER(1)", "CLASSIFIER(A, B)", "CLASSIFIER(1)"] {
        assert!(parsed(wrong).is_err(), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9] COUNT of a variable's
/// `.*` is the `<row pattern count function>`, where the name is. COUNT(*)
/// and COUNT of a column read as they did.
///
/// MC/DC of `"." && "*" && ")"` after the name. The "." is shown by
/// `COUNT(A.*)` against `COUNT(A)`, and the "*" by `COUNT(A.*)` against
/// `COUNT(A.X)`. The ")" is shown by `COUNT(A.*)` against `COUNT(A.*, 1)`,
/// which fails at the "*" (8) as a value expression, not at the "," (9)
/// where the count function would want ")".
#[test]
fn count_of_a_variables_rows_10_9() {
    let Ok(ValueExpression::SetFunction(bare)) = parsed("COUNT(A)") else { panic!("not a set function") };
    assert!(matches!(bare.arg, SetArg::All(_)));
    assert_eq!(parsed("COUNT(A.*, 1)").err().and_then(|e| e.pos), Some(8));
    let Ok(ValueExpression::SetFunction(f)) = parsed("COUNT(A.*)") else { panic!("not a set function") };
    assert!(matches!(&f.arg, SetArg::Variable(v, 6) if v == "A") && f.semantics.is_none());
    let Ok(ValueExpression::SetFunction(star)) = parsed("COUNT(*)") else { panic!("not a set function") };
    assert!(matches!(star.arg, SetArg::CountStar));
    let Ok(ValueExpression::SetFunction(col)) = parsed("COUNT(A.X)") else { panic!("not a set function") };
    assert!(matches!(col.arg, SetArg::All(_)));
    assert!(parsed("SUM(A.*)").is_err() && parsed("COUNT(A.*, 1)").is_err());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.9] RUNNING or FINAL before
/// an aggregate function, the row pattern count function too, is its
/// semantics; before a grouping operation or anything else no aggregate, a
/// syntax error.
#[test]
fn running_or_final_before_an_aggregate_6_9() {
    let semantics = |text: &str| match parsed(text) {
        Ok(ValueExpression::SetFunction(f)) => f.semantics,
        other => panic!("{text}: {other:?}"),
    };
    assert_eq!(semantics("RUNNING SUM(A.X)"), Some(Semantics::Running));
    assert_eq!(semantics("FINAL COUNT(*)"), Some(Semantics::Final));
    assert_eq!(semantics("FINAL COUNT(B.*)"), Some(Semantics::Final));
    assert_eq!(semantics("RUNNING EVERY(A.X > 0)"), Some(Semantics::Running));
    assert_eq!(semantics("AVG(A.X)"), None);
    for wrong in ["RUNNING CAST(A.X AS INT)", "FINAL GROUPING(X)", "RUNNING F(X)", "FINAL RUNNING SUM(X)"] {
        assert!(matches!(parsed(wrong), Err(e) if e.code == Code::Syntax), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10] a name before OVER and
/// "(" or a window name is a `<window row pattern measure>`. OVER as a
/// select item's name reads as it did.
///
/// MC/DC of `is_named(OVER) && ("(" | name)` after the name. The first
/// condition is shown by `M OVER (W)` against `M + (1)`, a sum, its third
/// token "(" too. The second is shown by `M OVER W` against `M OVER FROM`,
/// a column named OVER.
#[test]
fn a_measure_over_a_window_6_10() {
    assert!(matches!(parsed("M + (1)"), Ok(ValueExpression::Dyadic(..))));
    let ValueExpression::Window(w) = first_item("SELECT M OVER W FROM T") else { panic!("not a window function") };
    assert!(matches!(&w.kind, WindowKind::Measure(m) if m == "M"));
    assert!(matches!(&w.over, WindowOver::Name(n, 14) if n == "W") && w.at == 7);
    let ValueExpression::Window(inline) = first_item("SELECT M OVER (W) FROM T") else { panic!("not a window function") };
    assert!(matches!(inline.over, WindowOver::Specification(_)));
    assert!(matches!(first_item("SELECT M OVER FROM T"), ValueExpression::Column(_)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27, §6.31, §6.33] PREV,
/// CLASSIFIER and MATCH_NUMBER begin their expressions only before "(", and
/// another name before "(" begins none of them.
///
/// MC/DC of each `is_word(W) && opens`, W one of PREV, CLASSIFIER and
/// MATCH_NUMBER. `is_word(W)` is shown by `W(...)` against `F(X)`, a
/// function call. `opens` is shown by `W(...)` against W alone, a column.
#[test]
fn each_word_begins_its_expression_only_before_a_parenthesis_6_27() {
    assert!(matches!(parsed("PREV(A.X)"), Ok(ValueExpression::Navigation(_))));
    assert!(matches!(parsed("CLASSIFIER()"), Ok(ValueExpression::Classifier { .. })));
    assert!(matches!(parsed("MATCH_NUMBER()"), Ok(ValueExpression::MatchNumber { .. })));
    assert!(matches!(parsed("F(X)"), Ok(ValueExpression::Call { .. })));
    for name in ["PREV", "CLASSIFIER", "MATCH_NUMBER"] {
        assert!(matches!(parsed(name), Ok(ValueExpression::Column(_))), "{name}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.27] RUNNING or FINAL
/// begins a logical navigation only before FIRST or LAST and "(", inside
/// PREV or NEXT too. RUNNING FIRST before anything else fails where FIRST
/// is.
///
/// MC/DC of `(is_word(RUNNING) || is_word(FINAL)) && FIRST or LAST next &&
/// "(" after`. `is_word(RUNNING)` is shown by `PREV(RUNNING LAST(A.X))`
/// against `PREV(A.X)`, and `is_word(FINAL)` by `PREV(FINAL LAST(A.X))`
/// against `PREV(A.X)`. FIRST or LAST is shown by `RUNNING FIRST(A.X)`
/// against `RUNNING SUM(A.X)`, an aggregate. The "(" is shown by `RUNNING
/// FIRST(A.X)` against `RUNNING FIRST X`, failing at FIRST (8), not at X
/// (14) where a navigation would want "(".
#[test]
fn running_or_final_navigates_only_before_first_or_last_and_a_parenthesis_6_27() {
    assert!(navigated("PREV(RUNNING LAST(A.X))").logical.is_some_and(|l| !l.final_));
    assert!(navigated("PREV(FINAL LAST(A.X))").logical.is_some_and(|l| l.final_));
    assert!(navigated("PREV(A.X)").logical.is_none());
    assert!(navigated("RUNNING FIRST(A.X)").logical.is_some());
    assert!(matches!(parsed("RUNNING SUM(A.X)"), Ok(ValueExpression::SetFunction(_))));
    assert_eq!(parsed("RUNNING FIRST X").err().and_then(|e| e.pos), Some(8));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.9] RUNNING or FINAL is a
/// `<running or final>` only before a name and "(". Before anything else it
/// is a column.
///
/// MC/DC of `named && "(" after`. `named` is shown by `RUNNING SUM(A.X)`
/// against `RUNNING + (1)`, a sum of a column, its third token "(" too. The
/// "(" is shown by `RUNNING SUM(A.X)` against `RUNNING X`, a column RUNNING
/// named X.
#[test]
fn running_or_final_is_a_column_unless_a_name_and_parenthesis_follow_6_9() {
    assert!(matches!(parsed("RUNNING SUM(A.X)"), Ok(ValueExpression::SetFunction(f)) if f.semantics == Some(Semantics::Running)));
    assert!(matches!(parsed("RUNNING + (1)"), Ok(ValueExpression::Dyadic(..))));
    assert!(matches!(first_item("SELECT RUNNING X FROM T"), ValueExpression::Column(c) if c.column == "RUNNING"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9; ISO/IEC 9075:1992
/// §5.4] a variable's name written after an `<introducer>` names the `<row
/// pattern count function>`'s variable too, and an introducer before a
/// string is a literal's. A regression test, failing before the fix, when
/// the introducer hid the "." and "*" after the name and COUNT read them as
/// a value expression and failed.
#[test]
fn count_of_an_introduced_variables_rows_10_9() {
    let Ok(ValueExpression::SetFunction(f)) = parsed("COUNT(_LATIN1 A.*)") else { panic!("not a set function") };
    assert!(matches!(&f.arg, SetArg::Variable(v, 6) if v == "A"));
    let Ok(ValueExpression::SetFunction(literal)) = parsed("COUNT(_LATIN1'X')") else { panic!("not a set function") };
    assert!(matches!(literal.arg, SetArg::All(_)));
}
