//! \[EXTENSION\] [GS §13.2] Adjacency indexes of a property graph's edge
//! tables: CREATE ADJACENCY INDEX ON GRAPH, which names an edge table by
//! its alias in the graph and is an index of the adjacency method of its
//! ends' columns (`index/adjacency.rs`), its source's first; and the index
//! an ENFORCED graph's base edge tables are given as it is created, if
//! none already keeps their ends.

use std::sync::Arc;

use super::graph_rule;
use crate::ast::{AdjacencyIndex, IndexStatement, Literal, OptionValue, TableName, TableOptionValue, TableStorage};
use crate::binder::qualify;
use crate::catalog::{ElementKind, GraphMode, QualifiedName};
use crate::db::{Session, State};
use crate::error::{At, Result};
use crate::value::Decimal;

/// The option of an adjacency index of its source's columns.
const SOURCE_COLUMNS: &str = "ADJACENCY.SOURCE_COLUMNS";
/// The option of an adjacency index of its included columns.
const INCLUDE: &str = "ADJACENCY.INCLUDE";
/// The option of an adjacency index of the ways it keeps.
const DIRECTION: &str = "ADJACENCY.DIRECTION";
/// The adjacency index method.
const ADJACENCY: &str = "adjacency";

/// An option of `name` of the whole number `n`.
fn counted(name: &str, n: usize) -> TableOptionValue {
    let value = Literal::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0));
    TableOptionValue { name: name.to_owned(), at: 0, value: Some(OptionValue::Literal(value)) }
}

/// The way `direction` keeps edges, as the adjacency method's option
/// names it: FORWARD by source, OUT; REVERSE by destination, IN.
fn kept(direction: Option<&str>) -> &'static str {
    match direction {
        Some("FORWARD") => "OUT",
        Some("REVERSE") => "IN",
        Some(_) | None => "BOTH",
    }
}

impl Session {
    /// \[GS §13.2\] CREATE ADJACENCY INDEX `stmt` as the CREATE INDEX of
    /// the adjacency method it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is no such graph, (GRF-0012) if it
    /// has no such edge table.
    pub(in crate::db) fn adjacency_create(&self, st: &State, stmt: &IndexStatement) -> Result<IndexStatement> {
        let IndexStatement::Adjacency(a) = stmt else { return Ok(stmt.clone()) };
        let AdjacencyIndex { name, graph, edge: (alias, alias_at), direction, include } = &**a;
        let qn = qualify(graph, &self.default_schema());
        let desc = st
            .catalog
            .graph(&qn)
            .ok_or_else(|| graph_rule(Some("GRF-0001"), format!("there is no property graph {qn}")))
            .or_at(graph.at)?;
        let element = desc.element(alias).filter(|e| e.kind == ElementKind::Edge);
        let element =
            element.ok_or_else(|| graph_rule(Some("GRF-0012"), format!("graph {qn} has no edge table {alias}"))).or_at(*alias_at)?;
        let Some(ends) = &element.endpoints else { return Err(graph_rule(Some("GRF-0012"), format!("edge table {alias} has no ends"))) };
        let mut columns: Vec<(String, usize)> =
            ends.source.columns.iter().chain(&ends.destination.columns).map(|c| (c.clone(), 0)).collect();
        columns.extend(include.iter().cloned());
        let options = vec![
            counted(SOURCE_COLUMNS, ends.source.columns.len()),
            counted(INCLUDE, include.len()),
            TableOptionValue { name: DIRECTION.to_owned(), at: 0, value: Some(OptionValue::Word(kept(direction.as_deref()).to_owned())) },
        ];
        let named = name.clone().unwrap_or_else(|| (format!("{}_{}_ADJACENCY", qn.ident, element.alias), graph.at));
        let table = TableName { auth: Some(element.table.auth.clone()), ident: element.table.ident.clone(), at: graph.at };
        let storage = TableStorage { engine: Some((ADJACENCY.to_owned(), 0)), options };
        Ok(IndexStatement::Create { name: named, table, columns, expressions: Vec::new(), storage })
    }

    /// Commits the transaction, then gives each ENFORCED graph it created
    /// or replaced the adjacency indexes of its edge tables [GS §13.2]: an index is
    /// built outside a transaction (RULES.md D34).
    ///
    /// # Errors
    ///
    /// Those of COMMIT, and of CREATE ADJACENCY INDEX.
    pub(in crate::db) fn committing(&self, st: &Arc<State>) -> Result<()> {
        let published = self.db.snapshot()?;
        let created: Vec<QualifiedName> =
            st.catalog.graphs().into_iter().filter(|g| published.catalog.graph(&g.name) != Some(*g)).map(|g| g.name.clone()).collect();
        self.commit_in(st)?;
        if created.is_empty() {
            return Ok(());
        }
        let now = self.db.snapshot()?;
        for graph in &created {
            self.graph_adjacency(&now, graph)?;
        }
        Ok(())
    }

    /// \[GS §13.2\] Gives each base edge table of graph `qn`, if it is
    /// ENFORCED, an adjacency index of its ends, unless one keeps them
    /// already.
    ///
    /// # Errors
    ///
    /// Those of CREATE ADJACENCY INDEX.
    fn graph_adjacency(&self, st: &Arc<State>, qn: &QualifiedName) -> Result<()> {
        let Some(desc) = st.catalog.graph(qn).filter(|g| g.mode == GraphMode::Enforced).cloned() else { return Ok(()) };
        for element in desc.edges() {
            let Some(ends) = &element.endpoints else { continue };
            let Some(table) = st.catalog.table(&element.table) else { continue };
            let wanted: Vec<usize> =
                ends.source.columns.iter().chain(&ends.destination.columns).filter_map(|c| table.column_desc(c).map(|(k, _)| k)).collect();
            let kept = st.catalog.indexes(&element.table).into_iter().any(|ix| ix.method == ADJACENCY && ix.columns.starts_with(&wanted));
            if kept || !self.owns_schema(st, &element.table.auth) {
                continue;
            }
            let graph = TableName { auth: Some(qn.auth.clone()), ident: qn.ident.clone(), at: 0 };
            let index = AdjacencyIndex { name: None, graph, edge: (element.alias.clone(), 0), direction: None, include: Vec::new() };
            let stmt = IndexStatement::Adjacency(Box::new(index));
            self.index_statement(st, &stmt)?;
        }
        Ok(())
    }
}
