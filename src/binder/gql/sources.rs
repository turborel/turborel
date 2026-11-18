//! \[EXTENSION\] The rows an element pattern of a GQL MATCH may bind to,
//! as SQL (RULES.md D-GQL): of each element table its labels allow, the
//! element's identity text `<alias>|<key>|...`, its label set `|A|B|`, and
//! its properties; of an edge, the identities of the vertices it leaves
//! and reaches in each direction the pattern allows. An open graph's
//! two tables hold the label set as text and the properties as JSON.

use super::{Graph, ID_TYPE, Kind, LABEL_BAR, Var, quote, string};
use crate::ast::{Directions, LabelExpression};
use crate::catalog::{ElementKind, ElementTableDesc, EndpointDesc, identifier, qualified_identifier};
use crate::error::{Code, Result, SqlError};

/// The prefix of an open graph's vertex's identity.
pub(crate) const OPEN_NODE: &str = "N";
/// The prefix of an open graph's edge's identity.
pub(crate) const OPEN_EDGE: &str = "E";

/// Whether the label set `labels` satisfies `e`.
pub(super) fn label_holds(e: &LabelExpression, labels: &[&str]) -> bool {
    match e {
        LabelExpression::Label(l, _) => labels.contains(&l.as_str()),
        LabelExpression::Wildcard(_) => !labels.is_empty(),
        LabelExpression::Negation(inner) => !label_holds(inner, labels),
        LabelExpression::Conjunction(a, b) => label_holds(a, labels) && label_holds(b, labels),
        LabelExpression::Disjunction(a, b) => label_holds(a, labels) || label_holds(b, labels),
    }
}

/// `e` as a test of the label set held in the SQL value `set`.
pub(super) fn label_sql(e: &LabelExpression, set: &str) -> String {
    match e {
        LabelExpression::Label(l, _) => super::rewrite::label_test(set, l),
        LabelExpression::Wildcard(_) => format!("({set} <> {})", string(LABEL_BAR)),
        LabelExpression::Negation(inner) => format!("(NOT {})", label_sql(inner, set)),
        LabelExpression::Conjunction(a, b) => format!("({} AND {})", label_sql(a, set), label_sql(b, set)),
        LabelExpression::Disjunction(a, b) => format!("({} OR {})", label_sql(a, set), label_sql(b, set)),
    }
}

/// Each label `e` names, and where.
fn named(e: &LabelExpression, out: &mut Vec<(String, usize)>) {
    match e {
        LabelExpression::Label(l, at) => out.push((l.clone(), *at)),
        LabelExpression::Wildcard(_) => {}
        LabelExpression::Negation(inner) => named(inner, out),
        LabelExpression::Conjunction(a, b) | LabelExpression::Disjunction(a, b) => {
            named(a, out);
            named(b, out);
        }
    }
}

/// The element tables of kind `kind` an element pattern of label
/// expression `label` may bind to.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) of a label no element table of the graph has.
pub(super) fn candidates<'g>(graph: &'g Graph, kind: ElementKind, label: Option<&LabelExpression>) -> Result<Vec<&'g ElementTableDesc>> {
    let tables = graph.desc.elements.iter().filter(|e| e.kind == kind);
    if graph.open {
        return Ok(tables.collect());
    }
    let mut labels = Vec::new();
    if let Some(e) = label {
        named(e, &mut labels);
    }
    let known = graph.desc.labels(None);
    if let Some((l, at)) = labels.iter().find(|(l, _)| !known.contains(&l.as_str())) {
        return Err(SqlError::at(Code::SyntaxRule, *at, format!("[EXTENSION] GRF-0012 graph {} has no label {l}", graph.desc.name)));
    }
    let holds = |e: &&ElementTableDesc| {
        let set: Vec<&str> = e.labels.iter().map(|l| l.name.as_str()).collect();
        label.is_none_or(|x| label_holds(x, &set))
    };
    Ok(tables.filter(holds).collect())
}

/// The variable `name` of an element of kind `kind` that binds to one of
/// `tables`: each property of their labels, of the type of the first
/// that has it.
pub(crate) fn element_var(graph: &Graph, name: &str, kind: ElementKind, tables: &[&ElementTableDesc]) -> Var {
    let edge = kind == ElementKind::Edge;
    if graph.open {
        return Var { name: name.to_owned(), kind: Kind::Element { edge, properties: None } };
    }
    let mut properties: Vec<(String, String)> = Vec::new();
    for p in tables.iter().flat_map(|t| t.properties()) {
        if !properties.iter().any(|(n, _)| *n == p.name) {
            properties.push((p.name.clone(), p.data_type.to_string()));
        }
    }
    Var { name: name.to_owned(), kind: Kind::Element { edge, properties: Some(properties) } }
}

