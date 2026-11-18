//! Unit tests of GRAPH_TABLE's clauses and of the predicates and
//! functions of graph elements and paths [P1 §4.7; graph standard §6.9,
//! §8, §11]: each read to its tree, inside a GRAPH_TABLE only where its
//! name could be a routine's.

use super::*;
use crate::ast::{
    GraphColumn, GraphFunctionName, GraphPredicate, GraphReference, PathEnd, Predicate, SameKind, SearchCondition, SystemTimeKind,
    ValueExpression,
};
use GraphFunctionName as F;

/// Adds to `found` each graph function in `v`, its own children's too.
fn functions(v: &ValueExpression, found: &mut Vec<(GraphFunctionName, Option<String>)>) {
    if let ValueExpression::Graph(f, _) = v {
        found.push((f.name, f.property.clone()));
    }
    v.for_each_child(&mut |c| functions(c, found));
}

/// The postfilter of `matched_text`, which is accepted.
fn postfilter(matched_text: &str) -> SearchCondition {
    let read = matched(matched_text).unwrap_or_else(|e| panic!("{matched_text}: {e:?}"));
    read.pattern.where_clause.unwrap_or_else(|| panic!("{matched_text}: no WHERE"))
}

/// The graph predicate `condition` is.
fn graph_predicate(condition: &SearchCondition) -> GraphPredicate {
    let SearchCondition::Predicate(p, _) = condition else { panic!("{condition:?}") };
    let Predicate::Graph(g) = &**p else { panic!("{p:?}") };
    (**g).clone()
}

/// The values of the COLUMNS of the GRAPH_TABLE `text`, which is
/// accepted.
fn columns(text: &str) -> Vec<GraphColumn> {
    table(text).unwrap_or_else(|e| panic!("{text}: {e:?}")).columns
}

/// The graph predicate of the postfilter of `matched_text`.
fn predicate_of(matched_text: &str) -> GraphPredicate {
    graph_predicate(&postfilter(matched_text))
}

/// The names of the variables `elements`, in order.
fn names(elements: &[GraphVariable]) -> Vec<&str> {
    elements.iter().map(|x| x.name.as_str()).collect()
}

/// Verifies: [EXTENSION] [P1 §4.7] SAME, VERTEX_EQUAL and EDGE_EQUAL
/// read to the same predicate of their variables in order, its spelling
/// kept; VERTEX_EQUAL and EDGE_EQUAL of two only, SAME of two at least.
#[test]
fn same_predicates_read_their_operands_4_7() {
    let same = predicate_of("(a)-[e]->(b)-[f]->(c) WHERE SAME(a, b, c)");
    assert!(matches!(&same, GraphPredicate::Same { kind: SameKind::Same, elements } if names(elements) == ["A", "B", "C"]));
    let vertices = predicate_of("(a)-[e]->(b) WHERE VERTEX_EQUAL(a, b)");
    assert!(matches!(&vertices, GraphPredicate::Same { kind: SameKind::VertexEqual, elements } if elements.len() == 2));
    let edges = predicate_of("(a)-[e]->(b)-[f]->(c) WHERE EDGE_EQUAL(e, f)");
    assert!(matches!(&edges, GraphPredicate::Same { kind: SameKind::EdgeEqual, .. }));
    assert_eq!(refused("(a)-[e]->(b) WHERE VERTEX_EQUAL(a, b, a)"), -102_i32);
    assert_eq!(refused("(a) WHERE SAME(a)"), -102_i32);
}

/// Verifies: [EXTENSION] [P1 §4.7] ALL_DIFFERENT reads its variables in
/// order, and PROPERTY_EXISTS its variable and property name.
#[test]
fn all_different_and_property_exists_read_4_7() {
    let different = predicate_of("(a)-[e]->(b) WHERE ALL_DIFFERENT(a, e, b)");
    assert!(matches!(&different, GraphPredicate::AllDifferent(v) if names(v) == ["A", "E", "B"]));
    let exists = predicate_of("(a) WHERE PROPERTY_EXISTS(a, owner)");
    assert!(matches!(&exists, GraphPredicate::PropertyExists { element, property } if element.name == "A" && property == "OWNER"));
}

/// Verifies: [EXTENSION] [P1 §4.7] IS \[NOT\] SOURCE OF, DESTINATION OF
/// and DIRECTED after a variable, and either with AND and OR.
#[test]
fn endpoint_predicates_read_their_operands_4_7() {
    let source = predicate_of("(a)-[e]-(b) WHERE a IS SOURCE OF e");
    assert!(matches!(&source, GraphPredicate::Endpoint { end: PathEnd::Source, not: false, .. }));
    let destination = predicate_of("(a)-[e]-(b) WHERE b IS NOT DESTINATION OF e");
    let named = |node: &GraphVariable, edge: &GraphVariable| (node.name.as_str(), edge.name.as_str()) == ("B", "E");
    assert!(matches!(&destination, GraphPredicate::Endpoint { end: PathEnd::Destination, not: true, node, edge } if named(node, edge)));
    assert!(matches!(&predicate_of("(a)-[e]-(b) WHERE e IS DIRECTED"), GraphPredicate::Directed { not: false, .. }));
    assert!(matches!(&predicate_of("(a)-[e]-(b) WHERE e IS NOT DIRECTED"), GraphPredicate::Directed { not: true, .. }));
    let both = postfilter("(a)-[e]-(b) WHERE e IS DIRECTED AND a IS SOURCE OF e OR NOT b:L");
    assert!(matches!(both, SearchCondition::Or(..)));
}

