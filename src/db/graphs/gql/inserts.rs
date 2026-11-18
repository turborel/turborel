//! \[EXTENSION\] INSERT of GQL (graph standard §12.1, RULES.md D110):
//! the elements its path patterns write, planned once for the statement,
//! and written for each row of the binding table.
//!
//! A vertex the pattern names that the table binds is written no more;
//! each other vertex and each edge is. A named one's variable is added to
//! the table, its columns those of the element as written. An open graph
//! writes its two tables (`gql/writes.rs`); a graph over tables, the one
//! base table the element's labels name, and of node and edge tables,
//! the element table of an edge that joins its endpoints' tables (§4.2).

use super::identities::{identity_of, key_value, open_element, table_of};
use super::modify::Binding;
use super::writes::{Typed, id_value, params, property_column, props_json, typed_for, written_tables};
use crate::ast::{EdgePattern, ElementPattern, LabelExpression, Orientation, PathPattern, PathPrimary};
use crate::binder::gql::sources::element_var;
use crate::binder::gql::{Graph, Kind, LABEL_BAR, Lowering, Var};
use crate::catalog::{ElementKind, ElementTableDesc, identifier, qualified_identifier};
use crate::db::Session;
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value};

/// An element an INSERT writes: its variable, kind, labels, the values
/// of its properties, by their columns of the values read, and, of an
/// edge, its endpoints' variables.
pub(super) struct Insert {
    /// Its variable's name.
    name: String,
    /// Whether it is bound to the table's.
    named: bool,
    /// Vertex or edge.
    kind: ElementKind,
    /// Its labels.
    labels: Vec<String>,
    /// Its properties and the value of each.
    props: Vec<(String, usize)>,
    /// Of an edge, the variables of the vertices it leaves and reaches.
    ends: Option<(String, String)>,
}

/// An element written: its identity, and its variable's columns.
type Written = (String, Vec<Typed>);

impl Session {
    /// The elements `paths` inserts: each new variable added to `b`.
    pub(super) fn insert_plan(&self, graph: &Graph, paths: &[PathPattern], b: &mut Binding, l: &Lowering<'_>) -> Result<Vec<Insert>> {
        let mut plan: Vec<Insert> = Vec::new();
        let mut value = 0_usize;
        for path in paths {
            path_plan(path, (b, l), &mut plan, &mut value)?;
        }
        // Vertices first, so each edge's endpoints are there.
        plan.sort_by_key(|i| i.kind == ElementKind::Edge);
        for i in plan.iter().filter(|i| i.named) {
            let var = self.inserted_var(graph, i)?;
            for (c, t) in var.columns() {
                b.names.push(c);
                b.texts.push(t.unwrap_or_default());
                b.types.push(DataType::varying(1000));
            }
            b.vars.push(var);
        }
        Ok(plan)
    }

    /// The variable of the element `i` inserts.
    fn inserted_var(&self, graph: &Graph, i: &Insert) -> Result<Var> {
        if graph.open {
            return Ok(Var { name: i.name.clone(), kind: Kind::Element { edge: i.kind == ElementKind::Edge, properties: None } });
        }
        let all = written_tables(self, graph, graph.closed, (i.kind, &i.labels))?;
        Ok(element_var(graph, &i.name, i.kind, &all))
    }

    /// The elements of `plan` inserted for `row` of `b`, of values
    /// `values`: its new variables' columns added to it.
    pub(super) fn insert_row_elements(
        &self,
        graph: &Graph,
        plan: &[Insert],
        (b, row): (&mut Binding, &mut Vec<Value>),
        values: &[Typed],
    ) -> Result<()> {
        let mut made: Vec<(String, String)> = Vec::new();
        for i in plan {
            let id_of = |n: &str, done: &[(String, String)]| {
                done.iter().find(|(m, _)| m == n).map(|(_, id)| id.clone()).or_else(|| b.id(row, n).map(str::to_owned))
            };
            let ends = i.ends.as_ref().map(|(s, d)| (id_of(s, &made).unwrap_or_default(), id_of(d, &made).unwrap_or_default()));
            let null = || (Value::Null, DataType::varying(1));
            let props: Vec<(String, Typed)> =
                i.props.iter().map(|(p, k)| (p.clone(), values.get(*k).cloned().unwrap_or_else(null))).collect();
            let (id, cols) = if graph.open {
                self.insert_open(graph, i, &props, ends.as_ref())?
            } else {
                self.insert_tabled(graph, i, &props, ends.as_ref())?
            };
            made.push((i.name.clone(), id));
            if i.named {
                pushed(b, row, cols);
            }
        }
        Ok(())
    }