/// The identity text of a row of element table `e`, its key columns
/// qualified by `q`.
pub(crate) fn identity(e: &ElementTableDesc, q: &str) -> String {
    let mut parts = vec![string(&e.alias)];
    for k in &e.key {
        parts.push(string(LABEL_BAR));
        parts.push(format!("CAST({q}.{} AS VARCHAR(200))", identifier(k)));
    }
    format!("CAST({} AS {ID_TYPE})", parts.join(" || "))
}

/// The identity text of the vertex endpoint `ep` of a row of an edge
/// table, the edge's columns qualified by `q`: written from the edge's
/// columns where they match the vertex's key, else read from the vertex
/// table.
pub(crate) fn endpoint(graph: &Graph, ep: &EndpointDesc, q: &str) -> Result<String> {
    let Some(v) = graph.desc.element(&ep.vertex) else {
        return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0004 vertex table {} is not in the graph", ep.vertex)));
    };
    let by_key: Option<Vec<&String>> =
        v.key.iter().map(|k| ep.references.iter().position(|r| r == k).and_then(|i| ep.columns.get(i))).collect();
    if let Some(columns) = by_key {
        let mut parts = vec![string(&v.alias)];
        for c in columns {
            parts.push(string(LABEL_BAR));
            parts.push(format!("CAST({q}.{} AS VARCHAR(200))", identifier(c)));
        }
        return Ok(format!("CAST({} AS {ID_TYPE})", parts.join(" || ")));
    }
    let on: Vec<String> =
        ep.references.iter().zip(&ep.columns).map(|(r, c)| format!("\"$v\".{} = {q}.{}", identifier(r), identifier(c))).collect();
    Ok(format!("(SELECT {} FROM {} AS \"$v\" WHERE {})", identity(v, "\"$v\""), qualified_identifier(&v.table), on.join(" AND ")))
}

/// The select list of `var`'s columns of a row of element table `e`,
/// qualified by `q`.
pub(crate) fn element_columns(e: &ElementTableDesc, var: &Var, q: &str) -> Vec<String> {
    let labels: Vec<&str> = e.labels.iter().map(|l| l.name.as_str()).collect();
    let set = format!("{LABEL_BAR}{}{LABEL_BAR}", labels.join(LABEL_BAR));
    let mut cols = vec![
        format!("{} AS {}", identity(e, q), quote(&var.name)),
        format!("CAST({} AS {ID_TYPE}) AS {}", string(&set), quote(&var.labels_column())),
    ];
    if let Kind::Element { properties: Some(props), .. } = &var.kind {
        for (p, t) in props {
            let value = e.property(p).map_or_else(|| "NULL".to_owned(), |d| format!("({})", d.expression));
            cols.push(format!("CAST({value} AS {t}) AS {}", quote(&var.property_column(p))));
        }
    }
    cols
}

/// The rows of `var`, of a vertex pattern whose element tables are
/// `tables` and label expression `label`.
pub(super) fn vertex_rows(graph: &Graph, var: &Var, tables: &[&ElementTableDesc], label: Option<&LabelExpression>) -> String {
    if graph.open {
        return open_rows(graph, var, label, None);
    }
    let selects: Vec<String> = tables
        .iter()
        .map(|e| format!("SELECT {} FROM {} AS \"$e\"", element_columns(e, var, "\"$e\"").join(", "), qualified_identifier(&e.table)))
        .collect();
    union_or_none(&selects, var, &[])
}

/// The rows of `var`, of an edge pattern whose element tables are
/// `tables`, label expression `label` and directions `ways`: each with
/// "$FROM" and "$TO", the vertices it leaves and reaches as matched.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0004) if an endpoint's vertex table is not in the
/// graph.
pub(super) fn edge_rows(
    graph: &Graph,
    var: &Var,
    (tables, label): (&[&ElementTableDesc], Option<&LabelExpression>),
    ways: Directions,
) -> Result<String> {
    if graph.open {
        return Ok(open_rows(graph, var, label, Some(ways)));
    }
    let mut selects = Vec::new();
    for e in tables {
        let Some(ends) = &e.endpoints else { continue };
        let (src, dst) = (endpoint(graph, &ends.source, "\"$e\"")?, endpoint(graph, &ends.destination, "\"$e\"")?);
        let cols = element_columns(e, var, "\"$e\"").join(", ");
        let from = format!("FROM {} AS \"$e\"", qualified_identifier(&e.table));
        let forward = (ends.undirected && ways.undirected) || (!ends.undirected && ways.right);
        let backward = (ends.undirected && ways.undirected) || (!ends.undirected && ways.left);
        if forward {
            selects.push(format!("SELECT {cols}, {src} AS \"$FROM\", {dst} AS \"$TO\" {from}"));
        }
        if backward {
            selects.push(format!("SELECT {cols}, {dst} AS \"$FROM\", {src} AS \"$TO\" {from}"));
        }
    }
    Ok(union_or_none(&selects, var, &["\"$FROM\"", "\"$TO\""]))
}

