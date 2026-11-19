//! Unit tests of a graph's statistics (RULES.md D151): made of its
//! tables' statistics as given.

use std::sync::Arc;

use super::*;
use crate::catalog::{Endpoints, GraphMode, Horizon, LabelDesc, PropertyDesc};
use crate::stats::StatsBuilder;
use crate::value::{DataType, Decimal};

/// The name `ident` of schema A.
fn a(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A whole number.
fn number(n: i32) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// A label `name` of the properties, each a column alone or an
/// expression.
fn label(name: &str, properties: &[(&str, &str)]) -> LabelDesc {
    let p = |(n, e): &(&str, &str)| PropertyDesc { name: (*n).into(), expression: (*e).into(), data_type: DataType::Boolean };
    LabelDesc { name: name.into(), properties: properties.iter().map(p).collect() }
}

/// An edge table `alias` of label `l` from V's (K1, K2) by (S1, S2) to
/// V's K1 by D.
fn edge(alias: &str, l: &str) -> ElementTableDesc {
    let end = |columns: &[&str], references: &[&str]| EndpointDesc {
        vertex: "V".into(),
        columns: columns.iter().map(|c| (*c).into()).collect(),
        references: references.iter().map(|c| (*c).into()).collect(),
    };
    ElementTableDesc {
        alias: alias.into(),
        table: a(alias),
        kind: ElementKind::Edge,
        key: vec!["ID".into()],
        labels: vec![label(l, &[])],
        endpoints: Some(Endpoints { source: end(&["S1", "S2"], &["K1", "K2"]), destination: end(&["D"], &["K1"]), undirected: false }),
        query: None,
    }
}

/// Graph A.G: vertex table V of labels P and Q, its properties K1 and
/// DOUBLED, an expression; edge tables E and F, each of label L.
fn graph() -> GraphDesc {
    let v = ElementTableDesc {
        alias: "V".into(),
        table: a("V"),
        kind: ElementKind::Vertex,
        key: vec!["K1".into(), "K2".into()],
        labels: vec![label("P", &[("K1", "K1"), ("DOUBLED", "K1 * 2")]), label("Q", &[])],
        endpoints: None,
        query: None,
    };
    GraphDesc {
        name: a("G"),
        elements: vec![v, edge("E", "L"), edge("F", "L")],
        mode: GraphMode::Trusted,
        invoker: false,
        names: "A".into(),
        reads: Vec::new(),
        horizon: Horizon::default(),
        valid: true,
        select: Vec::new(),
    }
}

/// Whether `got` is within a hundredth of `want`, a sketch's estimate.
fn near(got: f64, want: f64) -> bool {
    (got - want).abs() < 0.01
}

/// Statistics of `rows`, each of `columns`.
fn table(columns: &[&str], rows: &[Vec<Value>]) -> (Arc<dyn Statistics>, Vec<String>) {
    let mut builder = StatsBuilder::new(columns.len());
    for row in rows {
        builder.add(row, 8);
    }
    let stats: Arc<dyn Statistics> = Arc::new(builder.finish());
    (stats, columns.iter().map(|c| (*c).into()).collect())
}

/// V of 4 rows; E of 6 edges: (S1, S2) of (1, 1) three times, then (1,
/// 2), (2, 2) and (3, 2), so S1 has 3 values of 4, 1 and 1 edges and S2
/// 2 of 3 and 3; D NULL once.
fn tables(name: &QualifiedName) -> Option<(Arc<dyn Statistics>, Vec<String>)> {
    match name.ident.as_str() {
        "V" => Some(table(&["K1", "K2"], &(1..=4).map(|k| vec![number(k), number(1)]).collect::<Vec<_>>())),
        "E" => {
            let pairs = [(1, 1, 1), (1, 1, 2), (1, 1, 3), (1, 2, 1), (2, 2, 2), (3, 2, 0)];
            let rows: Vec<Vec<Value>> =
                pairs.iter().map(|&(s1, s2, d)| vec![number(s1), number(s2), if d == 0_i32 { Value::Null } else { number(d) }]).collect();
            Some(table(&["S1", "S2", "D"], &rows))
        }
        _ => None,
    }
}

/// Verifies: [DESIGN] RULES.md D151: of an endpoint of several columns,
/// the degrees are those of its column of most distinct values, the
/// maximum the least of its columns' maxima; of one column, its values'
/// frequencies; the fan-out is the edges by the vertex table's rows, an
/// edge of a NULL endpoint column not counted; an edge table whose
/// statistics are not known has none.
#[test]
fn degrees_of_one_column_and_of_several() {
    let stats = GraphStatistics::of(&graph(), &tables);
    let out = stats.degree("E", Direction::Out).unwrap();
    assert!(near(out.average, 2.0), "6 edges of the 3 distinct values of S1: {}", out.average);
    assert_eq!(out.frequencies.most, 3.0_f64, "the least of S1's 4 and S2's 3");
    assert_eq!(out.fan_out, Some(1.5_f64), "6 edges of 4 vertices");
    let into = stats.degree("E", Direction::In).unwrap();
    assert!(near(into.average, 5.0 / 3.0), "{}", into.average);
    assert_eq!((into.frequencies.most, into.frequencies.median), (2.0_f64, 2.0_f64));
    assert_eq!(into.fan_out, Some(1.25_f64), "5 edges not NULL of 4 vertices");
    assert_eq!(stats.degree("F", Direction::Out), None, "F's statistics are not known");
    assert_eq!(stats.degree("V", Direction::Out), None, "a vertex table");
    assert_eq!((stats.rows("V"), stats.rows("E"), stats.rows("F"), stats.rows("X")), (Some(4.0_f64), Some(6.0_f64), None, None));
}

/// Verifies: [DESIGN] RULES.md D151: each label transition of an edge
/// table whose statistics are known counts its edges with neither end
/// NULL, once for each source and destination label; asked for by any
/// part, they are summed; a property that is a column alone has that
/// column's statistics, an expression none.
#[test]
fn transitions_and_properties() {
    let stats = GraphStatistics::of(&graph(), &tables);
    assert_eq!(stats.transitions.len(), 4, "P or Q, L, P or Q");
    assert_eq!(stats.transitions(Some("P"), Some("L"), Some("Q")), Some(5.0_f64));
    assert_eq!(stats.transitions(None, Some("L"), None), Some(20.0_f64));
    assert_eq!(stats.transitions(Some("X"), None, None), None);
    let k1 = stats.property("V", "K1").unwrap();
    assert!(k1.distinct.is_some_and(|d| near(d, 4.0)) && k1.nulls == Some(0.0_f64), "{k1:?}");
    assert_eq!(k1.bounds.as_ref().map(|(least, most)| (least.to_string(), most.to_string())), Some(("1".into(), "4".into())));
    let doubled = stats.property("V", "DOUBLED").unwrap();
    assert_eq!((doubled.distinct, doubled.nulls, doubled.bounds.is_none()), (None, None, true));
    assert!(stats.property("V", "NONE").is_none());
}
