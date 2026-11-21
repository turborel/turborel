//! The table rows a FULL outer index join keeps, its U2 extended on the
//! left with nulls [ISO/IEC 9075:1992 §7.5 GR3, GR4] (RULES.md D135):
//! after its probes, each row of the table, as the statement reads it,
//! that its own filter is true of and that no probe paired. The probes record the ids
//! of the rows they paired, a set charged to the memory budget (D38); a
//! row read again after its version changed keeps its id, so it is
//! matched once. The table is read in full, as its serializable read is.

use std::collections::HashSet;
use std::sync::Arc;

use super::Probed;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::exec::Db;
use crate::exec::{Runner, eval_cond};
use crate::logical::Truth;
use crate::memory::Reservation;
use crate::physical::{Preserved, Unmatched};
use crate::storage::{self, Row, RowId, Rows};
use crate::txn::checked;
use crate::value::Value;

/// The first set's capacity, so a join of few matches allocates once.
const INITIAL: usize = 64;
/// Geometric growth bounds reallocations as matches accumulate.
const GROWTH: usize = 2;
/// Bytes charged for each slot: the id and the hash table's own control
/// byte and spare capacity, at most as large again.
const SLOT_BYTES: usize = 2 * size_of::<RowId>();

pub(super) struct Matched {
    rows: Ids,
    unmatched: Unmatched,
    /// The table's columns the query reads.
    columns: Vec<usize>,
    /// The table's rows, read once the probes are done.
    rest: Option<Rows>,
}

impl Matched {
    /// Of a FULL join, the set its probes fill; none of another join.
    pub(super) fn of(outer: Preserved, columns: &[usize], db: &Db) -> Result<Option<Matched>> {
        let Preserved::Both(unmatched) = outer else { return Ok(None) };
        Ok(Some(Matched { rows: Ids::new(db)?, unmatched: *unmatched, columns: columns.to_vec(), rest: None }))
    }

    /// Whether the probes are done and the table is being read.
    pub(super) fn started(&self) -> bool {
        self.rest.is_some()
    }

    /// Records that a probe paired the row `id`.
    pub(super) fn add(&mut self, id: RowId) -> Result<()> {
        self.rows.add(id)
    }

    /// The next table row no probe paired, after the outer row's nulls,
    /// followed by its id if the table's rows are located.
    pub(super) fn next(&mut self, table: &Probed, runner: &Runner) -> Result<Option<Row>> {
        let width = self.unmatched.width;
        let rows = self.rest.get_or_insert_with(|| {
            let read = Arc::clone(&table.storage).scan(table.tx, &self.columns);
            Box::new(checked(read, table.stop.clone()))
        });
        while let Some((id, row)) = rows.next().transpose()? {
            if self.rows.contains(id) {
                continue;
            }
            let mut joined: Row = std::iter::repeat_n(Value::Null, width).chain(row).collect();
            if table.located {
                joined.push(storage::located(id));
            }
            let kept = match &self.unmatched.filter {
                Some(filter) => eval_cond(filter, &joined, runner)? == Truth::True,
                None => true,
            };
            if kept {
                return Ok(Some(joined));
            }
        }
        Ok(None)
    }
}

/// The ids of the rows a join paired, a set charged to the memory budget.
pub(super) struct Ids {
    rows: HashSet<RowId>,
    held: Reservation,
}

impl Ids {
    pub(super) fn new(db: &State) -> Result<Ids> {
        Ok(Ids { rows: HashSet::new(), held: db.memory.reserve(0, "the rows an outer join matched")? })
    }

    /// Records that the row `id` was paired.
    pub(super) fn add(&mut self, id: RowId) -> Result<()> {
        if self.rows.len() == self.rows.capacity() {
            let capacity = self.rows.capacity().checked_mul(GROWTH).ok_or_else(exhausted)?.max(INITIAL);
            self.held.resize(capacity.checked_mul(SLOT_BYTES).ok_or_else(exhausted)?)?;
            self.rows.try_reserve(capacity.saturating_sub(self.rows.len())).map_err(|_| exhausted())?;
        }
        self.rows.insert(id);
        Ok(())
    }

    pub(super) fn contains(&self, id: RowId) -> bool {
        self.rows.contains(&id)
    }
}

fn exhausted() -> SqlError {
    SqlError::new(Code::Resources, "[DESIGN] the rows an outer join matched exceed the memory budget")
}
