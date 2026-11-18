//! \[EXTENSION\] SET, REMOVE and DELETE of GQL (graph standard §12.3,
//! RULES.md D-GQL), of one row of the binding table: each element's
//! table and row found from its identity.
//!
//! A property of a graph over tables is its column, set or made null; a
//! label is its table's, so SET adds, and REMOVE takes away, only a label
//! it has, or has not (GRF-0051, GRF-0053). An open graph's element's
//! labels and properties are its text and its JSON object, rewritten.
//! DELETE of a vertex with edges fails (R12.1, GRF-0052) unless DETACH,
//! which deletes the edges first.

use super::identities::{key_value, table_of};
use super::modify::Binding;
use super::writes::{Typed, id_value, params, property_column, props_json, refused, relabeled, typed_for};
use crate::ast::{GqlRemoveItem, GqlSetItem};
use crate::binder::gql::Graph;
use crate::catalog::{ElementKind, ElementTableDesc, qualified_identifier};
use crate::db::Session;
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value};

/// The condition on an open graph's row, column `column`, of the element
/// of identity parameter `param`.
fn open_row(column: &str, param: &str) -> String {
    format!("\"{column}\" = {}", key_value(param, 0, "BIGINT"))
}

impl Session {
    /// SET `items` of `row`, the values they write `values`.
    pub(super) fn set_items(
        &self,
        graph: &Graph,
        items: &[GqlSetItem],
        (b, row): (&Binding, &mut [Value]),
        values: &[Typed],
    ) -> Result<()> {
        let mut next = values.iter();
        for item in items {
            match item {
                GqlSetItem::Property { variable, property, .. } => {
                    let v = next.next().cloned().unwrap_or((Value::Null, DataType::varying(1)));
                    self.set_property(graph, (b, row), variable, (&[(property.clone(), v)], false))?;
                }
                GqlSetItem::All { variable, properties, .. } => {
                    let all: Vec<(String, Typed)> = properties
                        .iter()
                        .map(|(p, _)| (p.clone(), next.next().cloned().unwrap_or((Value::Null, DataType::varying(1)))))
                        .collect();
                    self.set_property(graph, (b, row), variable, (&all, true))?;
                }
                GqlSetItem::Label { variable, label, .. } => self.set_label(graph, (b, row), (variable, label), true)?,
            }
        }
        Ok(())
    }

    /// REMOVE `items` of `row`.
    pub(super) fn remove_items(&self, graph: &Graph, items: &[GqlRemoveItem], (b, row): (&Binding, &mut [Value])) -> Result<()> {
        for item in items {
            match item {
                GqlRemoveItem::Property(variable, property, _) => {
                    self.set_property(graph, (b, row), variable, (&[(property.clone(), (Value::Null, DataType::varying(1)))], false))?;
                }
                GqlRemoveItem::Label(variable, label, _) => self.set_label(graph, (b, row), (variable, label), false)?,
            }
        }
        Ok(())
    }

    /// Sets `props` of the element of `variable` in `row`, every other
    /// property made null too if `all`.
    fn set_property(
        &self,
        graph: &Graph,
        (b, row): (&Binding, &mut [Value]),
        variable: &str,
        (props, all): (&[(String, Typed)], bool),
    ) -> Result<()> {
        let Some(id) = b.id(row, variable).map(str::to_owned) else { return Ok(()) };
        if graph.open {
            let column = format!("{variable}$PROPS");
            let at = b.at(&column);
            let base = if all { Value::Null } else { at.and_then(|i| row.get(i)).cloned().unwrap_or(Value::Null) };
            let json = props_json(&base, &props.iter().map(|(k, (v, _))| (k.clone(), v.clone())).collect::<Vec<_>>())?;
            let table = Session::open_table(&graph.desc.name, kind_of(b, variable));
            self.write(
                &format!("UPDATE {table} SET \"PROPS\" = GQL_P0 WHERE {}", open_row("ID", "GQL_P1")),
                &[(json.clone(), DataType::Json), id_value(&id)],
            )?;
            if let Some(slot) = at.and_then(|i| row.get_mut(i)) {
                *slot = json;
            }
            return Ok(());
        }
        let e = table_of(graph, &id)?;
        let mut sets: Vec<(String, Typed, Option<String>)> = Vec::new();
        for (p, v) in props {
            sets.push((property_column(graph, graph.closed, e, p)?, typed_for(e, p, v), Some(p.clone())));
        }
        if all {
            for p in e.properties().into_iter().filter(|p| !props.iter().any(|(n, _)| *n == p.name)) {
                let column = property_column(graph, graph.closed, e, &p.name)?;
                sets.push((column, (Value::Null, p.data_type.clone()), Some(p.name.clone())));
            }
        }
        self.update_row(e, &id, &sets)?;
        for (_, (v, _), p) in sets {
            let at = p.and_then(|p| b.at(&format!("{variable}.{p}")));
            if let Some(slot) = at.and_then(|i| row.get_mut(i)) {
                *slot = v;
            }
        }
        Ok(())
    }

    /// Updates the row of `e` of identity `id`: each column of `sets` to
    /// its value.
    fn update_row(&self, e: &ElementTableDesc, id: &str, sets: &[(String, Typed, Option<String>)]) -> Result<()> {
        if sets.is_empty() {
            return Ok(());
        }
        let mut values: Vec<Typed> = sets.iter().map(|(_, v, _)| v.clone()).collect();
        values.push(id_value(id));
        let (_, names) = params(&values);
        let assigned: Vec<String> =
            sets.iter().zip(&names).map(|((c, _, _), p)| format!("{} = {p}", crate::catalog::identifier(c))).collect();
        let q = qualified_identifier(&e.table);
        let at = self.keyed(e, &q, names.last().map_or("", String::as_str))?;
        self.write(&format!("UPDATE {q} SET {} WHERE {at}", assigned.join(", ")), &values)
    }

