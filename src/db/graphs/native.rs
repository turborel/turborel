//! \[EXTENSION\] Native node and edge tables (RULES.md D109),
//! Mode B of the property graph implementation standard (§4.2): a table
//! created AS NODE or AS EDGE, its hidden pseudo-columns, its indexes,
//! its CONNECTION constraints' rules, and the implicit graph SYS_GRAPH of
//! its schema (`native/graph.rs`). The values its rows are given are
//! `native/rows.rs`'s; the checks of CONNECTION and of a node's incident
//! edges (R12.1, R12.2) are `integrity/connections.rs`'s.
//!
//! A node table's rows each get a `$NODE_ID`, an edge table's an
//! `$EDGE_ID`, drawn from a generator of the table's own as an identity
//! column's is (RULES.md D63), and given as the identity's text (R5.1).
//! An edge's `$FROM_ID` and `$TO_ID` are the texts of two nodes of its
//! schema. Each pseudo-column is NOT NULL. A node table has an index of
//! its `$NODE_ID`, an edge table one of its `$EDGE_ID` and an `adjacency`
//! index of its endpoints (§13.2, RULES.md D107).

mod graph;
mod rows;

pub(in crate::db) use graph::{is_sys_graph, sys_graph_refused};
pub(in crate::db) use rows::Native;

use std::sync::Arc;

use crate::ast::{
    AlterAction, ColumnConstraint, ColumnConstraintKind, ColumnDefinition, ConstraintAttributes, SequenceOptions, TableDefinition,
    TableElement, TypeName,
};
use crate::catalog::{
    Catalog, EDGE_ID, FROM_ID, Generated, GraphColumn, IndexDesc, NODE_ID, NativeKind, QualifiedName, SYS_GRAPH, SequenceDesc, TO_ID,
    TableDesc, id_type,
};
use crate::db::contents::{Element, Kept};
use crate::db::sequences::{DEFAULT_TYPE, generator};
use crate::db::{State, api, rule};
use crate::error::{At, Result};
use crate::index::Index;
use crate::index::adjacency::ADJACENCY_METHOD;

/// What a pseudo-column index's name ends in, after the table's.
const ADJACENCY_SUFFIX: &str = "ADJACENCY";

/// The name of the adjacency index of edge table `table`.
pub(in crate::db) fn adjacency_name(table: &QualifiedName) -> String {
    format!("{}.{}.{ADJACENCY_SUFFIX}", table.auth, table.ident)
}

