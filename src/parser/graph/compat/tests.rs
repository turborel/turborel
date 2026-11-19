//! Unit tests of the compatibility syntax [GS §10]: each front end's
//! statements read, their GPML written back by the graph tests' `shape`.

use super::super::tests::shape;
use crate::ast::{
    GraphColumn, GraphPattern, GraphTable, MatchClause, PathAlgorithm, Predicate, QueryExpression, QuerySpecification, SearchCondition,
    SelectList, SelectSublist, TableReference, ValueExpression,
};
use crate::error::Result;
use crate::lexer::Tok;
use crate::parser::Parser;

/// The query `text` reads as, all of it.
fn query(text: &str) -> Result<QueryExpression> {
    let mut p = Parser::new(text)?;
    let q = p.query_expression()?;
    if *p.peek() != Tok::End {
        return Err(p.syntax("expected the end of the query"));
    }
    Ok(q)
}

/// The query specification `text` reads as, ORDER BY and its counts
/// aside.
fn spec(text: &str) -> QuerySpecification {
    let mut q = query(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
    if let QueryExpression::Ordered(o) = q {
        q = o.body;
    }
    let QueryExpression::Spec(s) = q else { panic!("{text}: {q:?}") };
    *s
}

/// The SQLCODE `text` is refused with.
fn refused(text: &str) -> i32 {
    query(text).map(|_| panic!("{text}: accepted")).unwrap_err().sqlcode()
}

/// The MATCH of `text`'s WHERE clause, its first conjunct.
fn graph_match(text: &str) -> MatchClause {
    let s = spec(text);
    let mut c = s.table_expression.where_clause.unwrap_or_else(|| panic!("{text}: no WHERE"));
    while let SearchCondition::And(a, _) = c {
        c = *a;
    }
    let SearchCondition::Predicate(p, _) = c else { panic!("{text}: {c:?}") };
    let Predicate::GraphMatch(m) = *p else { panic!("{text}: {p:?}") };
    *m
}

/// The shape of MATCH `m`'s path patterns.
fn shaped(m: &MatchClause) -> String {
    shape(&GraphPattern { mode: None, paths: m.paths.clone(), keep: None, where_clause: None, segments: Vec::new(), at: 0 })
}

/// The GRAPH_TABLE of the one-table query `text`.
fn graph_table(text: &str) -> GraphTable {
    let s = spec(text);
    let first = s.table_expression.from.into_iter().next();
    let Some(TableReference::Graph(t)) = first else { panic!("{text}: {first:?}") };
    *t
}

/// The names of `t`'s columns.
fn column_names(t: &GraphTable) -> Vec<String> {
    t.columns
        .iter()
        .map(|c| if let GraphColumn::Value { name, .. } = c { name.clone().unwrap_or_default() } else { format!("{c:?}") })
        .collect()
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn server_match_is_a_path_pattern() {
    let m1 = graph_match("SELECT p2.name FROM person p1, friend_of f, person p2 WHERE MATCH(p1-(f)->p2) AND p1.name = 'Alice'");
    assert_eq!(shaped(&m1), "(P1)-[F]->(P2)");
    let m2 = graph_match("SELECT 1 FROM a, e, b, e2, c WHERE MATCH(a-(e)->b<-(e2)-c AND b-(e)->a)");
    assert_eq!(shaped(&m2), "(A)-[E]->(B)<-[E2]-(C), (B)-[E]->(A)");
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn shortest_path_is_any_shortest_with_its_last_node() {
    let text = "SELECT p1.name, STRING_AGG(p2.name, '->') WITHIN GROUP (GRAPH PATH) AS route, \
                LAST_VALUE(p2.name) WITHIN GROUP (GRAPH PATH) AS last_name, COUNT(p2.id) WITHIN GROUP (GRAPH PATH) AS hops \
                FROM person p1, friend_of FOR PATH f, person FOR PATH p2 WHERE MATCH(SHORTEST_PATH(p1(-(f)->p2)+))";
    let m = graph_match(text);
    assert_eq!(shaped(&m), "ANY SHORTEST (P1)(-[F]->(P2))+(LAST_NODE(P2))");
    assert_eq!(m.path_tables.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["F", "P2"]);
    assert_eq!(m.measures.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["$PATH1", "$PATH2", "$PATH3"]);
    let s = spec(text);
    let SelectList::Values(items) = s.select_list else { panic!("{text}") };
    let Some(SelectSublist::Derived(route)) = items.get(1) else { panic!("{text}") };
    assert!(matches!(&route.value, ValueExpression::Column(c) if c.column == "$PATH1"), "{route:?}");
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn bounded_and_backward_groups() {
    let m1 = graph_match("SELECT 1 FROM p WHERE MATCH(SHORTEST_PATH(p(-(f)->q){1,3}))");
    assert_eq!(shaped(&m1), "ANY SHORTEST (P)(-[F]->(Q)){1,3}(LAST_NODE(Q))");
    let m2 = graph_match("SELECT 1 FROM p WHERE MATCH(SHORTEST_PATH((q<-(f)-)+p))");
    assert_eq!(shaped(&m2), "ANY SHORTEST (LAST_NODE(Q))((Q)<-[F]-)+(P)");
    let m3 = graph_match("SELECT 1 FROM p WHERE MATCH(p(-(f)->q){2})");
    assert_eq!(shaped(&m3), "(P)(-[F]->(Q)){2,2}");
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn last_node_equality_is_kept_for_same() {
    let m1 = graph_match(
        "SELECT 1 FROM a, b WHERE MATCH(SHORTEST_PATH(a(-(f)->x)+) AND SHORTEST_PATH(b(-(g)->y)+) AND LAST_NODE(x) = LAST_NODE(y))",
    );
    assert_eq!(m1.ends, [("LAST_NODE(X)".to_owned(), "LAST_NODE(Y)".to_owned())]);
    let m2 = graph_match("SELECT 1 FROM a WHERE MATCH(SHORTEST_PATH(a(-(f)->x)+) AND LAST_NODE(x)-(g)->c)");
    assert_eq!(shaped(&m2), "ANY SHORTEST (A)(-[F]->(X))+(LAST_NODE(X)), (LAST_NODE(X))-[G]->(C)");
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn match_under_or_and_not_reads() {
    let s = spec("SELECT 1 FROM a, e, b WHERE NOT MATCH(a-(e)->b) OR MATCH(b-(e)->a)");
    assert!(matches!(s.table_expression.where_clause, Some(SearchCondition::Or(..))));
}

/// Verifies: [EXTENSION] [GS §10.1]
#[test]
fn graph_path_needs_a_conjunct_match() {
    assert_eq!(refused("SELECT COUNT(f.x) WITHIN GROUP (GRAPH PATH) FROM a, e FOR PATH f WHERE NOT MATCH(a(-(f)->b)+)"), -103_i32);
    assert_eq!(refused("SELECT 1 FROM a, e FOR PATH f"), -103_i32);
    assert_eq!(refused("SELECT 1 FROM a WHERE MATCH(a-(e)-b)"), -102_i32);
}

/// Verifies: [EXTENSION] [GS §10.2], GS R16.8
#[test]
fn hints_are_read_and_checked() {
    let t1 = graph_table(
        "SELECT * FROM GRAPH_TABLE (/*+ GRAPH_START(a) NO_WCOJ PATH_ALGO(BIBFS) SOME_OTHER(1) */ g MATCH (a) -> (b) COLUMNS (a.x))",
    );
    assert_eq!(t1.hints.start.as_ref().map(|(s, _)| s.as_str()), Some("A"));
    assert!(t1.hints.no_wcoj && t1.hints.path_algo == Some(PathAlgorithm::Bibfs), "{:?}", t1.hints);
    let t3 =
        graph_table("SELECT * FROM GRAPH_TABLE (/*+ USE_ADJACENCY(e1) */ g MATCH (a) -> (b) COLUMNS (a.x) OPTIONS (USE_ADJACENCY (e2)))");
    assert_eq!(t3.hints.use_adjacency.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["E2", "E1"]);
    assert_eq!(refused("SELECT * FROM GRAPH_TABLE (/*+ PATH_ALGO(SIDEWAYS) */ g MATCH (a) -> (b) COLUMNS (a.x))"), -102_i32);
    let t2 = graph_table("SELECT * FROM GRAPH_TABLE (g MATCH (a) -> (b) COLUMNS (a.x))");
    assert!(t2.hints.is_empty());
}

/// Verifies: [EXTENSION] [GS §10.3]
#[test]
fn pgql_query_is_a_graph_table() {
    let text =
        "SELECT n.name, COUNT(*) FROM MATCH (n:Person) -[e:knows]-> (m) ON social WHERE m.age > 30 GROUP BY n.name ORDER BY n.name LIMIT 5";
    let t1 = graph_table(text);
    assert_eq!(shape(&t1.pattern), "(N:PERSON)-[E:KNOWS]->(M) WHERE c");
    assert_eq!(column_names(&t1), ["NAME"]);
    let QueryExpression::Ordered(o) = query(text).unwrap_or_else(|e| panic!("{e:?}")) else { panic!("{text}") };
    assert!(o.fetch.is_some() && o.offset.is_none() && o.order_by.len() == 1);
    let t2 = graph_table("SELECT a.x, b.y FROM MATCH (a) -> (b), MATCH (b) -> (c)");
    assert_eq!(shape(&t2.pattern), "(A)->(B), (B)->(C)");
    assert_eq!(column_names(&t2), ["X", "Y"]);
    let t3 = graph_table("SELECT (SELECT 1 FROM t) AS one, LISTAGG(x.name, ',') FROM MATCH ANY SHORTEST (a) (-[e]->(x))+ (b)");
    assert_eq!(column_names(&t3), ["$1"]);
}

/// Verifies: [EXTENSION] [GS §10.3]
#[test]
fn pgql_names_one_graph() {
    assert_eq!(refused("SELECT a.x FROM MATCH (a) ON g1, MATCH (b) ON g2"), -103_i32);
}

/// Verifies: [EXTENSION] [GS §10.3]
#[test]
fn cypher_query_is_a_graph_table() {
    let text =
        "MATCH (a:Person {name: 'Ann'})-[r:KNOWS*1..3]->(b:Person:Admin) WHERE b.age > 3 RETURN b.name AS friend, count(r) SKIP 1 LIMIT 2";
    let t1 = graph_table(text);
    assert_eq!(shape(&t1.pattern), "DIFFERENT EDGES TRAIL (A:PERSON {NAME: v})-[R:KNOWS]->{1,3}(B:(PERSON&ADMIN)) WHERE c");
    let t2 = graph_table("USE s.g MATCH (a) RETURN a.x");
    assert!(matches!(&t2.graph, crate::ast::GraphReference::Named(n) if n.auth.as_deref() == Some("S")), "{:?}", t2.graph);
    let t3 = graph_table("USE g MATCH p = shortestPath((a)-[:R*]-(b)) RETURN length(p)");
    assert_eq!(shape(&t3.pattern), "DIFFERENT EDGES ANY SHORTEST P = (A)-[:R]-{1,}(B)");
    let t4 = graph_table("MATCH (a)<-[:R|:S *2]-(b) RETURN a.x");
    assert_eq!(shape(&t4.pattern), "DIFFERENT EDGES TRAIL (A)<-[:(R|S)]-{2,2}(B)");
}

/// Verifies: [EXTENSION] [GS §10.3]
#[test]
fn cypher_refuses_what_it_does_not_read() {
    assert_eq!(refused("MATCH (a*2) RETURN a.x"), -102_i32);
    assert_eq!(refused("MATCH (a)-[r*1..x]->(b) RETURN a.x"), -102_i32);
    assert_eq!(refused("MATCH (a)-[r]->(b)"), -102_i32);
    assert_eq!(refused("MATCH (a)-[e*]->(b) RETURN e.id"), -103_i32);
}
