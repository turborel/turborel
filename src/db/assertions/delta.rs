//! \[EXTENSION\] A form whose rows are a query's (`queried.rs`), checked by
//! the query run over the rows changed [ISO/IEC 9075:1992 §4.10.1, §11.34
//! GR2] (RULES.md D51).
//!
//! - **Joins.** Each scan in turn is made of its table's rows changed, as
//!   they are now, as constant rows; the others read their tables as they
//!   are now. A scan of a table the statement did not change is of no row.
//! - **Groupings.** The groups of the rows changed, as they are now and as
//!   they were, are made of the table's rows now: the query run with its
//!   grouping columns not distinct from the keys of those groups.
//!
//! The rows changed, or the groups' keys, are held to as many bytes as
//! MEMORY.WORK_BYTES says, reserved of the memory budget; a change of
//! more, or one the budget cannot hold, is checked whole, which streams
//! the tables it reads.
//!
//! The query reads as a constraint check reads, serializably at every
//! level (RULES.md D29): its index probes and scans count as reading their
//! tables whole, so a row another transaction commits meanwhile that would
//! make a row of the query with a row changed fails one of the two
//! transactions at commit, as of a whole check.
//!
//! Of a grouping whose set function's argument may be null, whether the
//! warning of a null eliminated is raised [§6.5 GR1b] depends on every
//! group: so it is checked whole unless a warning is raised once the
//! groups changed are made, by them or before. Only the first warning is
//! kept (`Warnings`), so then the whole check could raise none other.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::{State, plan};
use super::changes::{Seen, passes};
use super::forms::{Form, Kind, Step};
use super::queried::{Query, Shape, of_groups, scans, with_rows};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{Heard, eval, execute};
use crate::logical::{Expr, LogicalPlan};
use crate::memory::Reservation;
use crate::settings::{MEMORY_WORK_BYTES, work_bytes};
use crate::spill::{norm_row, row_bytes};
use crate::storage::{Row, RowId};
use crate::txn::Txn;
use crate::value::DataType;

/// The rows a statement changed, or a transaction while an assertion was
/// deferred, as a check of it reads them.
pub(super) trait Changes {
    /// Calls `each` of each row of `table` changed: its id, and the row as
    /// it was before the change, if it was and is kept.
    ///
    /// # Errors
    ///
    /// `each`'s, or reading what was changed.
    fn visit(&mut self, table: &QualifiedName, each: &mut dyn FnMut(RowId, Option<&Row>) -> Result<()>) -> Result<()>;
}

/// The bytes held of the rows changed: up to a limit, reserved of the
/// memory budget.
pub(super) struct Held {
    limit: usize,
    bytes: usize,
    reserved: Option<Reservation>,
    /// More were asked for than the limit, or the budget, allows.
    pub(super) over: bool,
}

impl Held {
    /// None yet, of as many as MEMORY.WORK_BYTES says.
    pub(super) fn new(st: &State) -> Held {
        Held { limit: work_bytes(st.settings.get(MEMORY_WORK_BYTES)), bytes: 0, reserved: None, over: false }
    }

    /// Whether `more` bytes are held too, within the limit and granted by
    /// `st`'s budget; if not, none are from now on.
    pub(super) fn take(&mut self, st: &State, more: usize) -> bool {
        let bytes = self.bytes.saturating_add(more);
        let granted = !self.over
            && bytes <= self.limit
            && match &mut self.reserved {
                Some(reserved) => reserved.resize(bytes).is_ok(),
                None => st.memory.reserve(bytes, "an assertion's rows changed").map(|r| self.reserved = Some(r)).is_ok(),
            };
        if granted {
            self.bytes = bytes;
        } else {
            self.over = true;
        }
        granted
    }
}

impl State {
    /// What the rows `changes` holds show of `form`, its rows those of
    /// `query`: none if only checking it whole tells.
    ///
    /// # Errors
    ///
    /// The query's, or reading the rows changed.
    pub(super) fn query_seen(
        self: &Arc<Self>,
        tx: &Txn,
        (form, query): (&Form, &Query),
        changes: &mut dyn Changes,
    ) -> Result<Option<Seen>> {
        match &query.shape {
            Shape::Joined => self.joined_seen(tx, (form, &query.plan), changes),
            Shape::Grouped { table, steps, keys, nullable } => {
                let Some(groups) = self.changed_groups(tx, (table, steps, keys), changes)? else { return Ok(None) };
                let mut seen = Seen::default();
                if !groups.is_empty() {
                    let types = self.key_types(table, keys);
                    self.seen_of(tx, form, of_groups(query.plan.clone(), (&groups, types.as_deref())), &mut seen)?;
                }
                // Whether a null is eliminated in a group not made.
                Ok((!nullable || tx.stop.warnings().raised()).then_some(seen))
            }
        }
    }

