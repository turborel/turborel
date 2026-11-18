//! \[EXTENSION\] The element tables an element pattern of a GQL MATCH may
//! bind to, and its variable's properties (RULES.md D110); label
//! expressions as tests of a label set held as text, `|A|B|`, as an open
//! graph's two tables hold it.

use super::{Graph, Kind, LABEL_BAR, Var, string};
use crate::ast::LabelExpression;
use crate::catalog::{ElementKind, ElementTableDesc};
use crate::error::{Code, Result, SqlError};

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
