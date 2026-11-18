//! \[EXTENSION\] ALTER PROPERTY GRAPH (§4.1): element tables added and
//! dropped, labels and properties added and dropped, and COMPILE. Each
//! action changes the graph's definition, read again from its definition
//! text, and the graph is described whole again from it, so every rule of
//! §4.1 holds of what it leaves; an INVALID graph so described is valid
//! again. A view that reads the graph and binds no more after it is
//! dropped with CASCADE, and refuses the action under RESTRICT.

use super::super::definitions::{Defined, in_schema};
use super::super::{State, rule};
use super::{Describing, graph_definition, graph_rule};
use crate::ast::{
    AlterGraph, AlterGraphAction, DropBehavior, ElementAction, ElementDefinition, GraphDefinition, LabelDefinition, PropertiesDefinition,
};
use crate::binder::{qualify, view_degree};
use crate::catalog::QualifiedName;
use crate::error::{At, Result};

impl State {
    /// ALTER PROPERTY GRAPH `a`, its names in schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is no such graph; (GRF-0012) if it
    /// names an element table, label or property the graph has not; as
    /// `describe_graph`'s, of the graph it leaves; RESTRICT, if a view that
    /// reads it binds no more.
    pub(in crate::db) fn alter_graph(&mut self, schema: &str, a: &AlterGraph) -> Result<Defined> {
        let qn = qualify(&a.name, schema);
        if super::native::is_sys_graph(&qn) {
            return Err(super::native::sys_graph_refused(&qn)).or_at(a.name.at);
        }
        let desc = self.graph_named(&qn).or_at(a.name.at)?;
        let mut def = graph_definition(&desc.definition())?;
        let behavior = match &a.action {
            AlterGraphAction::Compile => DropBehavior::Restrict,
            AlterGraphAction::Add { edges, tables } => {
                let to = if *edges { &mut def.edges } else { &mut def.vertices };
                to.extend(tables.iter().cloned());
                DropBehavior::Restrict
            }
            AlterGraphAction::Drop { edges, aliases, behavior } => {
                dropped_tables(&mut def, *edges, aliases, *behavior)?;
                *behavior
            }
            AlterGraphAction::Element { edge, alias, action } => {
                let tables = if *edge { &mut def.edges } else { &mut def.vertices };
                let Some(e) = tables.iter_mut().find(|e| e.alias_name() == alias.0) else {
                    let kind = if *edge { "edge" } else { "vertex" };
                    return Err(graph_rule(Some("GRF-0012"), format!("graph {qn} has no {kind} table {}", alias.0))).or_at(alias.1);
                };
                element_altered(e, action)?
            }
        };
        let unplaced = |e: crate::error::SqlError| crate::error::SqlError { pos: None, ..e };
        let again = self.describe_graph(&def, Describing { names: &desc.names, horizon: None }).map_err(unplaced).or_at(a.name.at)?;
        self.put_graph(crate::catalog::GraphDesc { select: desc.select.clone(), ..again });
        self.graph_grants_standing(&qn);
        let reads = self.catalog.graph(&qn).map(|g| g.reads.clone()).unwrap_or_default();
        let mut defined = Defined { locks: in_schema(&qn, reads), ..Defined::default() };
        self.graph_readers_rebound(&qn, behavior, &mut defined)?;
        Ok(defined)
    }

    /// The views that read graph `qn` and bind no more as it is now,
    /// dropped with what reads them if `behavior` is CASCADE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 with RESTRICT if there is one.
    fn graph_readers_rebound(&mut self, qn: &QualifiedName, behavior: DropBehavior, defined: &mut Defined) -> Result<()> {
        let broken: Vec<QualifiedName> =
            (self.views_reading(qn).into_iter()).filter(|v| self.catalog.view(v).is_some_and(|d| view_degree(self, d).is_err())).collect();
        if let (Some(view), DropBehavior::Restrict) = (broken.first(), behavior) {
            return Err(rule(format!(
                "[EXTENSION] view {view} reads property graph {qn} and binds no more, so it is altered only with CASCADE"
            )));
        }
        for view in broken {
            if self.catalog.view(&view).is_some() {
                defined.dropped.absorb(self.drop_view(&view, true)?);
                defined.locks.push((view, crate::txn::LockMode::Exclusive));
            }
        }
        Ok(())
    }
}

