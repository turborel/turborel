use super::*;
use crate::ast::{CursorSpecification, Literal, ValueExpression, ValueSpecification};
use crate::error::Code;

/// The cursor specification `sql`, parsed.
fn parsed(sql: &str) -> Result<CursorSpecification> {
    Parser::new(sql)?.cursor_specification()
}

/// The ORDER BY, OFFSET and FETCH FIRST of the query `sql`.
fn ordered(sql: &str) -> OrderedQuery {
    let c = parsed(sql).unwrap();
    c.query.ordered().cloned().unwrap_or_else(|| panic!("no ORDER BY, OFFSET or FETCH: {sql}"))
}

/// The whole number a count is a literal of.
fn literal(c: &RowCount) -> i128 {
    let ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(d))) = &c.value else { panic!("not a literal: {c:?}") };
    d.units
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17] each clause alone and all three:
/// OFFSET of ROW or ROWS, FETCH FIRST or NEXT, of a count or none, ONLY,
/// WITH TIES or PERCENT, each where it is written.
#[test]
fn each_clause_after_a_query_expression_body_7_17() {
    let o = ordered("SELECT A FROM T ORDER BY A OFFSET 2 ROWS FETCH FIRST 3 ROWS ONLY");
    assert_eq!(o.order_by.len(), 1);
    assert_eq!(o.offset.as_ref().map(literal), Some(2));
    let f = o.fetch.unwrap();
    assert!(matches!(&f.quantity, Some(FetchQuantity::Rows(c)) if literal(c) == 3) && !f.ties);
    assert_eq!(f.at, 41);
    let offset_alone = ordered("SELECT A FROM T OFFSET 1 ROW");
    assert!(offset_alone.order_by.is_empty() && offset_alone.fetch.is_none() && offset_alone.offset.is_some());
    let no_count = ordered("SELECT A FROM T FETCH NEXT ROW ONLY").fetch.unwrap();
    assert!(no_count.quantity.is_none(), "GR7b iii, a count of 1");
    let percent = ordered("SELECT A FROM T ORDER BY A FETCH FIRST 10 PERCENT ROWS WITH TIES").fetch.unwrap();
    assert!(matches!(&percent.quantity, Some(FetchQuantity::Percent(c)) if literal(c) == 10) && percent.ties);
    assert!(parsed("SELECT A FROM T").unwrap().query.ordered().is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17, §6.4] a count is a literal,
/// signed too, a parameter name, or [EXTENSION] a `?`; anything else, a
/// syntax error; the clauses in their order only.
#[test]
fn a_count_is_a_simple_value_specification_6_4() {
    assert_eq!(ordered("SELECT A FROM T OFFSET -1 ROWS").offset.as_ref().map(literal), Some(-1));
    let mut p = Parser::for_prepare("SELECT A FROM T OFFSET ? ROWS FETCH FIRST ? ROWS ONLY", false).unwrap();
    let c = p.cursor_specification().unwrap();
    assert_eq!(p.dynamic_parameters(), 2);
    let o = c.query.ordered().unwrap();
    assert!(matches!(&o.offset, Some(RowCount { value: ValueExpression::Value(ValueSpecification::Dynamic { index: 0, .. }), .. })));
    for wrong in [
        "SELECT A FROM T OFFSET A + 1 ROWS",
        "SELECT A FROM T FETCH FIRST 1 ROWS",
        "SELECT A FROM T FETCH 1 ROWS ONLY",
        "SELECT A FROM T FETCH FIRST ROWS WITH",
        "SELECT A FROM T FETCH FIRST 1 ROW ONLY OFFSET 1 ROW",
        "SELECT A FROM T FETCH FIRST 1 ROW ONLY ORDER BY A",
    ] {
        assert_eq!(parsed(wrong).err().map(|e| e.code), Some(Code::Syntax), "{wrong}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17] the clauses of a query primary in
/// parentheses, of a subquery and of a WITH clause's body; OFFSET, which
/// SQL-92 does not reserve, a correlation name or a column's name where no
/// count and ROW or ROWS follow it.
#[test]
fn the_clauses_where_a_query_expression_is_7_17() {
    let o = ordered("(SELECT A FROM T ORDER BY A FETCH FIRST 1 ROW ONLY) UNION (SELECT A FROM T OFFSET 1 ROW) ORDER BY 1");
    let QueryExpression::SetOp(s) = &o.body else { panic!("not a union") };
    assert!(s.left.ordered().is_some_and(|l| l.fetch.is_some()) && s.right.ordered().is_some_and(|r| r.offset.is_some()));
    let with = parsed("WITH Q AS (SELECT A FROM T FETCH FIRST 2 ROWS ONLY) SELECT A FROM Q ORDER BY A").unwrap();
    let QueryExpression::With(w) = &with.query else { panic!("no WITH") };
    assert!(w.elements.first().is_some_and(|e| e.query.ordered().is_some()));
    assert!(with.query.ordered().is_some_and(|b| b.order_by.len() == 1), "of the body, in the WITH's scope");
    let named = parsed("SELECT OFFSET FROM T OFFSET, U OFFSET 1 ROWS").unwrap();
    let offset = named.query.ordered().unwrap();
    let QueryExpression::Spec(spec) = &offset.body else { panic!("not a query specification") };
    assert_eq!(spec.table_expression.from.len(), 2);
    let correlations: Vec<Option<&str>> =
        spec.table_expression.from.iter().map(|r| r.named().and_then(|t| t.correlation.as_deref())).collect();
    assert_eq!(correlations, vec![Some("OFFSET"), None]);
    assert_eq!(literal(offset.offset.as_ref().unwrap()), 1);
}
