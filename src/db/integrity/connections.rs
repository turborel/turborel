//! \[EXTENSION\] The constraints of the node and edge tables of a schema
//! (§4.2) at the end of a statement: a CONNECTION constraint of an edge
//! table, satisfied by an edge whose endpoints are nodes of tables one of
//! its clauses joins (R12.2); and a node deleted, whose edges in an edge
//! table of its schema are deleted with it if a CONNECTION constraint
//! with ON DELETE CASCADE of that table names its node table, and else
//! forbid its deletion (R12.1).
//!
//! The edges of a node are found through each edge table's adjacency
//! index (`index/adjacency`), each row it gives read again, since it may
//! give versions the transaction does not see.

use std::ops::ControlFlow;
use std::sync::Arc;

use super::Effects;
use crate::catalog::{Connection, ElementId, FROM_ID, NODE_ID, NativeKind, QualifiedName, TO_ID, TableDesc};
use crate::db::{State, StatementContext, api};
use crate::error::{Code, Result, SqlError, fail};
use crate::graph::adjacency::{Direction, Step};
use crate::index::Key;
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::Value;

impl State {
    /// [R12.2] A row of edge table `desc` satisfies its CONNECTION
    /// constraint `name`, `k`: its endpoints are nodes, each of a row of
    /// its node table, of tables a clause of `k` joins.
    pub(super) fn check_connection(&self, tx: &Txn, desc: &TableDesc, (name, k): (&QualifiedName, &Connection), row: &Row) -> Result<()> {
        let end = |column: &str| desc.pseudo_column(column).and_then(|at| row.get(at));
        let (Some(Value::Char(from)), Some(Value::Char(to))) = (end(FROM_ID), end(TO_ID)) else { return Ok(()) };
        let (Some(f), Some(t)) = (ElementId::parse(from), ElementId::parse(to)) else { return Ok(()) };
        if !k.allows(&f.table, &t.table) {
            return fail(
                Code::Constraint,
                format!(
                    "[EXTENSION] GRF-0053 an edge of {} joins a node of {} to one of {}, which CONNECTION {name} does not allow",
                    desc.name, f.table, t.table
                ),
            );
        }
        for (id, text) in [(&f, from), (&t, to)] {
            if !self.node_exists(tx, &id.table, text)? {
                return fail(
                    Code::Constraint,
                    format!(
                        "[EXTENSION] GRF-0053 an edge of {} joins node {text}, which is no row of {}: CONNECTION {name}",
                        desc.name, id.table
                    ),
                );
            }
        }
        Ok(())
    }

    /// Whether node table `table` has the node of identity `text`.
    fn node_exists(&self, tx: &Txn, table: &QualifiedName, text: &str) -> Result<bool> {
        let Some(at) = self.catalog.table(table).and_then(|t| t.pseudo_column(NODE_ID)) else { return Ok(false) };
        let mut found = false;
        self.each_row_with(tx, table, (&[at], &[Value::Char(text.to_owned())]), &mut |_, _| {
            found = true;
            Ok(ControlFlow::Break(()))
        })?;
        Ok(found)
    }

    /// Whether a row deleted of `table` may have edges to act on: it is a
    /// node table of a schema with an edge table.
    pub(super) fn has_edges(&self, table: &QualifiedName) -> bool {
        let edges = |t: &&TableDesc| t.name.auth == table.auth && t.native() == Some(NativeKind::Edge);
        self.catalog.table(table).is_some_and(|t| t.native() == Some(NativeKind::Node)) && self.catalog.tables().iter().any(edges)
    }

    /// [R12.1] The edges of node `old`, a row deleted of node table
    /// `node`, in each edge table of its schema: deleted, each as a row a
    /// statement deletes, if a CONNECTION constraint of ON DELETE CASCADE
    /// of the edge table names `node`; else GRF-0052.
    pub(super) fn node_deleted(self: &Arc<Self>, tx: &Txn, (node, old): (&QualifiedName, &Row), effects: &mut Effects<'_>) -> Result<()> {
        let Some(Value::Char(id)) = self.catalog.table(node).and_then(|t| t.pseudo_column(NODE_ID)).and_then(|at| old.get(at)) else {
            return Ok(());
        };
        let edge_tables: Vec<TableDesc> = self
            .catalog
            .tables()
            .into_iter()
            .filter(|t| t.name.auth == node.auth && t.native() == Some(NativeKind::Edge))
            .cloned()
            .collect();
        for e in &edge_tables {
            let edges = self.edges_of(tx, e, id)?;
            if edges.is_empty() {
                continue;
            }
            if !e.connections().any(|(_, k)| k.cascade && k.names(node)) {
                return Err(SqlError::new(
                    Code::Constraint,
                    format!(
                        "[EXTENSION] GRF-0052 node {id} of {node} has edges in {}, whose CONNECTION constraints do not delete them with it (R12.1)",
                        e.name
                    ),
                ));
            }
            let cx = StatementContext { st: self, tx };
            for (edge, row) in edges {
                effects.cursors.acted_on(&e.name, edge);
                effects.before_deleted_row(cx, &e.name, (edge, &row))?;
                self.delete_row(tx, &e.name, edge)?;
                effects.deleted(&e.name, edge, row)?;
            }
        }
        Ok(())
    }

    /// The rows of edge table `e`, with their ids, of edges leaving or
    /// entering node `id`, each once, as the transaction sees them.
    fn edges_of(&self, tx: &Txn, e: &TableDesc, id: &str) -> Result<Vec<(RowId, Row)>> {
        let name = crate::db::graphs::native::adjacency_name(&e.name);
        let adjacency = Arc::clone(self.index_of(&name)?).adjacency().ok_or_else(|| api(format!("[DESIGN] {name} keeps no adjacency")))?;
        let key = Key(vec![Value::Char(id.to_owned())]);
        let mut steps: Vec<Step> = Vec::new();
        adjacency.steps(&key, Direction::Out, &mut steps)?;
        adjacency.steps(&key, Direction::In, &mut steps)?;
        let mut ids: Vec<RowId> = steps.into_iter().map(|s| s.edge).collect();
        ids.sort_unstable();
        ids.dedup();
        self.read_lock(tx, &e.name)?;
        let storage = self.storage_of(&e.name)?;
        let ends: Vec<usize> = [FROM_ID, TO_ID].iter().filter_map(|c| e.pseudo_column(c)).collect();
        let mut rows = Vec::new();
        for edge in ids {
            let Some(row) = storage.get(tx.id, edge)? else { continue };
            if ends.iter().any(|&at| matches!(row.get(at), Some(Value::Char(v)) if v.as_str() == id)) {
                rows.push((edge, row));
            }
        }
        Ok(rows)
    }
}