impl State {
    /// [EXTENSION] (§4.2) The pseudo-columns of table `t`, created AS NODE
    /// or AS EDGE, named `name`, after its columns: each NOT NULL, and of
    /// an identity its own generator.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `t` is temporary, typed, made from a query or
    /// system-versioned, none of which a node or edge table is, or its
    /// schema has a table or view of the name of its implicit graph.
    pub(in crate::db) fn native_elements(&self, name: &QualifiedName, t: &TableDefinition) -> Result<Vec<Element<'static>>> {
        let Some((kind, at)) = t.graph else { return Ok(Vec::new()) };
        let refused = [
            (t.temporary.is_some(), "temporary"),
            (t.typed.is_some(), "a typed table"),
            (t.subquery.is_some(), "made from a query"),
            (t.versioning.is_some(), "system-versioned"),
        ];
        if let Some((_, what)) = refused.iter().find(|(is, _)| *is) {
            return Err(rule(format!("[EXTENSION] a node or edge table (§4.2) is a persistent base table of its elements, not {what}")))
                .or_at(at);
        }
        let graph = QualifiedName { auth: name.auth.clone(), ident: SYS_GRAPH.to_owned() };
        if self.catalog.table(&graph).is_some() || self.catalog.view(&graph).is_some() {
            let msg = format!(
                "[EXTENSION] schema {} has a table or view {SYS_GRAPH}, the name of its node and edge tables' graph (§4.2)",
                name.auth
            );
            return Err(rule(msg)).or_at(at);
        }
        // [DESIGN] Numbered from 0, as the form §4.2 keeps numbers them.
        let made = generator(name.clone(), DEFAULT_TYPE, &SequenceOptions::default(), at)?;
        let identity = SequenceDesc { start: 0, minimum: 0, origin: 0, ..made };
        let columns = match kind {
            NativeKind::Node => vec![GraphColumn::Node(identity)],
            NativeKind::Edge => vec![GraphColumn::Edge(identity), GraphColumn::From, GraphColumn::To],
        };
        Ok(columns.into_iter().map(|g| pseudo_element(g, at)).collect())
    }

    /// [EXTENSION] (§4.2) That the CONNECTION constraints of `desc`, among
    /// the tables `earlier` of the same `<schema definition>`, are of an
    /// edge table, and each names node tables of its schema.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is not.
    pub(in crate::db) fn check_connections(&self, desc: &TableDesc, earlier: &[TableDesc]) -> Result<()> {
        for (name, k) in desc.connections() {
            if desc.native() != Some(NativeKind::Edge) {
                return Err(rule(format!("[EXTENSION] {name} is a CONNECTION constraint, so {} is an edge table (§4.2)", desc.name)));
            }
            for node in k.clauses.iter().flat_map(|(from, to)| [from, to]) {
                let found = self.catalog.table(node).cloned().or_else(|| earlier.iter().find(|t| t.name == *node).cloned());
                if node.auth != desc.name.auth || found.and_then(|t| t.native()) != Some(NativeKind::Node) {
                    let msg = format!(
                        "[EXTENSION] GRF-0004 CONNECTION constraint {name} names {node}, which is no node table of schema {} (§4.2)",
                        desc.name.auth
                    );
                    return Err(rule(msg));
                }
            }
        }
        Ok(())
    }

    /// [EXTENSION] (§4.2) That ALTER TABLE `action` of `desc` changes none
    /// of its pseudo-columns.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it drops or alters one.
    pub(in crate::db) fn pseudo_unaltered(desc: &TableDesc, action: &AlterAction) -> Result<()> {
        if matches!(action, AlterAction::AddVersioning(_)) && desc.native().is_some() {
            return Err(rule(format!("[EXTENSION] {} is a node or edge table (§4.2), which is not system-versioned", desc.name)));
        }
        let column = match action {
            AlterAction::DropColumn { column, .. } | AlterAction::AlterColumn { column, .. } => column,
            AlterAction::AddColumn(_)
            | AlterAction::AddConstraint(_)
            | AlterAction::DropConstraint { .. }
            | AlterAction::AddPeriod(..)
            | AlterAction::DropPeriod { .. }
            | AlterAction::AddVersioning(_)
            | AlterAction::DropVersioning { .. }
            | AlterAction::RevertCollation(_)
            | AlterAction::Truncate { .. }
            | AlterAction::Storage(_) => return Ok(()),
        };
        if desc.column_desc(column).is_some_and(|(_, c)| c.hidden()) {
            return Err(rule(format!("[EXTENSION] {column} is a pseudo-column of {} (§4.2), which is not dropped or altered", desc.name)));
        }
        Ok(())
    }

    /// [EXTENSION] (R12.1) That the rows of node table `desc` may go at
    /// once: its schema has no edge table, whose edges a row may join.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has one.
    pub(in crate::db) fn truncatable(&self, desc: &TableDesc) -> Result<()> {
        if desc.native() != Some(NativeKind::Node) {
            return Ok(());
        }
        let edges = self.catalog.tables().into_iter().find(|t| t.name.auth == desc.name.auth && t.native() == Some(NativeKind::Edge));
        if let Some(e) = edges {
            return Err(rule(format!(
                "[EXTENSION] GRF-0052 an edge of {} may join a node of {} (R12.1), so its rows are not truncated: DELETE checks each",
                e.name, desc.name
            )));
        }
        Ok(())
    }

    /// [EXTENSION] (§4.2, §13.2) The indexes of the pseudo-columns of node
    /// or edge table `d`, made of `made`'s structures where given, else
    /// anew, each recorded in `catalog`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a method is missing; the method's.
    pub(in crate::db) fn pseudo_indexes(
        &mut self,
        catalog: &mut dyn Catalog,
        d: &TableDesc,
        made: &dyn Fn(&str) -> Option<Arc<dyn Index>>,
    ) -> Result<Vec<(String, Arc<dyn Index>)>> {
        let named = |suffix: &str| format!("{}.{}.{suffix}", d.name.auth, d.name.ident);
        let at = |column: &str| d.pseudo_column(column);
        let btree = self.constraint_method().to_owned();
        let wanted: Vec<(String, Vec<usize>, String)> = match d.native() {
            None => return Ok(Vec::new()),
            Some(NativeKind::Node) => vec![(named(NODE_ID), at(NODE_ID).into_iter().collect(), btree)],
            Some(NativeKind::Edge) => vec![
                (named(EDGE_ID), at(EDGE_ID).into_iter().collect(), btree),
                (adjacency_name(&d.name), [at(FROM_ID), at(TO_ID)].into_iter().flatten().collect(), ADJACENCY_METHOD.to_owned()),
            ],
        };
        let mut out = Vec::new();
        for (name, columns, method) in wanted {
            let index = IndexDesc {
                name: name.clone(),
                table: d.name.clone(),
                columns,
                method: method.clone(),
                unique: false,
                options: Vec::new(),
                collations: Vec::new(),
                nulls: false,
                expressions: Vec::new(),
            };
            let types = index.key_types(d).ok_or_else(|| api(format!("[DESIGN] {} has no pseudo-column of index {name}", d.name)))?;
            let structure = if let Some(s) = made(&name) {
                s
            } else {
                let m = Arc::clone(self.methods.get(&method).ok_or_else(|| api(format!("[DESIGN] no index method {method}")))?);
                self.index_structure(&*m, (&index, d), &types)?.0
            };
            self.indexes.insert(name.clone(), Arc::clone(&structure));
            catalog.add_index(index);
            out.push((name, structure));
        }
        Ok(out)
    }
}

/// The `<column definition>` of pseudo-column `g`, written at `at`, NOT
/// NULL, and what it keeps: how its values are given.
fn pseudo_element(g: GraphColumn, at: usize) -> Element<'static> {
    let not_null = ColumnConstraint { name: None, at, kind: ColumnConstraintKind::NotNull, attributes: ConstraintAttributes::default() };
    let def = ColumnDefinition {
        name: g.name().to_owned(),
        at,
        data_type: Some(TypeName::Data(id_type())),
        default: None,
        generated: None,
        constraints: vec![not_null],
        collate: None,
    };
    Element::Copied(Box::new((TableElement::Column(def), Kept::new(None, Some(Generated::Graph(g))))))
}