/// Verifies: [EXTENSION] [P1 §4.7] IS \[NOT\] LABELED and `x` : `<label
/// expression>` after a variable, of a whole label expression.
#[test]
fn labeled_predicates_read_their_labels_4_7() {
    let any = predicate_of("(a) WHERE a IS LABELED Account|IP");
    assert!(matches!(&any, GraphPredicate::Labeled { not: false, label: LabelExpression::Disjunction(..), .. }));
    let none = predicate_of("(a) WHERE a IS NOT LABELED %");
    assert!(matches!(&none, GraphPredicate::Labeled { not: true, label: LabelExpression::Wildcard(_), .. }));
    let colon = predicate_of("(a) WHERE a:Account&!IP");
    assert!(matches!(&colon, GraphPredicate::Labeled { not: false, label: LabelExpression::Conjunction(..), .. }));
}

/// Verifies: [EXTENSION] [Graph standard §6.9, §11.1] each function of
/// one variable reads to its name and operand inside a GRAPH_TABLE, PREV
/// and FIRST with a property after them.
#[test]
fn graph_functions_read_in_columns_6_9() {
    let text = "GRAPH_TABLE (G MATCH ANY p = (a)-[e]->+(b) COLUMNS (ELEMENT_ID(a), ELEMENT_ID(p), VERTEX_ID(a), VERTEX_ID(b), \
        LABELS(b), PATH_LENGTH(p), VERTICES(p), EDGES(p), IS_ACYCLIC(p), IS_TRAIL(p), IS_SIMPLE(p), PATH_TO_JSON(p) AS J))";
    let names: Vec<GraphFunctionName> = columns(text)
        .iter()
        .map(|c| match c {
            GraphColumn::Value { value: ValueExpression::Graph(f, _), .. } => f.name,
            GraphColumn::Value { .. } | GraphColumn::All(_) => panic!("{c:?}"),
        })
        .collect();
    let expected = [F::ElementId, F::ElementId, F::VertexId, F::VertexId, F::Labels, F::PathLength, F::Vertices, F::Edges];
    assert_eq!(names[..8], expected);
    assert_eq!(names[8..], [F::IsAcyclic, F::IsTrail, F::IsSimple, F::PathToJson]);
    let read = columns("GRAPH_TABLE (G MATCH (a)-[e]->(b) COLUMNS (EDGE_ID(e) AS ID))");
    let GraphColumn::Value { value: ValueExpression::Graph(f, _), name, .. } = &read[0] else { panic!("{read:?}") };
    assert_eq!((f.name, name.as_deref()), (F::EdgeId, Some("ID")));
    for function in crate::ast::GRAPH_FUNCTIONS {
        assert_eq!(F::named(function.1), Some(function.0));
        assert_eq!(function.0.word(), function.1);
        assert_eq!(function.0.operand(), function.2);
    }
}

/// Verifies: [EXTENSION] [Graph standard §11.1] PREV and FIRST of a
/// variable, with or without a property after them, inside a quantified
/// pattern's prefilter.
#[test]
fn prev_and_first_read_with_properties_11_1() {
    let text = "ANY (a)[-[t:Transfer]-> WHERE PREV(t) IS NULL OR t.ts > PREV(t).ts AND t.x = FIRST(t).x]+(b)";
    let read = matched(text).unwrap_or_else(|e| panic!("{e:?}"));
    let mut found = Vec::new();
    let PathPrimary::Parenthesized(p) = &read.pattern.paths[0].expression.terms()[0].factors[1].primary else { panic!() };
    p.where_clause.as_ref().unwrap().for_each_value(&mut |v| functions(v, &mut found));
    assert!(found.contains(&(F::Prev, None)), "{found:?}");
    assert!(found.contains(&(F::Prev, Some("TS".into()))), "{found:?}");
    assert!(found.contains(&(F::First, Some("X".into()))), "{found:?}");
}

