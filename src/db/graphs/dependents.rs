//! \[EXTENSION\] What a property graph depends on (§4.1 R4.7): the tables
//! and views it reads, the privileges its owner holds on them, and the
//! routines its properties invoke. A change of a table it reads is met as
//! a view's is: a column it names dropped is refused under RESTRICT and
//! drops it under CASCADE, and a column it names is not renamed. Any other
//! change that breaks it, a key's constraint or a column's type changed,
//! marks it INVALID: it stays so, its definition kept, until ALTER
//! PROPERTY GRAPH ... COMPILE finds it holds again.

use super::super::State;
use super::super::ddl::Dropped;
use super::super::revoke::Abandoned;
use crate::ast::{AlterAction, AlterColumnAction, DropBehavior};
use crate::catalog::{Catalog, GraphDesc, QualifiedName, applicable, owner_of, sole};
use crate::error::{Result, SqlError};

impl State {
    /// The graphs that read base table `table`, or a view that reads it,
    /// described again as ALTER TABLE `action` left it: those that hold
    /// with their types as they are now, the others as `graph_broken`
    /// says. What it took away.
    ///
    /// # Errors
    ///
    /// As `graph_broken`'s.
    pub(in crate::db) fn graphs_altered(&mut self, table: &QualifiedName, action: &AlterAction) -> Result<Dropped> {
        let mut touched = self.views_reading(table);
        touched.push(table.clone());
        let reading: Vec<GraphDesc> =
            self.catalog.graphs().into_iter().filter(|g| g.reads.iter().any(|r| touched.contains(r))).cloned().collect();
        // [EXTENSION] (§4.2) SYS_GRAPH is described anew instead.
        let reading = reading.into_iter().filter(|g| !super::native::is_sys_graph(&g.name));
        let mut dropped = Dropped::default();
        for g in reading {
            match self.redescribe_graph(&g, true) {
                Ok(again) => {
                    if again != g {
                        self.put_graph(again);
                    }
                }
                Err(e) => dropped.absorb(self.graph_broken(g, (table, action), e)?),
            }
        }
        Ok(dropped)
    }

    /// Graph `g`, which `action` on `table` broke as `e` says: with a
    /// column dropped CASCADE, dropped with what reads it; else INVALID.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column dropped RESTRICT, or renamed, is one it
    /// names.
    fn graph_broken(&mut self, g: GraphDesc, (table, action): (&QualifiedName, &AlterAction), e: SqlError) -> Result<Dropped> {
        let refused = match action {
            AlterAction::DropColumn { behavior: DropBehavior::Cascade, .. } => return self.drop_graph(&g.name, true),
            AlterAction::DropColumn { column, behavior: DropBehavior::Restrict, .. } => Some(SqlError {
                msg: format!(
                    "[ISO/IEC 9075:1992 §11.15 SR3] property graph {} reads {table}.{column} and binds no more without it ({}), so it is dropped only with CASCADE",
                    g.name, e.msg
                ),
                ..e
            }),
            // [RULES.md D83] as a text that names it, refused.
            AlterAction::AlterColumn { column, action: AlterColumnAction::Rename(_), .. } => {
                Some(super::graph_rule(None, format!("property graph {} names {table}.{column}, so it is not renamed", g.name)))
            }
            AlterAction::AddColumn(_)
            | AlterAction::AlterColumn { .. }
            | AlterAction::AddConstraint(_)
            | AlterAction::DropConstraint { .. }
            | AlterAction::AddPeriod(..)
            | AlterAction::DropPeriod { .. }
            | AlterAction::AddVersioning(_)
            | AlterAction::DropVersioning { .. }
            | AlterAction::RevertCollation(_)
            | AlterAction::Truncate { .. }
            | AlterAction::AttachPartition { .. }
            | AlterAction::DetachPartition(_)
            | AlterAction::Storage(_) => None,
        };
        if let Some(refusal) = refused {
            return Err(refusal);
        }
        self.put_graph(GraphDesc { valid: false, ..g });
        Ok(Dropped::default())
    }

    /// Adds to `abandoned` the graphs whose owners hold SELECT in `after`
    /// no more on a table or view they read, as a view's owner must
    /// [ISO/IEC 9075:1992 §11.37 SR13], and those of the others some of
    /// whose grants their grantors hold with grant option no more
    /// [§11.37 SR9].
    pub(in crate::db) fn graphs_abandoned(&self, after: &dyn Catalog, abandoned: &mut Abandoned) {
        for g in self.catalog.graphs() {
            let owner = owner_of(&*self.catalog, &g.name.auth);
            if g.reads.iter().any(|t| !applicable(after, sole(after, owner), t).0.select) {
                abandoned.graphs.push(g.name.clone());
                continue;
            }
            if g.select.iter().any(|u| !g.select_of(after, sole(after, &u.grantor)).1) {
                abandoned.graph_grants.push(g.name.clone());
            }
        }
    }

    /// The graphs that hold no more in `after`, as `lost` takes the error
    /// of each described again there.
    pub(in crate::db) fn graphs_unbound_in(&self, after: &State, lost: &dyn Fn(&SqlError) -> bool) -> Vec<QualifiedName> {
        let gone = |g: &&GraphDesc| after.catalog.graph(&g.name).is_some() && after.redescribe_graph(g, true).is_err_and(|e| lost(&e));
        self.catalog.graphs().into_iter().filter(gone).map(|g| g.name.clone()).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.62 GR1, by analogy] The graphs whose
    /// properties invoke one of the routines `gone`.
    pub(in crate::db) fn graphs_invoking(&self, gone: &[QualifiedName]) -> Vec<QualifiedName> {
        let on = |g: &&GraphDesc| g.horizon.subjects().is_some_and(|s| s.iter().any(|r| gone.contains(r)));
        self.catalog.graphs().into_iter().filter(on).map(|g| g.name.clone()).collect()
    }
}