    /// An element of an open graph inserted.
    fn insert_open(&self, graph: &Graph, element: &Insert, props: &[(String, Typed)], ends: Option<&(String, String)>) -> Result<Written> {
        let name = &graph.desc.name;
        let number = self.next_number(name)?;
        let id = identity_of(open_element(graph, element.kind)?, std::slice::from_ref(&number.0))?;
        let labels = if element.labels.is_empty() {
            LABEL_BAR.to_owned()
        } else {
            format!("{LABEL_BAR}{}{LABEL_BAR}", element.labels.join(LABEL_BAR))
        };
        let json = props_json(&Value::Null, &props.iter().map(|(k, (v, _))| (k.clone(), v.clone())).collect::<Vec<_>>())?;
        let labels = (Value::Char(labels.as_str().into()), DataType::varying(1000));
        let mut values = vec![number, labels.clone(), (json.clone(), DataType::Json)];
        let table = Session::open_table(name, element.kind);
        let sql = match ends {
            None => format!("INSERT INTO {table} (\"ID\", \"LABELS\", \"PROPS\") VALUES (GQL_P0, GQL_P1, GQL_P2)"),
            Some((s, d)) => {
                values.extend([id_value(s), id_value(d)]);
                format!(
                    "INSERT INTO {table} (\"ID\", \"LABELS\", \"PROPS\", \"SRC\", \"DST\", \"DIRECTED\") VALUES (GQL_P0, GQL_P1, GQL_P2, {}, {}, TRUE)",
                    key_value("GQL_P3", 0, "BIGINT"),
                    key_value("GQL_P4", 0, "BIGINT")
                )
            }
        };
        self.write(&sql, &values)?;
        Ok((id.clone(), vec![id_value(&id), labels, (json, DataType::Json)]))
    }

    /// An element of a graph over tables inserted.
    fn insert_tabled(&self, graph: &Graph, i: &Insert, props: &[(String, Typed)], ends: Option<&(String, String)>) -> Result<Written> {
        let all = written_tables(self, graph, graph.closed, (i.kind, &i.labels))?;
        let e = joining(graph, &all, ends)?;
        let mut cols = Vec::new();
        for (p, v) in props {
            cols.push((property_column(graph, graph.closed, e, p)?, typed_for(e, p, v)));
        }
        let ends = ends.map(|(s, d)| (s.as_str(), d.as_str()));
        let closed = graph.closed.then_some(&graph.desc.name);
        let key = self.insert_row(graph, e, (cols, ends), closed)?;
        let var = element_var(graph, &i.name, i.kind, &all);
        let q = qualified_identifier(&e.table);
        let (key_params, names) = params(&key);
        let at: Vec<String> = e.key.iter().zip(&names).map(|(k, p)| format!("{q}.{} = {p}", identifier(k))).collect();
        let mut select: Vec<String> = e.key.iter().map(|k| format!("{q}.{}", identifier(k))).collect();
        if let Kind::Element { properties: Some(declared), .. } = &var.kind {
            for (p, t) in declared {
                let value = e.property(p).map_or_else(|| "NULL".to_owned(), |d| format!("({})", d.expression));
                select.push(format!("CAST({value} AS {t})"));
            }
        }
        let sql = format!("SELECT {} FROM {q} WHERE {}", select.join(", "), at.join(" AND "));
        let (types, rows) = self.read_all(&sql, &key_params)?;
        let mut row = rows.into_iter().next().unwrap_or_default().into_iter().zip(types);
        let keys: Vec<Value> = row.by_ref().take(e.key.len()).map(|(v, _)| v).collect();
        let id = identity_of(e, &keys)?;
        let labels: Vec<&str> = e.labels.iter().map(|l| l.name.as_str()).collect();
        let set = format!("{LABEL_BAR}{}{LABEL_BAR}", labels.join(LABEL_BAR));
        let mut written = vec![id_value(&id), (Value::Char(set), DataType::varying(1000))];
        written.extend(row);
        Ok((id, written))
    }
}

