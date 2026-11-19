//! \[EXTENSION\] Global indexes (RULES.md D157): of a UNIQUE constraint of
//! a partitioned table whose key lacks a partition key of its tree, one
//! index over the rows of every partition, keyed by the constraint's
//! columns. Each entry names a row by its partition's id and its id in
//! the partition, packed into one row id (`packed`). It lives on the
//! partitioned table, whose storage holds no row, so its structure is
//! kept, checkpointed and reopened as any index; its entries are written
//! with each partition's rows (`txn/write.rs`), recovered with them
//! (`db/recovery.rs`) and read back through `Dispatched`.

use std::sync::Arc;

use super::super::alter::Made;
use super::super::{State, api};
use crate::catalog::{Catalog, IndexDesc, IndexTemplate, QualifiedName, global_index_name, is_global_index};
use crate::error::{Code, Result, SqlError};
use crate::index::Index;
use crate::storage::{RowId, Rows};
use crate::txn::{Timestamp, Txn};

/// \[DESIGN\] The bits of a packed row id that are the row's id in its
/// partition; the bits above them are the partition's id.
const ROW_BITS: u32 = 40;

/// \[DESIGN\] The least partition id a packed row id cannot hold.
const MOST_PARTITION: u64 = 1 << (u64::BITS - ROW_BITS);

/// The row id of a global index's entry for row `row` of the partition of
/// id `partition`.
///
/// # Errors
///
/// SQLCODE -900 if either is past what a packed row id holds.
pub(crate) fn packed(partition: u64, row: RowId) -> Result<RowId> {
    if partition >= MOST_PARTITION || row >> ROW_BITS != 0 {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[EXTENSION] row {row} of partition {partition} is past what a global index names (RULES.md D157)"),
        ));
    }
    Ok(partition << ROW_BITS | row)
}

/// The partition's id and the row's id in it, of a packed row id.
pub(crate) fn unpacked(id: RowId) -> (u64, RowId) {
    (id >> ROW_BITS, id & ((1 << ROW_BITS) - 1))
}

/// Whether a template of `owner` of the key `columns` needs a global index:
/// a partitioned table from `owner` down is partitioned by a key whose
/// columns it lacks, or by an expression, so two rows of one key may be in
/// two partitions.
fn needs_global(catalog: &dyn Catalog, owner: &QualifiedName, columns: &[usize]) -> bool {
    let p = catalog.partitioning();
    let tables = std::iter::once(owner.clone()).chain(p.descendants(owner));
    let keys: Vec<_> = tables.filter_map(|t| p.key(&t).cloned()).collect();
    keys.iter().any(|key| !key.expressions.is_empty() || !key.columns.iter().all(|c| columns.contains(c)))
}

impl State {
    /// The global indexes over the rows of `leaf`, a partition holding rows,
    /// each with its id, which its entries are packed with.
    pub(crate) fn globals_over(&self, leaf: &QualifiedName) -> Vec<(IndexDesc, u64)> {
        let p = self.catalog.partitioning();
        let Some(of) = p.of(leaf).filter(|_| !p.is_empty() && !p.is_partitioned(leaf)) else { return Vec::new() };
        let mut out = Vec::new();
        for ancestor in p.ancestors(leaf) {
            out.extend(self.catalog.indexes(&ancestor).into_iter().filter(|ix| is_global_index(&ix.name)).map(|ix| (ix.clone(), of.id)));
        }
        out
    }

    /// The global indexes of `table`'s tree in `catalog` made again: one a
    /// UNIQUE template needs made, `made`'s or reopened, else filled from
    /// the partitions of the state's tree, of the rows `tx` sees if given
    /// (`fill_seen`), noted in `built`; one no longer needed dropped. What
    /// it dropped.
    ///
    /// # Errors
    ///
    /// As `make_global`.
    pub(in crate::db) fn sync_globals(
        &mut self,
        catalog: &mut dyn Catalog,
        table: &QualifiedName,
        (built, made, tx): (&mut Made, Option<&Made>, Option<&Txn>),
    ) -> Result<Vec<Arc<dyn Index>>> {
        let p = catalog.partitioning();
        if p.of(table).is_none() && !p.is_partitioned(table) {
            return Ok(Vec::new());
        }
        let root = p.root(table);
        let owners: Vec<QualifiedName> =
            std::iter::once(root.clone()).chain(p.descendants(&root)).filter(|t| p.is_partitioned(t)).collect();
        // [DESIGN] A temporary tree's rows are each session's own, few, and
        // checked across its partitions.
        let temporary = catalog.table(&root).is_some_and(|d| d.temporary.is_some());
        let mut dropped = Vec::new();
        for owner in &owners {
            let templates = catalog.partitioning().key(owner).map(|k| k.templates.clone()).unwrap_or_default();
            let wanted: Vec<IndexTemplate> =
                templates.into_iter().filter(|t| t.unique && !temporary && needs_global(catalog, owner, &t.columns)).collect();
            let names: Vec<String> = wanted.iter().map(|t| global_index_name(&t.name)).collect();
            let had: Vec<String> =
                catalog.indexes(owner).into_iter().filter(|ix| is_global_index(&ix.name)).map(|ix| ix.name.clone()).collect();
            for name in had.iter().filter(|n| !names.contains(n)) {
                catalog.remove_index(name);
                dropped.extend(self.indexes.remove(name));
            }
            let missing: Vec<&IndexTemplate> = wanted.iter().filter(|t| !had.contains(&global_index_name(&t.name))).collect();
            for t in missing {
                self.make_global(catalog, owner, t, (built, made, tx))?;
            }
        }
        Ok(dropped)
    }

