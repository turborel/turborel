//! \[EXTENSION\] ALTER TABLE ... ATTACH PARTITION and DETACH PARTITION
//! (RULES.md D153). A table attached keeps its rows, which must each be
//! one its bound holds (checked as the statement ends, with every
//! constraint of the tree, `validate_attached`), and its indexes: each
//! template of the tree adopts an index of a partition that has its
//! definition, under the template's derived name, or is built on it. A
//! table detached keeps its rows and its indexes, those of the tree's
//! templates renamed as ordinary indexes of it.

use std::sync::Arc;

use super::super::alter::{Made, Reshape};
use super::super::ddl::Dropped;
use super::super::integrity::Effects;
use super::super::{State, api};
use super::indexes::derived_of;
use crate::ast::{AlterAction, BoundSpec, TableName};
use crate::binder::qualify;
use crate::catalog::{Catalog, ColumnDesc, IndexDesc, MAX_PARTITION_DEPTH, PartitionDesc, QualifiedName, TableDesc, template_of};
use crate::error::{At, Result};
use crate::partition::refused;
use crate::txn::{Timestamp, Txn};
use crate::value::Value;

/// What an ALTER TABLE action did: what it dropped, what it made, how the
/// rows change.
type Altered = (Dropped, Made, Option<Reshape>);

impl State {
    /// ATTACH PARTITION `table` FOR VALUES `spec` of the partitioned table
    /// `qn`, the names of schema `names`, a datetime of displacement
    /// `zone`; the structures of the indexes `made`'s, if given.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `qn` is not partitioned, the table is not a base
    /// table of the same columns, is a partition already or above `qn`,
    /// or the bound is not one `qn`'s strategy takes beside its others.
    pub(in crate::db) fn attach_partition(
        &mut self,
        (qn, names, zone): (&QualifiedName, &str, i16),
        (table, spec): (&TableName, &BoundSpec),
        (made, tx): (Option<&Made>, Option<&Txn>),
    ) -> Result<Altered> {
        let child = qualify(table, names);
        let desc = self.base_table(&child).or_at(table.at)?;
        let parent = self.base_table(qn)?;
        self.check_attachable((&parent, &desc), &child).or_at(table.at)?;
        let mut catalog = self.catalog.duplicate();
        let of = self.place_partition(&mut *catalog, (&child, qn), (spec, zone)).or_at(table.at)?;
        let key = catalog.partitioning().key(&child).cloned();
        catalog.partitioning_mut().put(PartitionDesc { table: child.clone(), key, of: Some(of) });
        let mut built = Made::default();
        for leaf in catalog.partitioning().leaves(&child) {
            self.give_templates(&mut *catalog, &leaf, (&mut built, made))?;
        }
        // The tree's global indexes, of its rows before; the rows attached
        // are added as they are checked (RULES.md D157).
        let mut dropped = Dropped::default();
        dropped.indexes.extend(self.sync_globals(&mut *catalog, qn, (&mut built, made, tx))?);
        self.catalog = Arc::from(catalog);
        // Opening the database, no statement adds them, so each such index
        // is built again once its tables are reopened.
        for leaf in self.catalog.partitioning().leaves(&child) {
            for (ix, _) in self.globals_over(&leaf) {
                self.leave_unbuilt(&ix.name);
            }
        }
        Ok((dropped, built, None))
    }

    /// That `desc`, `child`, may be attached to `parent`: a base table of
    /// its columns and temporary kind, in no tree yet, not above it, and
    /// shallow enough.
    fn check_attachable(&self, (parent, desc): (&TableDesc, &TableDesc), child: &QualifiedName) -> Result<()> {
        let p = self.catalog.partitioning();
        if !p.is_partitioned(&parent.name) {
            return refused(format!("[EXTENSION] {} is no partitioned table, so it takes no partition", parent.name));
        }
        if p.of(child).is_some() {
            return refused(format!("[EXTENSION] {child} is a partition already"));
        }
        if *child == parent.name || p.ancestors(&parent.name).contains(child) {
            return refused(format!("[EXTENSION] {child} is {} or above it in its tree", parent.name));
        }
        if desc.typed.is_some() || desc.columns.iter().any(|c| c.graph_column().is_some()) {
            return refused(format!("[EXTENSION] {child} is a typed table or a node or edge table, so it is no partition (RULES.md D153)"));
        }
        let height = p.descendants(child).iter().map(|d| p.ancestors(d).len()).max().unwrap_or(0);
        if p.ancestors(&parent.name).len().saturating_add(height).saturating_add(2) > MAX_PARTITION_DEPTH {
            return refused(format!("[EXTENSION] attached, {child} would be more than {MAX_PARTITION_DEPTH} levels deep"));
        }
        if parent.temporary != desc.temporary || child.auth != parent.name.auth {
            return refused(format!("[EXTENSION] {child} and {} are not of one schema and one temporary kind", parent.name));
        }
        let periods = |d: &TableDesc| d.periods.iter().map(|pd| (pd.name.clone(), pd.start.clone(), pd.end.clone())).collect::<Vec<_>>();
        if self.versioned(child) != self.versioned(&parent.name) || periods(parent) != periods(desc) {
            return refused(format!("[EXTENSION] {child} and {} have not the same periods, both system-versioned or neither", parent.name));
        }
        let same = |a: &ColumnDesc, b: &ColumnDesc| {
            a.name == b.name && a.data_type == b.data_type && a.collation == b.collation && a.not_null == b.not_null
        };
        let columns = parent.columns.len() == desc.columns.len() && parent.columns.iter().zip(&desc.columns).all(|(a, b)| same(a, b));
        if !columns {
            let message = format!(
                "[EXTENSION] {child} has not the columns of {}: the same names, types, collations and nullability, in order",
                parent.name
            );
            return refused(message);
        }
        Ok(())
    }