/// The elements `path` inserts added to `plan`, `b` the binding table
/// and `l` the lowering that names anonymous ones; `value` the place of
/// the next property's value.
fn path_plan(path: &PathPattern, (b, l): (&Binding, &Lowering<'_>), plan: &mut Vec<Insert>, value: &mut usize) -> Result<()> {
    let mut nodes: Vec<String> = Vec::new();
    let mut edges: Vec<(usize, &EdgePattern)> = Vec::new();
    for f in path.expression.terms().iter().flat_map(|t| &t.factors) {
        let (p, kind) = match &f.primary {
            PathPrimary::Node(n) => (n, ElementKind::Vertex),
            PathPrimary::Edge(e) => {
                edges.push((nodes.len(), e));
                (&e.filler, ElementKind::Edge)
            }
            PathPrimary::Parenthesized(_) => {
                return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] INSERT writes vertices and edges, not a parenthesized path"));
            }
        };
        let (name, named) = p.variable.as_ref().map_or_else(|| (l.anonymous(), false), |v| (v.name.clone(), !v.anonymous));
        let bound = b.at(&name).is_some() || plan.iter().any(|i| i.name == name);
        let props = property_places(p, value);
        if kind == ElementKind::Vertex {
            nodes.push(name.clone());
            if bound {
                continue;
            }
        }
        plan.push(Insert { name, named, kind, labels: labels_of(p.label.as_ref())?, props, ends: None });
    }
    for (at, e) in edges {
        let (from, to) = (nodes.get(at.saturating_sub(1)).cloned().unwrap_or_default(), nodes.get(at).cloned().unwrap_or_default());
        let ends = if e.orientation == Orientation::Left { (to, from) } else { (from, to) };
        let name = e.filler.variable.as_ref().map(|v| v.name.clone());
        let unplaced = |i: &&mut Insert| i.kind == ElementKind::Edge && i.ends.is_none() && name.as_ref().is_none_or(|n| *n == i.name);
        if let Some(i) = plan.iter_mut().find(unplaced) {
            i.ends = Some(ends);
        }
    }
    Ok(())
}

/// The properties of `p`, each with the place of its value, counted on
/// from `value`.
fn property_places(p: &ElementPattern, value: &mut usize) -> Vec<(String, usize)> {
    let mut props = Vec::new();
    for s in &p.properties {
        props.push((s.name.clone(), *value));
        *value = value.saturating_add(1);
    }
    props
}

/// `cols` added to `row` of `b`, their types its columns'.
fn pushed(b: &mut Binding, row: &mut Vec<Value>, cols: Vec<Typed>) {
    for (v, t) in cols {
        row.push(v);
        if let Some(slot) = b.types.get_mut(row.len().saturating_sub(1)) {
            *slot = t;
        }
    }
}

/// Of the element tables `all` of one base table, the one an element is
/// written as: of an edge, the one that joins the tables of the vertices
/// `ends` identifies.
///
/// # Errors
///
/// SQLCODE -201 (GRF-0053) if no edge table joins them.
fn joining<'g>(graph: &'g Graph, all: &[&'g ElementTableDesc], ends: Option<&(String, String)>) -> Result<&'g ElementTableDesc> {
    let Some((s, d)) = ends else {
        return all.first().copied().ok_or_else(|| SqlError::new(Code::SyntaxRule, "[EXTENSION] GRF-0051 no element table"));
    };
    let (from, to) = (table_of(graph, s)?, table_of(graph, d)?);
    let joins =
        |e: &&&ElementTableDesc| e.endpoints.as_ref().is_some_and(|ep| ep.source.vertex == from.alias && ep.destination.vertex == to.alias);
    all.iter().find(joins).copied().ok_or_else(|| {
        SqlError::new(
            Code::Constraint,
            format!("[EXTENSION] GRF-0053 no edge table of graph {} joins {} to {}", graph.desc.name, from.alias, to.alias),
        )
    })
}

/// The labels of label expression `e` of an INSERT: one label, or labels
/// joined by "&".
///
/// # Errors
///
/// SQLCODE -103 of any other label expression.
fn labels_of(e: Option<&LabelExpression>) -> Result<Vec<String>> {
    let Some(e) = e else { return Ok(Vec::new()) };
    match e {
        LabelExpression::Label(l, _) => Ok(vec![l.clone()]),
        LabelExpression::Conjunction(a, b) => Ok([labels_of(Some(a))?, labels_of(Some(b))?].concat()),
        LabelExpression::Wildcard(at) => {
            Err(SqlError::at(Code::SyntaxRule, *at, "[EXTENSION] an inserted element's labels are labels joined by &"))
        }
        LabelExpression::Negation(_) | LabelExpression::Disjunction(_, _) => {
            Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] an inserted element's labels are labels joined by &"))
        }
    }
}
