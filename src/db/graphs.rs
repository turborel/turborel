//! \[EXTENSION\] Property graphs over tables (RULES.md D106),
//! Mode A of the property graph implementation standard (§4.1): CREATE,
//! ALTER and DROP PROPERTY GRAPH, in a transaction as a view's statements
//! are (`db/definitions.rs`, `db/drop.rs`).
//!
//! A graph's name is of the namespace of its schema's tables and views. It
//! depends on what it reads as a view does: DROP of a table or view it
//! reads, or of a routine its properties invoke, is refused under
//! RESTRICT and drops it under CASCADE, as is a revoke that leaves its
//! owner without SELECT on one. A change of a table it reads that breaks
//! it otherwise marks it INVALID until ALTER PROPERTY GRAPH ... COMPILE
//! finds it holds again (R4.7, `graphs/dependents.rs`).

mod adjacency;
mod alter;
mod dependents;
mod describe;
mod elements;
pub(in crate::db) mod gql;
pub(in crate::db) mod native;
mod privileges;
mod property;

use std::sync::Arc;

pub(in crate::db) use describe::{Describing, graph_definition};

use super::ddl::Dropped;
use super::definitions::{Defined, in_schema};
use super::{State, rule};
use crate::ast::GraphDefinition;
use crate::binder::{qualify, view_degree};
use crate::catalog::{GraphDesc, QualifiedName};
use crate::error::{At, Code, Result, SqlError, SqlState};

/// The Syntax Rule error of a property graph: \[EXTENSION\], the condition
/// of §22 it is if any, then `msg`.
pub(in crate::db) fn graph_rule(condition: Option<&str>, msg: impl Into<String>) -> SqlError {
    let msg: String = msg.into();
    let text = match condition {
        Some(c) => format!("[EXTENSION] {c} {msg}"),
        None => format!("[EXTENSION] {msg}"),
    };
    SqlError::new(Code::SyntaxRule, text)
}

impl State {
    /// CREATE PROPERTY GRAPH `def`, its names in schema `schema`: a graph
    /// of the name there already replaced if OR REPLACE, its privileges
    /// kept, or left as it is if IF NOT EXISTS.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no schema of its name (SQLSTATE 3F000), or
    /// it has a table, view or graph of its name; as `describe_graph`'s; of
    /// a graph replaced, if a view that reads it binds no more.
    pub(in crate::db) fn create_graph(&mut self, schema: &str, def: &GraphDefinition) -> Result<Defined> {
        let qn = qualify(&def.name, schema);
        if native::is_sys_graph(&qn) {
            return Err(native::sys_graph_refused(&qn)).or_at(def.name.at);
        }
        let earlier = self.catalog.graph(&qn).cloned();
        if earlier.is_some() && def.if_not_exists {
            return Ok(Defined::default());
        }
        if earlier.is_none() || !def.replace {
            self.new_graph_name(&qn).or_at(def.name.at)?;
        }
        let mut desc = self.describe_graph(def, Describing { names: schema, horizon: None })?;
        if let Some(before) = earlier {
            desc.select = before.select;
        }
        self.put_graph(desc);
        self.graph_grants_standing(&qn);
        // [§11.20 SR3, by analogy] What reads it binds still.
        if let Some(view) =
            self.views_reading(&qn).into_iter().find(|v| self.catalog.view(v).is_some_and(|v| view_degree(self, v).is_err()))
        {
            let msg = format!("view {view} reads property graph {qn} and binds no more as it is replaced");
            return Err(graph_rule(None, msg)).or_at(def.name.at);
        }
        let reads = self.catalog.graph(&qn).map(|g| g.reads.clone()).unwrap_or_default();
        Ok(Defined { locks: in_schema(&qn, reads), ..Defined::default() })
    }

    /// That `qn` may name a graph defined now: its schema exists, and has
    /// no table, view or graph of its name.
    fn new_graph_name(&self, qn: &QualifiedName) -> Result<()> {
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[EXTENSION] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let taken = self.catalog.table(qn).is_some() || self.catalog.view(qn).is_some() || self.catalog.graph(qn).is_some();
        if taken {
            let msg = format!("schema {} has a table, view or property graph {} already, whose names are one namespace", qn.auth, qn.ident);
            return Err(graph_rule(None, msg));
        }
        Ok(())
    }