    /// Gives `leaf`, a partition holding rows now in `catalog`'s tree, the
    /// index of each template of the tree it lacks: one of its own of the
    /// template's definition, adopted under the derived name, or one built
    /// of its rows, `made`'s if given.
    pub(in crate::db) fn give_templates(
        &mut self,
        catalog: &mut dyn Catalog,
        leaf: &QualifiedName,
        (built, made): (&mut Made, Option<&Made>),
    ) -> Result<()> {
        let desc = catalog.table(leaf).cloned().ok_or_else(|| api(format!("[DESIGN] no table {leaf}")))?;
        for ix in derived_of(catalog, leaf) {
            if catalog.index(&ix.name).is_some() {
                continue;
            }
            let ix = self.bound_derived(ix, &desc)?;
            if let Some(own) = adoptable(catalog, &ix) {
                self.rename_index(catalog, &own, &ix.name);
                if let Some(old) = catalog.index(&ix.name).cloned() {
                    catalog.remove_index(&ix.name);
                    catalog.add_index(IndexDesc { unique: ix.unique, nulls: ix.nulls, ..old });
                }
                continue;
            }
            let kept = made.and_then(|m| m.indexes.iter().find(|(n, _)| *n == ix.name)).map(|(_, s)| Arc::clone(s));
            let structure = if let Some(structure) = kept {
                structure
            } else {
                let (structure, reopened) = self.derived_structure(&ix, &desc)?;
                if !reopened {
                    self.load_index(&*structure, &ix, Timestamp::MAX)?;
                }
                structure
            };
            self.indexes.insert(ix.name.clone(), Arc::clone(&structure));
            built.indexes.push((ix.name.clone(), structure));
            catalog.add_index(ix);
        }
        Ok(())
    }

    /// Renames the index `from` of `catalog` `to`, its structure kept.
    fn rename_index(&mut self, catalog: &mut dyn Catalog, from: &str, to: &str) {
        let Some(old) = catalog.index(from).cloned() else { return };
        catalog.remove_index(from);
        catalog.add_index(IndexDesc { name: to.to_owned(), ..old });
        if let Some(structure) = self.indexes.remove(from) {
            self.indexes.insert(to.to_owned(), structure);
        }
    }

    /// DETACH PARTITION `table` of the partitioned table `qn`, the names of
    /// schema `names`: it keeps its rows, and its indexes of the tree's
    /// templates, as ordinary ones (`plain_name`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no partition of `qn`, or a foreign key
    /// references `qn` or a table above it, whose rows it would take away.
    pub(in crate::db) fn detach_partition(
        &mut self,
        (qn, names): (&QualifiedName, &str),
        table: &TableName,
        tx: Option<&Txn>,
    ) -> Result<Altered> {
        let child = qualify(table, names);
        let p = self.catalog.partitioning();
        if p.of(&child).is_none_or(|o| o.parent != *qn) {
            return refused(format!("[EXTENSION] {child} is no partition of {qn}")).or_at(table.at);
        }
        let tree: Vec<QualifiedName> = std::iter::once(qn.clone()).chain(p.ancestors(qn)).collect();
        let referenced = self.catalog.tables().into_iter().flat_map(TableDesc::foreign_keys).any(|(_, fk)| tree.contains(&fk.table));
        if referenced {
            return refused(format!("[EXTENSION] a foreign key references {qn} or a table above it, so no partition of it is detached"))
                .or_at(table.at);
        }
        let mut catalog = self.catalog.duplicate();
        let leaves = catalog.partitioning().leaves(&child);
        let before: Vec<Vec<String>> = leaves.iter().map(|l| derived_of(&*catalog, l).into_iter().map(|ix| ix.name).collect()).collect();
        let key = catalog.partitioning().key(&child).cloned();
        catalog.partitioning_mut().put(PartitionDesc { table: child.clone(), key, of: None });
        for (leaf, derived) in leaves.iter().zip(before) {
            let kept: Vec<String> = derived_of(&*catalog, leaf).into_iter().map(|ix| ix.name).collect();
            for name in derived.iter().filter(|n| !kept.contains(n)) {
                let plain = plain_name(&*catalog, name, leaf);
                self.rename_index(&mut *catalog, name, &plain);
                if let Some(ix) = catalog.index(&plain).cloned() {
                    catalog.remove_index(&plain);
                    catalog.add_index(IndexDesc { unique: false, ..ix });
                }
            }
        }
        // The global indexes of both trees, as each needs them (RULES.md D157).
        let (mut dropped, mut built) = (Dropped::default(), Made::default());
        dropped.indexes.extend(self.sync_globals(&mut *catalog, qn, (&mut built, None, tx))?);
        dropped.indexes.extend(self.sync_globals(&mut *catalog, &child, (&mut built, None, tx))?);
        self.catalog = Arc::from(catalog);
        Ok((dropped, built, None))
    }
}