    /// The global index of the template `t` of `owner`, made in `catalog`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its method is not registered; the method's, making
    /// or filling it.
    fn make_global(
        &mut self,
        catalog: &mut dyn Catalog,
        owner: &QualifiedName,
        t: &IndexTemplate,
        (built, made, tx): (&mut Made, Option<&Made>, Option<&Txn>),
    ) -> Result<()> {
        let desc = catalog.table(owner).cloned().ok_or_else(|| api(format!("[DESIGN] no table {owner}")))?;
        let ix = IndexDesc {
            name: global_index_name(&t.name),
            table: owner.clone(),
            columns: t.columns.clone(),
            method: t.method.clone(),
            unique: false,
            options: t.options.clone(),
            collations: Vec::new(),
            nulls: t.nulls,
            expressions: Vec::new(),
        };
        let ix = self.bound_derived(ix, &desc)?;
        let kept = made.and_then(|m| m.indexes.iter().find(|(n, _)| *n == ix.name)).map(|(_, s)| Arc::clone(s));
        let structure = if let Some(structure) = kept {
            structure
        } else {
            let (structure, reopened) = self.derived_structure(&ix, &desc)?;
            if !reopened {
                match tx {
                    Some(tx) => self.fill_seen(&*structure, &ix, tx)?,
                    None => self.load_index(&*structure, &ix, Timestamp::MAX)?,
                }
            }
            structure
        };
        self.indexes.insert(ix.name.clone(), Arc::clone(&structure));
        built.indexes.push((ix.name.clone(), structure));
        catalog.add_index(ix);
        Ok(())
    }

    /// Fills `structure`, new, of the global index `ix`, with an entry of
    /// each row of each partition of the state's tree that `tx` sees, which
    /// are those every snapshot sees once it commits, as it holds the
    /// partitioned table's exclusive lock, as a UNIQUE constraint's index
    /// is filled (`State::unique_index`).
    ///
    /// # Errors
    ///
    /// As reading the rows and filling the index.
    fn fill_seen(&self, structure: &dyn Index, ix: &IndexDesc, tx: &Txn) -> Result<()> {
        let p = self.catalog.partitioning();
        for leaf in p.leaves(&ix.table) {
            let Some(partition) = p.of(&leaf).filter(|_| !p.is_partitioned(&leaf)).map(|o| o.id) else { continue };
            self.read_lock(tx, &leaf)?;
            let width = self.catalog.table(&leaf).map_or(0, |d| d.columns.len());
            let all: Vec<usize> = (0..width).collect();
            for read in Arc::clone(self.storage_of(&leaf)?).scan(tx.id, &all) {
                let (id, row) = read?;
                if let Some(key) = ix.entry_of(&row) {
                    structure.insert(key, packed(partition, id)?)?;
                }
            }
        }
        Ok(())
    }

    /// The versions readers may still see of the rows of each partition
    /// under the global index `ix`'s table, as of `as_of`, each named by
    /// its packed row id: what fills the index.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a partition has no storage.
    pub(in crate::db) fn global_versions(&self, ix: &IndexDesc, as_of: Timestamp) -> Result<Rows> {
        let p = self.catalog.partitioning();
        let mut each: Vec<Rows> = Vec::new();
        for leaf in p.leaves(&ix.table) {
            let Some(id) = p.of(&leaf).filter(|_| !p.is_partitioned(&leaf)).map(|o| o.id) else { continue };
            let versions = Arc::clone(self.storage_of(&leaf)?).versions(self.tx.horizon(), as_of, &ix.reads());
            each.push(Box::new(versions.map(move |read| read.and_then(|(row, values)| Ok((packed(id, row)?, values))))));
        }
        Ok(Box::new(each.into_iter().flatten()))
    }

    /// The entries of the rows of `table`'s partitions, or its own if it
    /// holds rows, added to the global indexes over them (`index_partition_rows`).
    ///
    /// # Errors
    ///
    /// As `index_partition_rows`.
    pub(in crate::db) fn index_partitions(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.index_partition_rows(tx, &self.catalog.partitioning().leaves(table), true)
    }

    /// As `index_partitions`, the entries taken back.
    ///
    /// # Errors
    ///
    /// As `index_partition_rows`.
    pub(in crate::db) fn unindex_partitions(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.index_partition_rows(tx, &self.catalog.partitioning().leaves(table), false)
    }

    /// The entries of the rows of `leaves`, as the state's tree places them,
    /// added to the global indexes over them, or taken back if not `add`,
    /// each undone if `tx` rolls back: of a table attached, before its rows
    /// are checked; of one detached, truncated, given new storage or
    /// dropped, as it goes.
    ///
    /// # Errors
    ///
    /// As reading the rows and changing the indexes.
    fn index_partition_rows(&self, tx: &Txn, leaves: &[QualifiedName], add: bool) -> Result<()> {
        let change = if add { Self::index_add } else { Self::index_remove };
        for leaf in leaves {
            let globals = self.globals_over(leaf);
            if globals.is_empty() {
                continue;
            }
            self.read_lock(tx, leaf)?;
            let width = self.catalog.table(leaf).map_or(0, |d| d.columns.len());
            let columns: Vec<usize> = (0..width).collect();
            for read in Arc::clone(self.storage_of(leaf)?).scan(tx.id, &columns) {
                let (id, row) = read?;
                for (ix, partition) in &globals {
                    let entry = packed(*partition, id)?;
                    change(self, tx, ix, &row, entry)?;
                }
            }
        }
        Ok(())
    }
}
