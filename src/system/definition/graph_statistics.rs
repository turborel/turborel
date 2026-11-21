//! \[EXTENSION\] The Definition Schema's table GRAPH_STATISTICS (RULES.md
//! D151): of each property graph, the statistics its planner reads
//! (`stats/graph.rs`), a row of each kind of them. VERTEX and EDGE, of an
//! element table: its rows and, of an edge table, its out-degree and
//! in-degree distributions, each vertex with an edge there counted once.
//! PROPERTY, of a property of an element table: its distinct values and
//! the share NULL, of a property that is a column alone. TRANSITION, of a
//! source label, an edge label and a destination label: the edges that
//! make it. A statistic not known yet, its tables' statistics not
//! gathered, is NULL. The view GRAPH_STATISTICS reads it
//! (`information/graphs.rs`).

use std::sync::Arc;

use super::{Columns, defined, ids};
use crate::catalog::{ColumnDesc, ElementKind};
use crate::stats::graph::{Degree, ElementStatistics, GraphStatistics};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, text};
use crate::value::cast::approximate_to_exact;
use crate::value::{BIGINT_PRECISION, DataType, Spelled, Value};

/// \[DESIGN\] The decimal places of an average or a share shown.
const PLACES: u32 = 2;
/// The degree columns of each end: AVERAGE, MEDIAN, P90, P99, MAXIMUM.
const DEGREE_COLUMNS: [&str; 5] = ["AVERAGE", "MEDIAN", "P90", "P99", "MAXIMUM"];

/// The table.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![defined("GRAPH_STATISTICS", columns, Rows::Infallible(rows))]
}

/// A column `name` of an estimate: DECIMAL(18, 2), nullable.
fn estimate(name: &str) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Exact { precision: BIGINT_PRECISION, scale: PLACES, spelled: Spelled::Decimal },
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }
}

fn columns() -> Vec<ColumnDesc> {
    let mut columns = Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .id("STATISTIC_KIND", true)
        .id("ELEMENT_TABLE_ALIAS", false)
        .id("PROPERTY_NAME", false)
        .id("SOURCE_LABEL", false)
        .id("EDGE_LABEL", false)
        .id("DESTINATION_LABEL", false)
        .done();
    columns.extend(["ROW_COUNT", "DISTINCT_VALUES", "NULL_FRACTION"].map(estimate));
    for end in ["OUT_DEGREE", "IN_DEGREE"] {
        columns.extend(DEGREE_COLUMNS.map(|c| estimate(&format!("{end}_{c}"))));
    }
    columns
}

/// `x` as an estimate column holds it, rounded; NULL if not known.
fn shown(x: Option<f64>) -> Value {
    x.and_then(|x| approximate_to_exact(x, (BIGINT_PRECISION, PLACES), "GRAPH_STATISTICS").ok()).unwrap_or(Value::Null)
}

/// The degree columns of `degree`, NULL if not known.
fn degrees(degree: Option<Degree>) -> [Value; 5] {
    let f = degree.map(|d| d.frequencies);
    [
        shown(degree.map(|d| d.average)),
        shown(f.map(|f| f.median)),
        shown(f.map(|f| f.p90)),
        shown(f.map(|f| f.p99)),
        shown(f.map(|f| f.most)),
    ]
}

/// A row's columns after its graph's name: its kind, alias, property,
/// labels, then the estimates.
struct Tail<'a> {
    kind: &'a str,
    alias: Option<&'a str>,
    property: Option<&'a str>,
    labels: Option<[&'a str; 3]>,
    counts: [Option<f64>; 3],
    degrees: [Option<Degree>; 2],
}

impl Tail<'_> {
    fn values(&self) -> Vec<Value> {
        let name = |s: Option<&str>| s.map_or(Value::Null, text);
        let mut out = vec![text(self.kind), name(self.alias), name(self.property)];
        out.extend((0..3).map(|n| name(self.labels.and_then(|l| l.get(n).copied()))));
        out.extend(self.counts.map(shown));
        out.extend(self.degrees.into_iter().flat_map(degrees));
        out
    }
}

/// The rows of element table `e`: its own, then one of each property.
fn element_rows(e: &ElementStatistics) -> Vec<Tail<'_>> {
    let kind = match e.kind {
        ElementKind::Vertex => "VERTEX",
        ElementKind::Edge => "EDGE",
    };
    let mut out = vec![Tail {
        kind,
        alias: Some(&e.alias),
        property: None,
        labels: None,
        counts: [e.rows, None, None],
        degrees: [e.out_degree, e.in_degree],
    }];
    out.extend(e.properties.iter().map(|p| Tail {
        kind: "PROPERTY",
        alias: Some(&e.alias),
        property: Some(&p.name),
        labels: None,
        counts: [None, p.distinct, p.nulls],
        degrees: [None, None],
    }));
    out
}

/// Of `stats`, each row's columns after its graph's name.
fn tails(stats: &GraphStatistics) -> Vec<Tail<'_>> {
    let mut out: Vec<Tail<'_>> = stats.elements.iter().flat_map(element_rows).collect();
    out.extend(stats.transitions.iter().map(|t| Tail {
        kind: "TRANSITION",
        alias: None,
        property: None,
        labels: Some([&t.source, &t.edge, &t.destination]),
        counts: [Some(t.edges), None, None],
        degrees: [None, None],
    }));
    out
}

/// Each graph's statistics, as known now.
fn rows(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for g in seen.state.catalog.graphs() {
        let Some(stats) = seen.state.graph_statistics(&g.name) else { continue };
        rows.extend(tails(&stats).iter().map(|t| [ids(&catalog, &g.name).to_vec(), t.values()].concat()));
    }
    rows
}
