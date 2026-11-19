//! \[EXTENSION\] Rows routed to the partitions that hold them (RULES.md
//! D155). A row written to a partitioned table goes down its tree, at each
//! level to the partition whose bound holds its key, else the DEFAULT
//! partition, else nowhere, and the statement fails with SQLSTATE 23T01. A
//! row written to a partition itself must be one its bound, and each of
//! its ancestors' bounds, holds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::BoundPartitionKey;
use crate::catalog::{Catalog, MAX_PARTITION_DEPTH, PartitionBound, QualifiedName};
use crate::db::integrity::Effects;
use crate::db::write::Written;
use crate::db::{State, StatementContext, set_columns};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::Runner;
use crate::storage::{Row, RowId};
use crate::txn::{Again, Txn};

/// How rows are routed among one partitioned table's partitions: its key,
/// bound, and its partitions, each with its bound, the DEFAULT apart.
pub(crate) struct Route {
    key: BoundPartitionKey,
    partitions: Vec<(QualifiedName, PartitionBound)>,
    default: Option<QualifiedName>,
}

/// The routes of the partitioned tables written, made once of a catalog
/// and kept while it is the one written by: held with them, so no other
/// catalog is taken for it.
#[derive(Default)]
pub(crate) struct Routes(Mutex<Option<Kept>>);

/// The routes made of one catalog, with it.
type Kept = (Arc<dyn Catalog>, HashMap<QualifiedName, Arc<Route>>);

impl State {
    /// The route of the partitioned table `table`, if it is one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its strategy is not registered; a key expression's
    /// binding errors.
    fn route_of(&self, table: &QualifiedName) -> Result<Option<Arc<Route>>> {
        if !self.catalog.partitioning().is_partitioned(table) {
            return Ok(None);
        }
        let routes = &self.strategies.routes;
        {
            let held = routes.0.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((catalog, kept)) = held.as_ref()
                && Arc::ptr_eq(catalog, &self.catalog)
                && let Some(route) = kept.get(table)
            {
                return Ok(Some(Arc::clone(route)));
            }
        }
        let Some(key) = self.partition_key(table)? else { return Ok(None) };
        let (mut partitions, mut default) = (Vec::new(), None);
        for child in self.catalog.partitioning().children(table) {
            let Some(of) = &child.of else { continue };
            match &of.bound {
                PartitionBound::Default => default = Some(child.table.clone()),
                bound @ (PartitionBound::Range { .. } | PartitionBound::List(_) | PartitionBound::With(_)) => {
                    partitions.push((child.table.clone(), bound.clone()));
                }
            }
        }
        let route = Arc::new(Route { key, partitions, default });
        let mut held = routes.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held.as_ref().is_none_or(|(catalog, _)| !Arc::ptr_eq(catalog, &self.catalog)) {
            *held = Some((Arc::clone(&self.catalog), HashMap::new()));
        }
        if let Some((_, kept)) = held.as_mut() {
            kept.insert(table.clone(), Arc::clone(&route));
        }
        Ok(Some(route))
    }

    /// The table that holds `row` written to `table`: the partition at
    /// the bottom of its tree whose bounds hold it, or `table` itself if
    /// it is not partitioned.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23T01, if no partition holds it; a key
    /// expression's errors.
    pub(crate) fn route_row(&self, table: &QualifiedName, row: &Row) -> Result<QualifiedName> {
        let mut at = table.clone();
        for _ in 0..=MAX_PARTITION_DEPTH {
            let Some(route) = self.route_of(&at)? else { return Ok(at) };
            let key = route.key.key.computed_key(row)?;
            let bounds: Vec<&PartitionBound> = route.partitions.iter().map(|(_, b)| b).collect();
            let found = route.key.strategy.route(&bounds, &key.0)?;
            let next = found.and_then(|k| route.partitions.get(k)).map(|(t, _)| t).or(route.default.as_ref());
            at = next.cloned().ok_or_else(|| no_partition(format!("[EXTENSION] no partition of {at} holds the row (RULES.md D155)")))?;
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] the tree of {table} is deeper than {MAX_PARTITION_DEPTH}")))
    }

