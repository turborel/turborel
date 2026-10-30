//! \[EXTENSION\] ALTER TABLE [ISO/IEC 9075:1992 §11.10]: ADD COLUMN
//! [§11.11] and DROP COLUMN [§11.15]; ALTER COLUMN [§11.12]
//! (`db/defaults.rs`); ADD and DROP of a table constraint [§11.16, §11.17]
//! (`db/alter/constraints.rs`); and the table's storage
//! (`db/alter/storage.rs`); in a transaction (`db/ddl.rs`). Split from
//! db/ddl.rs (RULES.md R-SIZE-2).
//!
//! A column added or dropped gives the table a new generation of its storage
//! (`TableDesc::generation`), and its rows are copied there, each with the
//! column added (its default [§11.11 GR2]) or removed [§11.15 GR4], by the
//! transaction's own writes: logged, under
//! its locks, and undone with it. Its indexes are made new on it, their
//! columns renumbered [§11.15 GR6]. Readers of earlier snapshots go on
//! reading the old generation, discarded once none does; a statement or
//! transaction that began before is refused at its next use of the table
//! (`TableLock::check_generation`).
//!
//! The views over the table, and the CHECKs that read it, keep their
//! meaning [§11.11 GR2, note]: each sees the columns the table had when it
//! was defined (`Horizon`), so a column added is none of theirs; a view
//! that refers to a dropped column, by name or by *, is one RESTRICT
//! refuses and CASCADE drops [§11.15 SR4, GR2], found by binding it again
//! without the column.

mod columns;
mod constraints;
mod storage;

pub(super) use storage::value_of;

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use super::ddl::Dropped;
use super::schema::constraint_index_name;
use super::{State, api, rule};
use crate::ast::{AlterAction, DropBehavior};
use crate::binder::view_degree;
use crate::catalog::{Actions, ConstraintKind, IndexDesc, PrivilegeDesc, QualifiedName, TableDesc, ViewDesc};
use crate::error::{At, Result};
use crate::index::Index;
use crate::storage::{Row, TableStorage};
use crate::txn::Txn;
use crate::value::Value;

/// What ALTER TABLE gives the table, made once and published as it is:
/// the storage of its new generation, if its columns change, and its
/// indexes made new; or what a `<schema definition>` gives each of its
/// tables, by name.
#[derive(Clone, Default)]
pub(super) struct Made {
    pub(super) storage: Option<Arc<dyn TableStorage>>,
    pub(super) indexes: Vec<(String, Arc<dyn Index>)>,
    pub(super) tables: Vec<(QualifiedName, Made)>,
}

impl Made {
    /// Discards them: they were never published.
    pub(super) fn discard(&self) {
        if let Some(storage) = &self.storage {
            storage.discard();
        }
        self.indexes.iter().for_each(|(_, ix)| ix.discard());
        self.tables.iter().for_each(|(_, made)| made.discard());
    }

    /// What was made for table `name`, of a `<schema definition>`.
    pub(super) fn of_table(&self, name: &QualifiedName) -> Option<&Made> {
        self.tables.iter().find(|(n, _)| n == name).map(|(_, made)| made)
    }
}

/// How each row of the table changes.
#[derive(Clone, Copy)]
pub(super) enum Reshape {
    /// None: the rows copied as they are, to storage made anew
    /// (`db/alter/storage.rs`).
    Keep,
    /// A column added last, null until its default is given it
    /// (`db/ddl.rs`).
    Append,
    /// The column at this position removed.
    Remove(usize),
}

impl Reshape {
    /// `row`, reshaped.
    pub(super) fn row(self, mut row: Row) -> Row {
        match self {
            Reshape::Keep => {}
            Reshape::Append => row.push(Value::Null),
            Reshape::Remove(at) => {
                if at < row.len() {
                    row.remove(at);
                }
            }
        }
        row
    }

