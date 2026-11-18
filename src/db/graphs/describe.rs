//! \[EXTENSION\] A property graph's descriptor from its definition
//! (RULES.md D-GRAPH-CATALOG), each rule of §4.1 checked: its element
//! tables usable and their aliases unique, each key and endpoint found or
//! written (R4.2) and held as its mode says (R4.3), and its labels and
//! properties (R4.4 to R4.6). A graph altered or compiled is described
//! again from its definition text (`GraphDesc::definition`), so one set of
//! rules holds of every graph however it came to be.

use std::cell::RefCell;

use super::elements::Element;
use super::graph_rule;
use crate::ast::{ElementDefinition, GraphDefinition, GraphModeOption};
use crate::binder::{PropertyBinding, property_type, qualify};
use crate::catalog::{ElementKind, ElementTableDesc, GraphDesc, GraphMode, Horizon, QualifiedName, WHOLE, owner_of};
use crate::db::State;
use crate::error::{At, Result};
use crate::parser::Parser;

/// How a graph's definition is described: the schema of the names it
/// writes without one, and, of a graph described again to see that it
/// still holds, what its expressions saw as it was defined.
#[derive(Clone, Copy)]
pub(in crate::db) struct Describing<'a> {
    /// The schema of names written without one.
    pub(in crate::db) names: &'a str,
    /// The horizon it was defined with; none of a graph defined, altered
    /// or compiled, whose expressions bind with the routines there are.
    pub(in crate::db) horizon: Option<&'a Horizon>,
}

impl State {
    /// The descriptor of the graph `def` defines, as `how` says: valid,
    /// with no privilege granted on it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it breaks a rule of §4.1, each as §22 names it;
    /// -104 if the owner of its schema holds no SELECT on an element table,
    /// or a privilege a property expression needs.
    pub(in crate::db) fn describe_graph(&self, def: &GraphDefinition, how: Describing<'_>) -> Result<GraphDesc> {
        let name = qualify(&def.name, how.names);
        let owner = owner_of(&*self.catalog, &name.auth).to_owned();
        let invoked = RefCell::new(Vec::new());
        let binding = PropertyBinding {
            owner: &owner,
            names: how.names,
            horizon: how.horizon.unwrap_or(&WHOLE),
            invoked: if how.horizon.is_none() { Some(&invoked) } else { None },
        };
        let mode = match def.mode {
            None | Some(GraphModeOption::Enforced) => GraphMode::Enforced,
            Some(GraphModeOption::Trusted) => GraphMode::Trusted,
        };
        let mut elements: Vec<ElementTableDesc> = Vec::new();
        let mut reads: Vec<QualifiedName> = Vec::new();
        let written = def.vertices.iter().map(|e| (e, ElementKind::Vertex)).chain(def.edges.iter().map(|e| (e, ElementKind::Edge)));
        for (e, kind) in written {
            unique_alias(&elements, e)?;
            let element = Element { binding, mode, kind, vertices: &elements };
            let (desc, read) = self.element_described(element, e)?;
            for t in read {
                if !reads.contains(&t) {
                    reads.push(t);
                }
            }
            elements.push(desc);
        }
        shared_labels(&elements)?;
        let horizon = how.horizon.cloned().unwrap_or_else(|| Horizon::default().invoking(invoked.take()));
        let names = how.names.to_owned();
        Ok(GraphDesc { name, elements, mode, invoker: def.invoker, names, reads, horizon, valid: true, select: Vec::new() })
    }

    /// The graph `desc` described again from its definition text, as it
    /// was defined if `fixed`, else with the routines there are now: its
    /// grants kept, valid as it was.
    ///
    /// # Errors
    ///
    /// As `describe_graph`'s, at no place in a statement.
    pub(in crate::db) fn redescribe_graph(&self, desc: &GraphDesc, fixed: bool) -> Result<GraphDesc> {
        let unplaced = |e: crate::error::SqlError| crate::error::SqlError { pos: None, ..e };
        let def = graph_definition(&desc.definition()).map_err(unplaced)?;
        let how = Describing { names: &desc.names, horizon: if fixed { Some(&desc.horizon) } else { None } };
        let again = self.describe_graph(&def, how).map_err(unplaced)?;
        Ok(GraphDesc { select: desc.select.clone(), valid: desc.valid, ..again })
    }
}

/// The definition the text `sql` of a CREATE PROPERTY GRAPH statement
/// writes.
///
/// # Errors
///
/// SQLCODE -102 if it does not parse as one.
pub(in crate::db) fn graph_definition(sql: &str) -> Result<GraphDefinition> {
    Parser::for_stored(sql)?.graph_definition()
}

/// That no element table before has the alias of `e`.
///
/// # Errors
///
/// SQLCODE -103 if one has.
fn unique_alias(before: &[ElementTableDesc], e: &ElementDefinition) -> Result<()> {
    let alias = e.alias_name();
    if before.iter().any(|d| d.alias == alias) {
        let msg = format!("the graph has element table alias {alias} already, so give each element table its own with AS (§4.1)");
        return Err(graph_rule(None, msg)).or_at(e.alias_at());
    }
    Ok(())
}

/// [ISO/IEC 9075:1992 §9.3] That each label on more than one element table
/// has the same property names on each, of union-compatible types
/// (R4.4, R5.6).
///
/// # Errors
///
/// SQLCODE -103 (GRF-0011) if not.
fn shared_labels(elements: &[ElementTableDesc]) -> Result<()> {
    let mut labels: Vec<&str> = Vec::new();
    for l in elements.iter().flat_map(|e| &e.labels) {
        if !labels.contains(&l.name.as_str()) {
            labels.push(&l.name);
        }
    }
    for label in labels {
        let on: Vec<&ElementTableDesc> = elements.iter().filter(|e| e.has_label(label)).collect();
        let Some((alias, first)) = on.first().and_then(|e| Some((&e.alias, e.label(label)?))) else { continue };
        let names: Vec<&str> = first.properties.iter().map(|p| p.name.as_str()).collect();
        for e in &on {
            let mut theirs: Vec<&str> = e.label(label).map(|l| l.properties.iter().map(|p| p.name.as_str()).collect()).unwrap_or_default();
            let mut ours = names.clone();
            theirs.sort_unstable();
            ours.sort_unstable();
            if theirs != ours {
                let msg = format!("label {label} has other properties on element table {} than on {alias} (R4.4)", e.alias);
                return Err(graph_rule(Some("GRF-0011"), msg));
            }
        }
        for name in &names {
            let types: Vec<_> = on.iter().filter_map(|e| e.property(name)).map(|p| p.data_type.clone()).collect();
            if let Err(e) = property_type(&types) {
                let msg = format!("property {name} of label {label} is of types no union takes (R5.6), as {}", e.msg);
                return Err(graph_rule(Some("GRF-0011"), msg));
            }
        }
    }
    Ok(())
}