/// An index of `ix`'s table, its own (of no template, enforcing no
/// constraint), of `ix`'s definition: its method, key and options.
fn adoptable(catalog: &dyn Catalog, ix: &IndexDesc) -> Option<String> {
    let shown = |options: &[(String, Value)]| options.iter().map(|(n, v)| (n.clone(), v.to_literal())).collect::<Vec<_>>();
    let same = |own: &&IndexDesc| {
        own.method == ix.method && own.columns == ix.columns && own.texts() == ix.texts() && shown(&own.options) == shown(&ix.options)
    };
    catalog
        .indexes(&ix.table)
        .into_iter()
        .filter(|own| template_of(&own.name).is_none() && !own.unique)
        .find(same)
        .map(|own| own.name.clone())
}

/// The ordinary name the index `name`, of a template, of the partition
/// `leaf` detached, takes: the template's name and the table's, joined by
/// an underscore, else the first of them and a number free.
fn plain_name(catalog: &dyn Catalog, name: &str, leaf: &QualifiedName) -> String {
    let base = format!("{}_{}", template_of(name).unwrap_or(name), leaf.ident);
    if catalog.index(&base).is_none() {
        return base;
    }
    // A catalog holds fewer indexes than this.
    (2_u64..=u64::from(u32::MAX)).map(|n| format!("{base}_{n}")).find(|n| catalog.index(n).is_none()).unwrap_or(base)
}

/// The rows of the table `attached`, attached to a tree in `next`, as
/// transaction `tx` sees them: each held by its partition's bounds, and
/// satisfying the tree's constraints, checked as the statement ends.
///
/// # Errors
///
/// SQLCODE -201, SQLSTATE 23T01, for a row no bound holds; for a
/// constraint a row breaks.
pub(in crate::db) fn validate_attached(next: &Arc<State>, tx: &Txn, attached: &QualifiedName, mut effects: Effects<'_>) -> Result<()> {
    for leaf in next.catalog.partitioning().leaves(attached) {
        next.read_lock(tx, &leaf)?;
        let width = next.catalog.table(&leaf).map_or(0, |d| d.columns.len());
        let columns: Vec<usize> = (0..width).collect();
        effects.rewritten(&leaf);
        for read in Arc::clone(next.storage_of(&leaf)?).scan(tx.id, &columns) {
            let (id, row) = read?;
            next.check_holds(&leaf, &row)?;
            effects.inserted(&leaf, id, &row)?;
        }
    }
    next.end_statement(tx, effects)
}

impl State {
    /// That `action` may alter `qn`: of a table of a partitioning tree,
    /// neither its columns nor its periods change, which are the tree's,
    /// and a partitioned table, which holds no rows, keeps no storage
    /// options and is truncated by its partitions.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it may not.
    pub(in crate::db) fn check_alterable(&self, qn: &QualifiedName, action: &AlterAction) -> Result<()> {
        let p = self.catalog.partitioning();
        let partitioned = p.is_partitioned(qn);
        if !partitioned && p.of(qn).is_none() {
            return Ok(());
        }
        let refused_for = match action {
            AlterAction::AddColumn(_)
            | AlterAction::DropColumn { .. }
            | AlterAction::AlterColumn { .. }
            | AlterAction::AddPeriod(..)
            | AlterAction::DropPeriod { .. }
            | AlterAction::AddVersioning(_)
            | AlterAction::DropVersioning { .. }
            | AlterAction::RevertCollation(_) => Some("its columns and periods are its tree's, the same in each table of it"),
            AlterAction::Storage(_) | AlterAction::Truncate { .. } => {
                partitioned.then_some("it holds no rows: its partitions do, each altered or truncated itself")
            }
            AlterAction::AddConstraint(_)
            | AlterAction::DropConstraint { .. }
            | AlterAction::AttachPartition { .. }
            | AlterAction::DetachPartition(_) => None,
            AlterAction::RenameTable { .. } => Some("its tree knows it by its name"),
        };
        match refused_for {
            Some(why) => refused(format!("[EXTENSION] {qn} is partitioned or a partition, and {why} (RULES.md D153)")),
            None => Ok(()),
        }
    }
}