    /// The new position of the column at `at`, none if it is removed.
    pub(super) fn moved(self, at: usize) -> Option<usize> {
        let Reshape::Remove(gone) = self else { return Some(at) };
        match at.cmp(&gone) {
            Ordering::Less => Some(at),
            Ordering::Equal => None,
            Ordering::Greater => Some(at.saturating_sub(1)),
        }
    }

    /// The columns `at`, moved; none if one is removed.
    pub(super) fn columns(self, at: &[usize]) -> Option<Vec<usize>> {
        at.iter().map(|&c| self.moved(c)).collect()
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.10] Alters base table `qn` as `action` says,
    /// the names `action` writes without a schema of schema `names` [§5.4
    /// SR8], a datetime it writes without a displacement of `zone`'s [§5.3
    /// SR18]: its descriptor, indexes, views and privileges, and a new generation
    /// of its storage, `made`'s if given, else new. What it took away, what
    /// it made, and how its rows change.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule the action breaks; the engine's or method's.
    pub(super) fn alter_table(
        &mut self,
        (qn, names, zone): (&QualifiedName, &str, i16),
        action: &AlterAction,
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        match action {
            AlterAction::AddColumn(_) | AlterAction::DropColumn { .. } => {
                let (dropped, built, reshape) = self.alter_columns((qn, names, zone), action, made)?;
                Ok((dropped, built, Some(reshape)))
            }
            AlterAction::AlterColumn { column, at, default } => {
                self.alter_column((qn, zone), column, default.as_ref()).or_at(*at)?;
                Ok((Dropped::default(), Made::default(), None))
            }
            AlterAction::AddConstraint(c) => Ok((Dropped::default(), self.add_constraint((qn, names), c, (made, tx)).or_at(c.at)?, None)),
            AlterAction::DropConstraint { name, behavior } => {
                Ok((self.drop_constraint((qn, names), name, *behavior).or_at(name.at)?, Made::default(), None))
            }
            AlterAction::Storage(_) => self.alter_storage(qn, action, (made, tx)),
        }
    }

    /// The column actions of `alter_table`, whose rows change as the
    /// reshape returned says.
    fn alter_columns(
        &mut self,
        (qn, names, zone): (&QualifiedName, &str, i16),
        action: &AlterAction,
        made: Option<&Made>,
    ) -> Result<(Dropped, Made, Reshape)> {
        let desc = self.base_table(qn)?;
        let (next, reshape, others) = match action {
            AlterAction::AddColumn(def) => (self.add_column((&desc, names, zone), def).or_at(def.at)?, Reshape::Append, Vec::new()),
            AlterAction::DropColumn { column, at, behavior } => self.drop_column(&desc, column, *behavior).or_at(*at)?,
            AlterAction::AlterColumn { .. }
            | AlterAction::AddConstraint(_)
            | AlterAction::DropConstraint { .. }
            | AlterAction::Storage(_) => {
                return Err(api("[DESIGN] not a column action"));
            }
        };
        // [§11.15 SR4] the assertions that refer to a column dropped.
        let asserting = if let (AlterAction::DropColumn { behavior, .. }, Reshape::Remove(at)) = (action, reshape) {
            self.drop_column_asserting(&desc, at, *behavior == DropBehavior::Cascade)?
        } else {
            Vec::new()
        };
        let (mut dropped, built) = self.regenerate(qn, (&desc, next), (reshape, others), (made, action))?;
        dropped.names.extend(asserting);
        Ok((dropped, built, reshape))
    }