/// The selects `selects` combined by UNION ALL, or, if none, a select of
/// no row of `var`'s columns and `extra`.
fn union_or_none(selects: &[String], var: &Var, extra: &[&str]) -> String {
    if !selects.is_empty() {
        return selects.join(" UNION ALL ");
    }
    let mut cols: Vec<String> =
        var.columns().into_iter().map(|(c, t)| format!("CAST(NULL AS {}) AS {}", t.unwrap_or_default(), quote(&c))).collect();
    cols.extend(extra.iter().map(|c| format!("CAST(NULL AS {ID_TYPE}) AS {c}")));
    format!("SELECT {} FROM (VALUES (0)) AS \"$z\"(\"$Z\") WHERE 1 = 0", cols.join(", "))
}

/// The table of an open graph's elements of kind `kind`.
pub(crate) fn open_table(graph: &Graph, kind: ElementKind) -> String {
    graph.desc.elements.iter().find(|e| e.kind == kind).map_or_else(String::new, |e| qualified_identifier(&e.table))
}

/// An open graph's vertex's or edge's identity text, of its number
/// `id`.
pub(crate) fn open_id(prefix: &str, id: &str) -> String {
    format!("CAST({} || CAST({id} AS VARCHAR(20)) AS {ID_TYPE})", string(&format!("{prefix}{LABEL_BAR}")))
}

/// The rows of `var` of an open graph: of its vertices, or of its edges
/// in the directions `ways`.
fn open_rows(graph: &Graph, var: &Var, label: Option<&LabelExpression>, ways: Option<Directions>) -> String {
    let (prefix, kind) = if ways.is_some() { (OPEN_EDGE, ElementKind::Edge) } else { (OPEN_NODE, ElementKind::Vertex) };
    let cols = format!(
        "{} AS {}, CAST(\"$e\".\"LABELS\" AS {ID_TYPE}) AS {}, \"$e\".\"PROPS\" AS {}",
        open_id(prefix, "\"$e\".\"ID\""),
        quote(&var.name),
        quote(&var.labels_column()),
        quote(&var.props_column())
    );
    let test = label.map_or_else(|| "1 = 1".to_owned(), |e| label_sql(e, "\"$e\".\"LABELS\""));
    let from = format!("FROM {} AS \"$e\" WHERE {test}", open_table(graph, kind));
    let Some(ways) = ways else {
        return format!("SELECT {cols} {from}");
    };
    let (src, dst) = (open_id(OPEN_NODE, "\"$e\".\"SRC\""), open_id(OPEN_NODE, "\"$e\".\"DST\""));
    let mut selects = Vec::new();
    if ways.right || ways.undirected {
        let which = directed_test((ways.right, ways.undirected));
        selects.push(format!("SELECT {cols}, {src} AS \"$FROM\", {dst} AS \"$TO\" {from} AND {which}"));
    }
    if ways.left || ways.undirected {
        let which = directed_test((ways.left, ways.undirected));
        selects.push(format!("SELECT {cols}, {dst} AS \"$FROM\", {src} AS \"$TO\" {from} AND {which}"));
    }
    union_or_none(&selects, var, &["\"$FROM\"", "\"$TO\""])
}

/// The test of an open graph's edge's DIRECTED column: directed edges
/// if `directed`, undirected ones if `undirected`.
fn directed_test((directed, undirected): (bool, bool)) -> &'static str {
    match (directed, undirected) {
        (true, true) => "1 = 1",
        (true, false) => "\"$e\".\"DIRECTED\"",
        (false, true) => "NOT \"$e\".\"DIRECTED\"",
        (false, false) => "1 = 0",
    }
}

/// Every vertex's identity, as "$ID": the rows of a quantified pattern's
/// zero iterations.
pub(super) fn all_vertices(graph: &Graph) -> String {
    if graph.open {
        return format!(
            "SELECT {} AS \"$ID\" FROM {} AS \"$e\"",
            open_id(OPEN_NODE, "\"$e\".\"ID\""),
            open_table(graph, ElementKind::Vertex)
        );
    }
    let selects: Vec<String> = graph
        .desc
        .vertices()
        .map(|e| format!("SELECT {} AS \"$ID\" FROM {} AS \"$e\"", identity(e, "\"$e\""), qualified_identifier(&e.table)))
        .collect();
    if selects.is_empty() {
        return format!("SELECT CAST(NULL AS {ID_TYPE}) AS \"$ID\" FROM (VALUES (0)) AS \"$z\"(\"$Z\") WHERE 1 = 0");
    }
    selects.join(" UNION ALL ")
}