/// Verifies: [EXTENSION] [P1 §4.7] a graph function's or called
/// predicate's name is a routine's outside a GRAPH_TABLE, and in a
/// subquery inside one; PREV and FIRST of more than a name are row
/// pattern navigation.
#[test]
fn graph_names_are_routines_elsewhere_4_7() {
    let mut p = Parser::new("LABELS(X) + SAME(X, Y)").unwrap();
    let v = p.value_expression().unwrap();
    let mut found = Vec::new();
    functions(&v, &mut found);
    assert!(found.is_empty(), "{v:?}");
    let text = "GRAPH_TABLE (G MATCH (a) WHERE a.x IN (SELECT LABELS(Y) FROM T WHERE SAME(Y, Y)) COLUMNS (a.x))";
    let read = table(text).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(read.pattern.where_clause.is_some());
    let mut navigation = Parser::new("PREV(X.A, 1)").unwrap();
    navigation.graph = 1;
    assert!(!matches!(navigation.value_expression(), Ok(ValueExpression::Graph(..))));
}

/// Verifies: [EXTENSION] [Graph standard §8] COLUMNS of values named or
/// not, with or without AS, and `x`.* of an element.
#[test]
fn columns_read_values_and_stars_8() {
    let read = columns("GRAPH_TABLE (G MATCH (a)-[t]->+(b) KEEP ANY COLUMNS (a.owner AS SRC, b.owner DST, SUM(t.amount), a.*, 1))");
    assert_eq!(read.len(), 5);
    assert!(matches!(&read[0], GraphColumn::Value { name: Some(n), .. } if n == "SRC"));
    assert!(matches!(&read[1], GraphColumn::Value { name: Some(n), .. } if n == "DST"));
    assert!(matches!(&read[2], GraphColumn::Value { name: None, value: ValueExpression::SetFunction(_), .. }));
    assert!(matches!(&read[3], GraphColumn::All(v) if v.name == "A"));
    assert!(table("GRAPH_TABLE (G MATCH (a) COLUMNS ())").is_err());
    assert!(table("GRAPH_TABLE (G MATCH (a))").is_err(), "COLUMNS is needed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6; graph standard §8,
/// §11.3] a GRAPH_TABLE's graph, schema-qualified or LINE_GRAPH of a
/// label, its correlation name with or without AS, FOR SYSTEM_TIME or AS
/// OF TIMESTAMP, and AS OF SCN, read as AS OF SCN_TO_TIMESTAMP(n).
#[test]
fn graph_table_reads_its_graph_and_correlation_7_6() {
    let read1 = table("GRAPH_TABLE (S.G MATCH (a) COLUMNS (a.x)) AS GT").unwrap();
    assert_eq!((read1.graph.name().auth.as_deref(), read1.graph.name().ident.as_str()), (Some("S"), "G"));
    assert_eq!(read1.correlation.as_deref(), Some("GT"));
    let read2 = table("GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) GT").unwrap();
    assert_eq!(read2.correlation.as_deref(), Some("GT"));
    let read3 = table("GRAPH_TABLE (LINE_GRAPH(BANK, TRANSFER) MATCH (a) COLUMNS (a.x))").unwrap();
    assert!(matches!(&read3.graph, GraphReference::LineGraph { graph, label, .. } if graph.ident == "BANK" && label == "TRANSFER"));
    let read4 = table("GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) AS GT AS OF TIMESTAMP CURRENT_TIMESTAMP").unwrap();
    assert!(read4.system_time.is_some());
    let read5 = table("GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) AS OF TIMESTAMP CURRENT_TIMESTAMP").unwrap();
    assert!(read5.system_time.is_some());
    assert!(read5.correlation.is_none());
    let read6 = table("GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP GT").unwrap();
    assert!(read6.system_time.is_some());
    let scn = table("GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) AS OF SCN 5").unwrap();
    let Some(SystemTimeKind::AsOf(ValueExpression::Function { name, .. })) = scn.system_time.map(|s| s.kind) else { panic!("AS OF SCN") };
    assert_eq!(name, crate::functions::SCN_TO_TIMESTAMP);
    let twice = "GRAPH_TABLE (G MATCH (a) COLUMNS (a.x)) FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP AS OF TIMESTAMP CURRENT_TIMESTAMP";
    assert_eq!(table(twice).unwrap_err().sqlcode(), -102_i32);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6] GRAPH_TABLE is a
/// `<table primary>` of a query, joined like one, and GRAPH_TABLE, its
/// clauses' words and the functions' names stay non-reserved.
#[test]
fn graph_table_is_a_table_primary_7_6() {
    let sql = "SELECT GT.X FROM T, GRAPH_TABLE (G MATCH (a WHERE a.k = T.K) COLUMNS (a.x AS X)) AS GT JOIN U ON GT.X = U.X";
    assert!(Parser::new(sql).unwrap().query_expression().is_ok());
    let names = "SELECT GRAPH_TABLE, LABELS, SAME, SEGMENT, KEEP, TRAIL, WALK, SHORTEST, CHEAPEST, VERTEX, STEP FROM T";
    assert!(Parser::new(names).unwrap().query_expression().is_ok());
    let p = Parser::new("SELECT * FROM GRAPH_TABLE").unwrap().query_expression();
    assert!(p.is_ok(), "a table named GRAPH_TABLE: {p:?}");
}