    /// Table `qn`, `desc` until now, made `next`, its rows to change as
    /// `reshape` says, with `others`, other tables' descriptors its change
    /// changed: a new generation of its storage (`made`'s, if given), its
    /// indexes made new, its views, checks and privileges as `action`
    /// leaves them. What it took away, and what it made.
    fn regenerate(
        &mut self,
        qn: &QualifiedName,
        (desc, next): (&TableDesc, TableDesc),
        (reshape, others): (Reshape, Vec<TableDesc>),
        (made, action): (Option<&Made>, &AlterAction),
    ) -> Result<(Dropped, Made)> {
        let narrowed = self.narrowed_views(qn, reshape);
        let mut dropped = Dropped::default();
        let storage = if let Some(storage) = made.and_then(|m| m.storage.clone()) {
            storage
        } else {
            let engine = Arc::clone(self.engines.get(&next.engine).ok_or_else(|| api(format!("[DESIGN] no engine {}", next.engine)))?);
            self.table_storage(&*engine, &next)?
        };
        dropped.storage.extend(self.storage.insert(qn.clone(), Arc::clone(&storage)));
        let indexes = self.remake_indexes(&next, reshape, made, &mut dropped)?;
        let mut catalog = self.catalog.duplicate();
        catalog.replace_table(next.clone());
        // Other tables' foreign keys to it, renumbered or dropped.
        for other in others {
            catalog.replace_table(other);
        }
        catalog.add_views(narrowed);
        for ix in &indexes {
            catalog.remove_index(&ix.0.name);
            catalog.add_index(ix.0.clone());
        }
        self.catalog = Arc::from(catalog);
        self.rebind_checks(qn)?;
        self.alter_views(qn, reshape, action)?;
        self.alter_privileges(qn, desc, reshape);
        let built = Made {
            storage: Some(storage),
            indexes: indexes.into_iter().map(|(ix, structure)| (ix.name, structure)).collect(),
            tables: Vec::new(),
        };
        Ok((dropped, built))
    }