    /// [X3.135-1986 §8.7 GR1c] Inserts `row`, written to `table`, into the
    /// table that holds it (`route_row`): that table, and the row's id.
    ///
    /// # Errors
    ///
    /// As `route_row` and `insert_row`.
    pub(crate) fn insert_routed(&self, tx: &Txn, table: &QualifiedName, row: &Row) -> Result<(QualifiedName, RowId)> {
        let leaf = self.route_row(table, row)?;
        let id = self.insert_row(tx, &leaf, row)?;
        Ok((leaf, id))
    }

    /// That the partition `table` holds `row`, written to it: its bound and
    /// each of its ancestors' hold it. Of a table that is no partition,
    /// nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23T01, if one does not; SQLCODE -910 if
    /// `table` is partitioned, so holds no rows.
    pub(crate) fn check_holds(&self, table: &QualifiedName, row: &Row) -> Result<()> {
        let p = self.catalog.partitioning();
        if p.is_partitioned(table) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {table} is partitioned: its rows are its partitions'")));
        }
        let Some(root) = p.ancestors(table).pop() else { return Ok(()) };
        if self.route_row(&root, row).ok().as_ref() == Some(table) {
            return Ok(());
        }
        Err(no_partition(format!("[EXTENSION] the partition {table} does not hold the row, by its bound (RULES.md D155)")))
    }

    /// Inserts `row`, a row an UPDATE or MERGE moved out of its partition,
    /// where it is routed from `named`, the table the statement names:
    /// the partition, the row's id there, and the row as inserted, of a
    /// system-versioned table its period starting now.
    ///
    /// # Errors
    ///
    /// As `route_row` and `insert_row`.
    pub(crate) fn insert_moved(&self, tx: &Txn, named: &QualifiedName, row: &Row) -> Result<(QualifiedName, RowId, Row)> {
        let leaf = self.route_row(named, row)?;
        let mut row = row.clone();
        if let Some(stamp) = self.stamp_of(tx, &leaf)? {
            stamp.put(&mut row);
        }
        let id = self.insert_row(tx, &leaf, &row)?;
        Ok((leaf, id, row))
    }

    /// Whether `row`, written by a statement on `named` to `table`, a
    /// partition of its tree, is to be moved to another partition: its
    /// key leaves the partition's bounds, and the statement names the
    /// partitioned table, whose tree it is routed in again.
    pub(crate) fn moves(&self, (named, table): (&QualifiedName, &QualifiedName), row: &Row) -> bool {
        let p = self.catalog.partitioning();
        named != table && p.is_partitioned(named) && p.of(table).is_some() && self.check_holds(table, row).is_err()
    }
}

/// Row `id` of the partition the UPDATE `w` writes, made `new`, its key
/// now another partition's: deleted from its partition, its old version
/// kept in its history if it has one, then inserted where it is routed
/// from the table the statement names, stamped as of now, both noted in
/// `effects`; if another transaction changed it so its search condition no
/// longer holds, left alone, as noted in `again` (RULES.md D29).
///
/// # Errors
///
/// As `route_row`, `rewrite_row` and `insert_row`.
pub(in crate::db) fn move_row(
    cx: StatementContext<'_>,
    (w, runner): (&Written, &Runner),
    (id, new): (RowId, &Row),
    (effects, again): (&mut Effects<'_>, &mut HashMap<RowId, Option<Row>>),
) -> Result<()> {
    let StatementContext { st, tx } = cx;
    let table = &w.bound.table;
    let mut recheck = |newest: &Row| Ok(if w.holds_of(newest, runner)? { Again::Delete } else { Again::Skip });
    let Some(old) = st.rewrite_row(tx, (table, id), None, &mut recheck)? else {
        again.insert(id, None);
        return Ok(());
    };
    let (leaf, moved, new) = st.insert_moved(tx, &w.bound.named, new)?;
    effects.moved((table, id, old), (&leaf, moved, &new), &set_columns(w))
}

/// SQLCODE -201, SQLSTATE 23T01, of `message`.
fn no_partition(message: String) -> SqlError {
    SqlError::new(Code::Constraint, message).with_state(SqlState::NO_PARTITION)
}