    /// Of inner joins `plan`, what each scan in turn shows made of its
    /// table's rows changed: none if they are past what is held.
    fn joined_seen(self: &Arc<Self>, tx: &Txn, (form, plan): (&Form, &LogicalPlan), changes: &mut dyn Changes) -> Result<Option<Seen>> {
        let scanned = scans(plan);
        let (mut held, mut written) = (Held::new(self), HashMap::<&QualifiedName, Vec<Row>>::new());
        for table in &scanned {
            if written.contains_key(table) {
                continue;
            }
            let storage = self.storage_of(table)?;
            let mut rows = Vec::new();
            changes.visit(table, &mut |id, _| {
                if let Some(row) = storage.get(tx.id, id)?
                    && held.take(self, row_bytes(&row))
                {
                    rows.push(row);
                }
                Ok(())
            })?;
            if held.over {
                return Ok(None);
            }
            written.insert(table, rows);
        }
        let mut seen = Seen::default();
        for (at, table) in scanned.iter().enumerate() {
            let Some(rows) = written.get(table).filter(|rows| !rows.is_empty()) else { continue };
            if seen.passing.now && matches!(form.kind, Kind::NoRow) {
                break;
            }
            let types = self.column_types(table);
            self.seen_of(tx, form, with_rows(plan.clone(), at, (rows, &types)), &mut seen)?;
        }
        Ok(Some(seen))
    }

    /// The keys of the groups of `table`'s rows changed, now and before, of
    /// the rows passing `steps`, each the values of `keys`, once each: none
    /// if they are past what is held, or a row before is of other columns
    /// than the table has now.
    fn changed_groups(
        &self,
        tx: &Txn,
        (table, steps, keys): (&QualifiedName, &[Step], &[Expr]),
        changes: &mut dyn Changes,
    ) -> Result<Option<Vec<Row>>> {
        let (storage, width) = (self.storage_of(table)?, self.catalog.table(table).map_or(0, |d| d.columns.len()));
        let heard = Heard(tx.stop.warnings());
        let (mut held, mut once, mut groups) = (Held::new(self), HashSet::new(), Vec::new());
        changes.visit(table, &mut |id, before| {
            let now = storage.get(tx.id, id)?;
            for row in [now.as_ref(), before].into_iter().flatten() {
                held.over = held.over || row.len() != width;
                if held.over || !passes(steps, row, tx)? {
                    continue;
                }
                let key: Row = keys.iter().map(|k| eval(k, row, &heard)).collect::<Result<_>>()?;
                // Keys not distinct are of one group [ISO/IEC 9075:1992 §7.7 GR1].
                if once.insert(norm_row(&key)) && held.take(self, row_bytes(&key)) {
                    groups.push(key);
                }
            }
            Ok(())
        })?;
        Ok((!held.over).then_some(groups))
    }

    /// The types of `table`'s columns.
    fn column_types(&self, table: &QualifiedName) -> Vec<DataType> {
        self.catalog.table(table).map(|d| d.columns.iter().map(|c| c.data_type).collect()).unwrap_or_default()
    }

    /// The types of a grouping's `keys`, expressions over `table`'s row,
    /// if each is one of its columns.
    fn key_types(&self, table: &QualifiedName, keys: &[Expr]) -> Option<Vec<DataType>> {
        let types = self.column_types(table);
        keys.iter()
            .map(|k| match k {
                Expr::Column(at) => types.get(*at).copied(),
                Expr::Const(_)
                | Expr::Outer(..)
                | Expr::Dynamic(_)
                | Expr::Neg(_)
                | Expr::Arith(..)
                | Expr::Call(..)
                | Expr::Case(..)
                | Expr::Cast(..)
                | Expr::SortKey(..)
                | Expr::Domain(..)
                | Expr::Subquery(..)
                | Expr::Truth(_) => None,
            })
            .collect()
    }

    /// Takes in `seen` each row `rows`, a form's query made of rows
    /// changed, gives, as the rows of a table changed are taken in: those
    /// passing `form`'s steps, and of its argument, whether one is null.
    /// Of no row passing it stops at the first passing, as EXISTS does.
    fn seen_of(self: &Arc<Self>, tx: &Txn, form: &Form, rows: LogicalPlan, seen: &mut Seen) -> Result<()> {
        let (_, _, physical) = plan(self, rows);
        let heard = Heard(tx.stop.warnings());
        for row in execute(physical, self, tx.id) {
            let row = row?;
            let is_in = passes(&form.steps, &row, tx)?;
            if let (Some((at, x)), false) = (&form.nulls, seen.null) {
                seen.null = passes(form.steps.get(..*at).unwrap_or_default(), &row, tx)? && eval(x, &row, &heard)?.is_null();
            }
            seen.row(&form.kind, (Some(&row), None), (is_in, false), tx)?;
            if is_in && matches!(form.kind, Kind::NoRow) {
                break;
            }
        }
        Ok(())
    }
}