    /// [§11.9] The CHECKs that read `qn`, its own among them, bound to it
    /// as it is now, each seeing the columns it did.
    fn rebind_checks(&mut self, qn: &QualifiedName) -> Result<()> {
        let reads_it = |t: &TableDesc| {
            t.name == *qn
                || t.constraints.iter().any(|c| if let ConstraintKind::Check { reads, .. } = &c.kind { reads.contains(qn) } else { false })
        };
        let mut catalog = self.catalog.duplicate();
        for t in self.catalog.tables().into_iter().filter(|t| reads_it(t)) {
            let mut desc = t.clone();
            self.bind_checks(&mut desc)?;
            catalog.replace_table(desc);
        }
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// The base table `qn` [§11.10 SR2, SR4].
    pub(super) fn base_table(&self, qn: &QualifiedName) -> Result<TableDesc> {
        if let Some(desc) = self.catalog.table(qn) {
            return Ok(desc.clone());
        }
        let (what, clause) = if self.catalog.view(qn).is_some() { ("a view, not a base table", "SR4") } else { ("not defined", "SR2") };
        Err(rule(format!("[ISO/IEC 9075:1992 §11.10 {clause}] {qn} is {what}")))
    }

    /// The indexes of the table, `next` now, made again on its new
    /// generation (`made`'s, if given), their columns moved; one on a
    /// column removed, or enforcing a constraint no longer there, is not;
    /// a UNIQUE constraint added has one made. The old ones are in
    /// `dropped`. Each with its descriptor.
    fn remake_indexes(
        &mut self,
        next: &TableDesc,
        reshape: Reshape,
        made: Option<&Made>,
        dropped: &mut Dropped,
    ) -> Result<Vec<(IndexDesc, Arc<dyn Index>)>> {
        let mut kept: Vec<IndexDesc> = Vec::new();
        let mut gone = Vec::new();
        for ix in self.catalog.indexes(&next.name) {
            gone.push(ix.name.clone());
            let Some(columns) = reshape.columns(&ix.columns) else { continue };
            if ix.unique && !next.unique().any(|u| u == columns.as_slice()) {
                continue;
            }
            kept.push(IndexDesc { columns, ..ix.clone() });
        }
        let added: Vec<Vec<usize>> =
            next.unique().filter(|u| !kept.iter().any(|ix| ix.unique && ix.columns == *u)).map(<[usize]>::to_vec).collect();
        // Of as many names as the table has indexes, and one more, one is free.
        let names = self.catalog.indexes(&next.name).len().saturating_add(added.len());
        for columns in added {
            let free = (0..=names)
                .map(|k| constraint_index_name(&next.name, k))
                .find(|n| self.catalog.index(n).is_none() && !kept.iter().any(|ix| &ix.name == n));
            let name = free.ok_or_else(|| api(format!("[DESIGN] no name is free for a UNIQUE constraint of {}", next.name)))?;
            kept.push(IndexDesc {
                name,
                table: next.name.clone(),
                columns,
                method: self.constraint_method().to_owned(),
                unique: true,
                options: Vec::new(),
            });
        }
        let mut out = Vec::new();
        for ix in kept {
            let structure = if let Some((_, structure)) = made.and_then(|m| m.indexes.iter().find(|(n, _)| *n == ix.name)) {
                Arc::clone(structure)
            } else {
                let method =
                    Arc::clone(self.methods.get(&ix.method).ok_or_else(|| api(format!("[DESIGN] no index method {}", ix.method)))?);
                let types = next.types(&ix.columns).unwrap_or_default();
                self.index_structure(&*method, (&ix, next), &types)?.0
            };
            out.push((ix, structure));
        }
        let mut catalog = self.catalog.duplicate();
        for name in gone {
            catalog.remove_index(&name);
            dropped.indexes.extend(self.indexes.remove(&name));
        }
        self.catalog = Arc::from(catalog);
        for (ix, structure) in &out {
            self.indexes.insert(ix.name.clone(), Arc::clone(structure));
        }
        Ok(out)
    }

    /// The views that read `table`, once its column at a position is
    /// removed as `reshape` says, each seeing one column fewer of it if it
    /// saw that one [§11.15 GR4]: before they are bound again, so one that
    /// read it by * no longer binds as it did [§11.15 SR4].
    fn narrowed_views(&self, table: &QualifiedName, reshape: Reshape) -> Vec<ViewDesc> {
        let Reshape::Remove(at) = reshape else { return Vec::new() };
        self.catalog
            .views()
            .into_iter()
            .filter(|v| v.horizon.width(table).is_some())
            .map(|v| ViewDesc { horizon: v.horizon.without(table, at), ..v.clone() })
            .collect()
    }

    /// The views over `table` once it is altered: each must bind as before
    /// [§11.11 GR2, note]; after a DROP COLUMN, one that does not refers
    /// to the column, and is dropped with the views over it if CASCADE,
    /// else refused [§11.15 SR4, GR2]. An updatable view's columns are
    /// moved with the table's.
    fn alter_views(&mut self, table: &QualifiedName, reshape: Reshape, action: &AlterAction) -> Result<()> {
        let over = self.views_reading(table);
        let views: Vec<ViewDesc> = over.iter().filter_map(|v| self.catalog.view(v).cloned()).collect();
        let mut broken = Vec::new();
        for view in &views {
            if !view_degree(self, view).is_ok_and(|n| n == view.columns.len()) {
                broken.push(view.name.clone());
            }
        }
        let Some(first) = broken.first() else { return self.move_view_columns(&views, table, reshape) };
        let AlterAction::DropColumn { behavior: DropBehavior::Cascade, .. } = action else {
            return Err(match action {
                AlterAction::AddColumn(def) => api(format!("[DESIGN] view {first} binds otherwise once {} is added to {table}", def.name)),
                AlterAction::DropColumn { column, .. } => rule(format!(
                    "[ISO/IEC 9075:1992 §11.15 SR4] view {first} refers to {table}.{column}, so it is dropped only with CASCADE"
                )),
                AlterAction::AlterColumn { .. }
                | AlterAction::AddConstraint(_)
                | AlterAction::DropConstraint { .. }
                | AlterAction::Storage(_) => api("[DESIGN] a default, a constraint or the storage changes no view"),
            });
        };
        let mut gone: Vec<QualifiedName> = broken.clone();
        for view in &broken {
            gone.extend(self.views_reading(view));
        }
        let mut catalog = self.catalog.duplicate();
        for view in &gone {
            catalog.remove_view(view);
            catalog.revoke_all(view);
        }
        self.catalog = Arc::from(catalog);
        let kept: Vec<ViewDesc> = views.into_iter().filter(|v| !gone.contains(&v.name)).collect();
        self.move_view_columns(&kept, table, reshape)
    }

    /// The columns of `table` the updatable views of `views` are, moved.
    fn move_view_columns(&mut self, views: &[ViewDesc], table: &QualifiedName, reshape: Reshape) -> Result<()> {
        let mut moved = Vec::new();
        for view in views {
            let Some((base, map)) = &view.updates else { continue };
            if base != table {
                continue;
            }
            let map = reshape.columns(map).ok_or_else(|| api(format!("[DESIGN] view {} is a column dropped from {table}", view.name)))?;
            moved.push(ViewDesc { updates: Some((base.clone(), map)), ..view.clone() });
        }
        let mut catalog = self.catalog.duplicate();
        catalog.add_views(moved);
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// The privileges on `table`, `before` it was altered, with their
    /// columns moved: INSERT, UPDATE or REFERENCES of a column dropped
    /// revoked [§11.15 GR1]; of every column, extended to one added [§11.11
    /// GR3; §10.3 GR3–GR5].
    fn alter_privileges(&mut self, table: &QualifiedName, before: &TableDesc, reshape: Reshape) {
        let every: BTreeSet<usize> = (0..before.columns.len()).collect();
        let descs: Vec<PrivilegeDesc> = self.catalog.privileges(table).into_iter().cloned().collect();
        let appended = matches!(reshape, Reshape::Append);
        let moved = |columns: &mut BTreeSet<usize>, extended: bool| {
            *columns = columns.iter().filter_map(|&c| reshape.moved(c)).collect();
            if extended {
                columns.insert(before.columns.len());
            }
        };
        let privileges: Vec<_> = descs
            .iter()
            .map(|p| {
                let whole = |of: Columns| appended && on_every_column(&descs, p, (of, &every));
                let (insert, update, references) = (whole(|a| &a.insert), whole(|a| &a.update), whole(|a| &a.references));
                let mut p = p.clone();
                moved(&mut p.actions.insert, insert);
                moved(&mut p.actions.update, update);
                moved(&mut p.actions.references, references);
                p
            })
            .collect();
        let mut catalog = self.catalog.duplicate();
        catalog.revoke_all(table);
        catalog.grant(privileges);
        self.catalog = Arc::from(catalog);
    }
}

/// The columns of one privilege of a set of them: of INSERT, UPDATE or
/// REFERENCES.
type Columns = fn(&Actions) -> &BTreeSet<usize>;

/// Whether the privilege `of` picks, held on `every` column by `p`'s
/// grantee of `p`'s grantor, in any of `descs`, is `p`'s: of the grantable
/// descriptor if it is grantable on every column, else of the other. So a
/// privilege on the table goes to a column added, once [ISO/IEC 9075:1992
/// §11.11 GR3].
fn on_every_column(descs: &[PrivilegeDesc], p: &PrivilegeDesc, (of, every): (Columns, &BTreeSet<usize>)) -> bool {
    let alike = || descs.iter().filter(|q| q.grantor == p.grantor && q.grantee == p.grantee);
    let held: BTreeSet<usize> = alike().flat_map(|q| of(&q.actions).iter().copied()).collect();
    let given: BTreeSet<usize> = alike().filter(|q| q.grantable).flat_map(|q| of(&q.actions).iter().copied()).collect();
    held == *every && (given == *every) == p.grantable
}