    /// Adds, if `add`, or takes away label `label` of the element of
    /// `variable` in `row`.
    fn set_label(&self, graph: &Graph, (b, row): (&Binding, &mut [Value]), (variable, label): (&str, &str), add: bool) -> Result<()> {
        let Some(id) = b.id(row, variable).map(str::to_owned) else { return Ok(()) };
        if !graph.open {
            let e = table_of(graph, &id)?;
            if e.has_label(label) == add {
                return Ok(());
            }
            let what = if add { "adds" } else { "takes away" };
            return Err(refused(
                graph,
                graph.closed,
                &format!("an element of {} {what} no label {label}, its labels being its table's", e.alias),
            ));
        }
        let column = format!("{variable}$LABELS");
        let at = b.at(&column);
        let mut now = String::new();
        if let Some(Value::Char(s)) = at.and_then(|i| row.get(i)) {
            now.clone_from(s);
        }
        let labels = relabeled(&now, label, add);
        let table = Session::open_table(&graph.desc.name, kind_of(b, variable));
        let value = (Value::Char(labels.as_str().into()), DataType::varying(1000));
        let row_at = open_row("ID", "GQL_P1");
        self.write(&format!("UPDATE {table} SET \"LABELS\" = GQL_P0 WHERE {row_at}"), &[value.clone(), id_value(&id)])?;
        if let Some(slot) = at.and_then(|i| row.get_mut(i)) {
            *slot = value.0;
        }
        Ok(())
    }

    /// DELETE, or DETACH DELETE if `detach`, of the elements of `items`
    /// in `row`.
    pub(super) fn delete_items(
        &self,
        graph: &Graph,
        (detach, items): (bool, &[(String, usize)]),
        b: &Binding,
        row: &[Value],
    ) -> Result<()> {
        for (variable, at) in items {
            if b.at(variable).is_none() {
                return Err(SqlError::at(Code::SyntaxRule, *at, format!("[EXTENSION] variable {variable} is not in scope")));
            }
            let Some(id) = b.id(row, variable).map(str::to_owned) else { continue };
            if graph.open {
                self.delete_open(graph, &id, kind_of(b, variable), detach)?;
            } else {
                self.delete_tabled(graph, &id, detach)?;
            }
        }
        Ok(())
    }

    /// Deletes the open graph's element `id`.
    fn delete_open(&self, graph: &Graph, id: &str, kind: ElementKind, detach: bool) -> Result<()> {
        let g = &graph.desc.name;
        if kind == ElementKind::Vertex {
            let edges = Session::open_table(g, ElementKind::Edge);
            let incident = format!("{} OR {}", open_row("SRC", "GQL_P0"), open_row("DST", "GQL_P0"));
            self.detached(&edges, &incident, (id, detach))?;
        }
        let table = Session::open_table(g, kind);
        self.write(&format!("DELETE FROM {table} WHERE {}", open_row("ID", "GQL_P0")), &[id_value(id)])
    }

    /// Deletes the element `id` of a graph over tables, a vertex's edges
    /// first if `detach`.
    fn delete_tabled(&self, graph: &Graph, id: &str, detach: bool) -> Result<()> {
        let e = table_of(graph, id)?;
        if e.kind == ElementKind::Vertex {
            for edge in graph.desc.edges() {
                let Some(ends) = &edge.endpoints else { continue };
                let q = qualified_identifier(&edge.table);
                let mut incident = Vec::new();
                for end in [&ends.source, &ends.destination].into_iter().filter(|x| x.vertex == e.alias) {
                    incident.push(self.ends_at((e, end), &q, "GQL_P0")?);
                }
                if !incident.is_empty() {
                    self.detached(&q, &incident.join(" OR "), (id, detach))?;
                }
            }
        }
        let q = qualified_identifier(&e.table);
        self.write(&format!("DELETE FROM {q} WHERE {}", self.keyed(e, &q, "GQL_P0")?), &[id_value(id)])
    }

    /// The edges of `table` `incident` says meet vertex `id`: deleted if
    /// `detach`, else none may be (R12.1).
    fn detached(&self, table: &str, incident: &str, (id, detach): (&str, bool)) -> Result<()> {
        if detach {
            return self.write(&format!("DELETE FROM {table} WHERE {incident}"), &[id_value(id)]);
        }
        let (p, _) = params(&[id_value(id)]);
        let (_, rows) = self.read_all(&format!("SELECT COUNT(*) FROM {table} WHERE {incident}"), &p)?;
        let count = rows.into_iter().next().and_then(|r| r.into_iter().next()).map_or_else(String::new, |v| v.to_string());
        if count.trim() != "0" {
            return Err(SqlError::new(
                Code::Constraint,
                format!("[EXTENSION] GRF-0052 vertex {id} has edges, so DELETE does not delete it, but DETACH DELETE deletes them too"),
            ));
        }
        Ok(())
    }
}

/// Whether `variable` of `b` is an edge's or a vertex's.
fn kind_of(b: &Binding, variable: &str) -> ElementKind {
    if b.is_edge(variable) { ElementKind::Edge } else { ElementKind::Vertex }
}