    /// `desc`, recorded.
    pub(in crate::db) fn put_graph(&mut self, desc: GraphDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_graph(desc);
        self.catalog = Arc::from(catalog);
    }

    /// The graph `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is none.
    pub(in crate::db) fn graph_named(&self, qn: &QualifiedName) -> Result<GraphDesc> {
        let found = self.catalog.graph(qn).cloned();
        found.ok_or_else(|| graph_rule(Some("GRF-0001"), format!("there is no property graph {qn}")))
    }

    /// DROP PROPERTY GRAPH `qn`, with the views that read it, and what
    /// reads them, if `cascade`, and the privileges on it: what it took
    /// away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is no such graph, or, not
    /// `cascade`, a view or another definition reads it.
    pub(in crate::db) fn drop_graph(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        self.graph_named(qn)?;
        let mut dropped = self.drop_dependents(qn, cascade, "§11.20 SR3")?;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_graph(qn);
        self.catalog = Arc::from(catalog);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// The graphs that read one of `gone`, tables and views dropped,
    /// dropped with them if `cascade`, as DROP PROPERTY GRAPH ... CASCADE
    /// drops them: what was taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one reads one, as the Syntax Rule
    /// `cited` of a view that reads it says.
    pub(in crate::db) fn drop_graphing(&mut self, gone: &[&QualifiedName], (cascade, cited): (bool, &str)) -> Result<Dropped> {
        // [EXTENSION] (§4.2) SYS_GRAPH is described anew instead.
        let reading: Vec<(QualifiedName, QualifiedName)> = (self.catalog.graphs().into_iter())
            .filter(|g| !native::is_sys_graph(&g.name))
            .filter_map(|g| g.reads.iter().find(|r| gone.contains(r)).map(|r| (g.name.clone(), r.clone())))
            .collect();
        if let (Some((graph, read)), false) = (reading.first(), cascade) {
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 {cited}] {read} is read by property graph {graph}, so it is dropped only with CASCADE"
            )));
        }
        let mut dropped = Dropped::default();
        for (graph, _) in reading {
            if self.catalog.graph(&graph).is_some() {
                dropped.absorb(self.drop_graph(&graph, true)?);
            }
        }
        Ok(dropped)
    }

    /// The names of the graphs of schema `auth`.
    pub(in crate::db) fn schema_graphs(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.graphs().into_iter().filter(|g| g.name.auth == auth).map(|g| g.name.clone()).collect()
    }

    /// The graphs `graphs` dropped, as DROP PROPERTY GRAPH ... CASCADE
    /// drops them, of those still there: what was taken away.
    ///
    /// # Errors
    ///
    /// As `drop_graph`'s.
    pub(in crate::db) fn drop_graphs(&mut self, graphs: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for g in graphs {
            if self.catalog.graph(g).is_some() {
                dropped.absorb(self.drop_graph(g, true)?);
            }
        }
        Ok(dropped)
    }
}

impl State {
    /// The graphs of `<schema definition>` `s`, made once its tables,
    /// views and routines are, then the grants of SELECT on them by
    /// `owner`: whether one was not granted.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a graph names another schema; as `create_graph`'s
    /// and `grant_on_graph`'s.
    pub(in crate::db) fn schema_graphs_made(&mut self, s: &crate::ast::Schema, owner: &str) -> Result<bool> {
        for g in &s.graphs {
            if let Some(other) = g.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!(
                    "[ISO/IEC 9075:1992 §11.1 SR5, by analogy] property graph {} is in schema {other}, not {}",
                    g.name.ident, s.name
                );
                return Err(rule(msg)).or_at(g.name.at);
            }
            self.create_graph(&s.name, g)?;
        }
        let mut warned = false;
        for p in s.privileges.iter().filter(|p| p.object == crate::ast::PrivilegeObject::Graph) {
            warned |= self.grant_on_graph(owner, &s.name, p).or_at(p.table.at)?.completion != super::Completion::Success;
        }
        Ok(warned)
    }
}
