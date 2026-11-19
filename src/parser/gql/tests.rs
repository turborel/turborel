//! Unit tests of GQL's reading (RULES.md D144): each statement read into
//! its clauses, the session and transaction statements, and the
//! statements of graph types and graphs SQL text holds.

use super::*;
use crate::ast::{DropObject, GqlCall, GqlClause, GqlRemoveItem, GqlSetItem, GraphTyping, SchemaStatement, SqlStatement};

/// The request `text` reads as.
fn request(text: &str) -> Result<GqlRequest> {
    Parser::new(text)?.gql_request()
}

/// The procedure `text` reads as.
fn procedure(text: &str) -> GqlProcedure {
    match request(text).unwrap_or_else(|e| panic!("{text}: {e:?}")) {
        GqlRequest::Procedure(p) => p,
        GqlRequest::Session(_) | GqlRequest::Transaction(_) => panic!("{text} is no procedure"),
    }
}

/// The clauses of the first linear statement of `text`.
fn clauses(text: &str) -> Vec<GqlClause> {
    procedure(text).statements.into_iter().next().map(|c| c.first.clauses).unwrap_or_default()
}

/// Verifies: [EXTENSION] RULES.md D144: MATCH, OPTIONAL MATCH, FILTER,
/// WHERE, LET and FOR read as their clauses, in order.
#[test]
fn reading_clauses() {
    let read = clauses(
        "MATCH (a)-[e]->(b) OPTIONAL MATCH (b)->(c) FILTER a.x > 1 WHERE b.y = 2 LET z = 1, w = 2 FOR i IN ARRAY[1] WITH OFFSET k RETURN *",
    );
    assert_eq!(read.len(), 6);
    assert!(matches!(&read[0], GqlClause::Match { optional: false, .. }));
    assert!(matches!(&read[1], GqlClause::Match { optional: true, .. }));
    assert!(matches!(&read[2], GqlClause::Filter(_)));
    assert!(matches!(&read[3], GqlClause::Filter(_)));
    let GqlClause::Let(lets) = &read[4] else { panic!("{:?}", read[4]) };
    assert_eq!(lets.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Z", "W"]);
    let GqlClause::For { variable, place, .. } = &read[5] else { panic!("{:?}", read[5]) };
    assert_eq!(variable, "I");
    assert_eq!(place, &Some(("K".to_owned(), true)));
}

/// Verifies: [EXTENSION] RULES.md D144: RETURN with DISTINCT, items,
/// GROUP BY, ORDER BY, OFFSET or SKIP, and LIMIT.
#[test]
fn reading_return() {
    let p = procedure("MATCH (a) RETURN DISTINCT a.x AS x, COUNT(*) AS n GROUP BY x ORDER BY n DESC SKIP 1 LIMIT 2");
    let first = &p.statements[0].first;
    let GqlResult::Return(r) = &first.result else { panic!("{:?}", first.result) };
    assert!(r.distinct);
    assert_eq!(r.items.as_ref().map(Vec::len), Some(2));
    assert_eq!(r.group_by.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["X"]);
    assert!(r.page.order.first().is_some_and(|s| s.descending));
    assert!(r.page.offset.is_some() && r.page.limit.is_some());
    assert!(matches!(procedure("MATCH (a) FINISH").statements[0].first.result, GqlResult::Finish));
}

/// Verifies: [EXTENSION] RULES.md D144: NEXT chains statements, and
/// UNION, EXCEPT, INTERSECT and OTHERWISE combine them.
#[test]
fn reading_next_and_set_operators() {
    let p = procedure("MATCH (a) RETURN a NEXT RETURN 1 AS x UNION ALL RETURN 2 AS x EXCEPT RETURN 3 AS x OTHERWISE RETURN 4 AS x");
    assert_eq!(p.statements.len(), 2);
    let ops: Vec<GqlSetOperator> = p.statements[1].rest.iter().map(|(op, _)| *op).collect();
    assert_eq!(ops, [GqlSetOperator::Union(true), GqlSetOperator::Except(false), GqlSetOperator::Otherwise]);
    let i = procedure("RETURN 1 AS x INTERSECT RETURN 1 AS x");
    assert_eq!(i.statements[0].rest[0].0, GqlSetOperator::Intersect(false));
}

/// Verifies: [EXTENSION] RULES.md D144: USE names a statement's graph,
/// and CALL is inline, with its scope, or named, with YIELD.
#[test]
fn reading_use_and_call() {
    let p = procedure("USE S.G MATCH (a) CALL (a) { RETURN 1 AS x } OPTIONAL CALL P(1) YIELD c AS d RETURN *");
    let first = &p.statements[0].first;
    assert_eq!(first.graph.as_ref().map(|g| g.ident.as_str()), Some("G"));
    let GqlClause::Call(GqlCall::Inline { optional: false, scope: Some(scope), .. }) = &first.clauses[1] else {
        panic!("{:?}", first.clauses[1])
    };
    assert_eq!(scope, &["A".to_owned()]);
    let GqlClause::Call(GqlCall::Named { optional: true, yields, arguments, .. }) = &first.clauses[2] else {
        panic!("{:?}", first.clauses[2])
    };
    assert_eq!(arguments.len(), 1);
    assert_eq!(yields, &[("C".to_owned(), "D".to_owned())]);
}

/// Verifies: [EXTENSION] RULES.md D144: INSERT, SET, REMOVE, DELETE and
/// DETACH DELETE read as their statements.
#[test]
fn reading_data_modifying_statements() {
    let read = clauses("MATCH (a), (b) INSERT (a)-[:R {w: 1}]->(:N) SET a.x = 1, a:L, b = {y: 2} REMOVE a.x, a:L DETACH DELETE a, b");
    let GqlClause::Insert(paths) = &read[1] else { panic!("{:?}", read[1]) };
    assert_eq!(paths.len(), 1);
    let GqlClause::Set(items) = &read[2] else { panic!("{:?}", read[2]) };
    let [GqlSetItem::Property { property, .. }, GqlSetItem::Label { label, .. }, GqlSetItem::All { properties, .. }] = items.as_slice()
    else {
        panic!("{items:?}")
    };
    assert_eq!((property.as_str(), label.as_str(), properties.len()), ("X", "L", 1));
    let GqlClause::Remove(removed) = &read[3] else { panic!("{:?}", read[3]) };
    assert_eq!(removed[0], GqlRemoveItem::Property("A".to_owned(), "X".to_owned(), removed_at(&removed[0])));
    assert_eq!(removed[1], GqlRemoveItem::Label("A".to_owned(), "L".to_owned(), removed_at(&removed[1])));
    let GqlClause::Delete { detach: true, items: deleted } = &read[4] else { panic!("{:?}", read[4]) };
    assert_eq!(deleted.len(), 2);
    assert!(matches!(clauses("MATCH (a) NODETACH DELETE a").get(1), Some(GqlClause::Delete { detach: false, .. })));
}

/// Where remove item `r` was written.
fn removed_at(r: &GqlRemoveItem) -> usize {
    match r {
        GqlRemoveItem::Property(_, _, at) | GqlRemoveItem::Label(_, _, at) => *at,
    }
}

/// Verifies: [EXTENSION] RULES.md D144: the session and transaction
/// statements.
#[test]
fn reading_session_and_transaction_statements() {
    let session = |text: &str| match request(text) {
        Ok(GqlRequest::Session(s)) => format!("{s:?}"),
        other => panic!("{text}: {other:?}"),
    };
    assert_eq!(session("SESSION SET GRAPH S.G").split('(').next(), Some("SetGraph"));
    assert_eq!(session("SESSION SET SCHEMA S"), "SetSchema(\"S\")");
    assert_eq!(session("SESSION SET TIME ZONE '+01:00'").split('(').next(), Some("SetTimeZone"));
    assert_eq!(session("SESSION RESET GRAPH"), "Reset(Some(Graph))");
    assert_eq!(session("SESSION RESET"), "Reset(None)");
    assert_eq!(session("SESSION CLOSE"), "Close");
    let transaction = |text: &str| match request(text) {
        Ok(GqlRequest::Transaction(t)) => format!("{t:?}"),
        other => panic!("{text}: {other:?}"),
    };
    assert_eq!(transaction("START TRANSACTION READ ONLY"), "Start(Some(true))");
    assert_eq!(transaction("COMMIT"), "Commit");
    assert_eq!(transaction("ROLLBACK"), "Rollback");
}

/// Verifies: [EXTENSION] RULES.md D144: what is no GQL is refused with
/// SQLCODE -102.
#[test]
fn refusing_what_is_no_gql() {
    for text in ["MATCH", "MATCH (a) RETURN", "RETURN 1 AS x UNION", "MATCH (a) LET = 1 RETURN a", "SESSION SET"] {
        let e = request(text).err().unwrap_or_else(|| panic!("{text} was read"));
        assert_eq!(e.sqlcode(), -102_i32, "{text}: {e:?}");
    }
}

/// The SQL statement `text` reads as.
fn sql(text: &str) -> SqlStatement {
    Parser::new(text).and_then(|mut p| p.sql_statement()).unwrap_or_else(|e| panic!("{text}: {e:?}"))
}

/// Verifies: [EXTENSION] graph standard §4.3, §9: EXECUTE GQL, CREATE
/// and DROP GRAPH TYPE, and CREATE and DROP GRAPH, typed and open.
#[test]
fn reading_sql_statements_of_gql() {
    let SqlStatement::ExecuteGql(crate::ast::ExecuteGql { graph, text, .. }) = sql("EXECUTE GQL ON GRAPH G 'MATCH (a) RETURN a'") else {
        panic!()
    };
    assert_eq!(graph.map(|g| g.ident), Some("G".to_owned()));
    assert_eq!(text, "MATCH (a) RETURN a");
    let SqlStatement::Schema { statement: SchemaStatement::GraphType(t), .. } =
        sql("CREATE GRAPH TYPE T { NODE P {NAME STRING NOT NULL}, EDGE K CONNECTING (P -> P) }")
    else {
        panic!()
    };
    assert_eq!((t.nodes.len(), t.edges.len()), (1, 1));
    assert!(t.nodes[0].properties[0].not_null);
    let SqlStatement::Schema { statement: SchemaStatement::GqlGraph(g), .. } = sql("CREATE GRAPH IF NOT EXISTS G TYPED T") else {
        panic!()
    };
    assert!(g.if_not_exists && matches!(g.typing, GraphTyping::Typed(_)));
    let SqlStatement::Schema { statement: SchemaStatement::GqlGraph(o), .. } = sql("CREATE GRAPH H ANY") else { panic!() };
    assert!(matches!(o.typing, GraphTyping::Open));
    assert!(matches!(sql("DROP GRAPH TYPE IF EXISTS T"), SqlStatement::Drop { object: DropObject::GraphType(_, true), .. }));
    assert!(matches!(sql("DROP GRAPH G"), SqlStatement::Drop { object: DropObject::GqlGraph(_, false), .. }));
    assert!(matches!(sql("DROP PROPERTY GRAPH G"), SqlStatement::Drop { object: DropObject::Graph(..), .. }));
}