/// DROP VERTEX TABLES or DROP EDGE TABLES of `aliases` from `def`: of
/// vertex tables, the edge tables that reference one with them if
/// `behavior` is CASCADE.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if an alias is of no such table of the graph;
/// RESTRICT, if an edge table references a vertex table dropped; if no
/// vertex table would be left.
fn dropped_tables(def: &mut GraphDefinition, edges: bool, aliases: &[(String, usize)], behavior: DropBehavior) -> Result<()> {
    let tables = if edges { &mut def.edges } else { &mut def.vertices };
    for (alias, at) in aliases {
        if !tables.iter().any(|e| e.alias_name() == alias) {
            let kind = if edges { "edge" } else { "vertex" };
            return Err(graph_rule(Some("GRF-0012"), format!("the graph has no {kind} table {alias}"))).or_at(*at);
        }
    }
    tables.retain(|e| !aliases.iter().any(|(a, _)| e.alias_name() == a));
    if edges {
        return Ok(());
    }
    if def.vertices.is_empty() {
        return Err(graph_rule(None, "a graph has one vertex table at least, so its last is not dropped".to_owned()));
    }
    let references = |e: &ElementDefinition| {
        [&e.source, &e.destination].into_iter().flatten().any(|end| aliases.iter().any(|(a, _)| *a == end.vertex.0))
    };
    if let (Some(edge), DropBehavior::Restrict) = (def.edges.iter().find(|e| references(e)), behavior) {
        let msg = format!("edge table {} references a vertex table dropped, so it is dropped only with CASCADE", edge.alias_name());
        return Err(graph_rule(None, msg));
    }
    def.edges.retain(|e| !references(e));
    Ok(())
}

/// Element table `e` as `action` alters it: the `<drop behavior>` it
/// writes, RESTRICT if none.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if it names a label or property the element
/// table has not; if it drops its last label.
fn element_altered(e: &mut ElementDefinition, action: &ElementAction) -> Result<DropBehavior> {
    let alias = e.alias_name().to_owned();
    let unknown =
        |(label, at): &(String, usize)| graph_rule(Some("GRF-0012"), format!("element table {alias} has no label {label}")).or_at(*at);
    match action {
        ElementAction::AddLabel(l) => {
            e.labels.push(l.clone());
            Ok(DropBehavior::Restrict)
        }
        ElementAction::DropLabel { label, behavior } => {
            let before = e.labels.len();
            e.labels.retain(|l| l.name.as_deref() != Some(label.0.as_str()));
            if e.labels.len() == before {
                return Err(unknown(label));
            }
            if e.labels.is_empty() {
                return Err(graph_rule(None, format!("element table {alias} has one label at least, so its last is not dropped")))
                    .or_at(label.1);
            }
            Ok(*behavior)
        }
        ElementAction::AddProperties { label, properties } => {
            let l = labeled(e, &label.0).ok_or_else(|| unknown(label))?;
            match &mut l.properties {
                Some(PropertiesDefinition::Listed(list)) => list.extend(properties.iter().cloned()),
                Some(PropertiesDefinition::None | PropertiesDefinition::AllColumns(_)) | None => {
                    l.properties = Some(PropertiesDefinition::Listed(properties.clone()));
                }
            }
            Ok(DropBehavior::Restrict)
        }
        ElementAction::DropProperties { label, names, behavior } => {
            let l = labeled(e, &label.0).ok_or_else(|| unknown(label))?;
            let Some(PropertiesDefinition::Listed(list)) = &mut l.properties else {
                return Err(graph_rule(Some("GRF-0012"), format!("label {} of {alias} has no property {}", label.0, names.join(", "))))
                    .or_at(label.1);
            };
            if let Some(name) = names.iter().find(|n| !list.iter().any(|p| p.name.as_ref() == Some(n))) {
                return Err(graph_rule(Some("GRF-0012"), format!("label {} of {alias} has no property {name}", label.0))).or_at(label.1);
            }
            list.retain(|p| !names.iter().any(|n| p.name.as_ref() == Some(n)));
            if list.is_empty() {
                l.properties = Some(PropertiesDefinition::None);
            }
            Ok(*behavior)
        }
    }
}

/// The label `name` of element table `e`, as written.
fn labeled<'a>(e: &'a mut ElementDefinition, name: &str) -> Option<&'a mut LabelDefinition> {
    e.labels.iter_mut().find(|l| l.name.as_deref() == Some(name))
}
