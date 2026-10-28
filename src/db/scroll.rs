//! The rows a cursor is moved among by FETCH (`db/position.rs`): a
//! scrollable read-only cursor's, kept as they are read, and an updatable
//! cursor's, each derived as the cursor reaches it. Split from
//! db/cursor.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::position::{Move, Places, Position, Slot, fetch};
use super::{State, Stream, api};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{Runner, Subqueries, eval_cond};
use crate::logical::{Cond, Truth};
use crate::storage::{Row, RowId, TableStorage, project};
use crate::txn::TxId;

/// \[DESIGN\] A read-only SCROLL cursor's rows [ISO/IEC 9075:1992 §13.1]:
/// read from its stream as FETCH reaches them, and kept, so it may move
/// back.
pub(super) struct Kept {
    /// The rows still to read, and the tables they are read from.
    pub(super) stream: Stream,
    rows: Vec<Row>,
    /// Every row is read.
    done: bool,
    at: Position,
}

impl Kept {
    /// The rows of `stream`, the cursor before the first.
    pub(super) fn new(stream: Stream) -> Kept {
        Kept { stream, rows: Vec::new(), done: false, at: Position::Before(0) }
    }

    /// [ISO/IEC 9075:1992 §13.3] FETCH, moving as `to` says: the row it is
    /// then on, or none for no data.
    ///
    /// # Errors
    ///
    /// Reading a row failed: the cursor stays where it was \[GR7\].
    pub(super) fn fetch(&mut self, to: Move) -> Result<Option<Row>> {
        let from = self.at;
        let (at, row) = fetch(self, from, to)?;
        self.at = at;
        Ok(row)
    }

    /// Reads rows until the one at place `i` is read, or every one is.
    fn read_to(&mut self, i: usize) -> Result<()> {
        while self.rows.len() <= i && !self.done {
            match self.stream.next()? {
                Some(row) => self.rows.push(row),
                None => self.done = true,
            }
        }
        Ok(())
    }
}

impl Places for Kept {
    fn at(&mut self, i: usize) -> Result<Slot> {
        self.read_to(i)?;
        Ok(if i < self.rows.len() { Slot::Row } else { Slot::End })
    }
    fn row(&mut self, i: usize) -> Result<Row> {
        self.rows.get(i).cloned().ok_or_else(|| api(format!("[DESIGN] a cursor has no row {i}")))
    }
    fn count(&mut self) -> Result<usize> {
        self.read_to(usize::MAX)?;
        Ok(self.rows.len())
    }
}

/// An updatable cursor's rows, a working viewed table, each "derived only
/// when the cursor is positioned on that row" [X3.135-1986 §4.12]: which
/// rows it may cover, and in what order, is decided at OPEN; each is read
/// when the cursor reaches it, and covered only if it still satisfies
/// `condition`, the cursor's WHERE clause and its view's. A row deleted,
/// or changed so it no longer does, is passed over, which is where §4.12
/// puts the cursor for a deleted row ("positioned before the row that is
/// immediately after the position of the deleted row"). A row inserted, or
/// changed so it satisfies the condition, after OPEN is not covered: the
/// implementor-defined effect of an insertion on the cursor's position is
/// none. The cursor keeps the snapshot it was opened with, so every FETCH
/// reads through the definitions of its OPEN (RULES.md D27).
pub(super) struct Updatable {
    pub(super) snapshot: Arc<State>,
    /// The table or view its `<from clause>` names.
    pub(super) named: QualifiedName,
    /// The table it reads: `named`, or the updatable view's base table.
    pub(super) table: QualifiedName,
    /// The rows it may cover, in its order.
    pub(super) ids: Vec<RowId>,
    /// Of each of its columns, the column of `table` it is.
    pub(super) projection: Vec<usize>,
    pub(super) condition: Option<Cond>,
    pub(super) at: Position,
    /// The columns of `table` a positioned UPDATE may set: those of its
    /// FOR UPDATE [ISO/IEC 9075:1992 §13.1 SR13, SR14; §13.9 SR8].
    pub(super) settable: Vec<usize>,
    /// The columns of `table` its ORDER BY reads, which a positioned
    /// UPDATE may not set [§13.9 SR4].
    pub(super) ordered: Vec<usize>,
    /// The rows of `table` updated or deleted through it [§11.8 GR8].
    pub(super) changed: Vec<RowId>,
}

impl Updatable {
    /// The row the cursor is on, if it is on one.
    pub(super) fn current(&self) -> Option<RowId> {
        match self.at {
            Position::On(i) => self.ids.get(i).copied(),
            Position::Before(_) | Position::After => None,
        }
    }

    /// [X3.135-1986 §4.12] The current row deleted: the cursor is before
    /// the row after it.
    pub(super) fn deleted(&mut self) {
        if let Position::On(i) = self.at {
            self.at = Position::Before(i.saturating_add(1));
        }
    }

    /// [ISO/IEC 9075:1992 §13.3] FETCH by transaction `tx`, moving as `to`
    /// says: the values of the row it is then on, or none for no data.
    ///
    /// # Errors
    ///
    /// Reading a row, or evaluating its condition, failed: the cursor stays
    /// where it was \[GR7\].
    pub(super) fn fetch(&mut self, tx: TxId, to: Move) -> Result<Option<Row>> {
        let storage = Arc::clone(self.snapshot.storage_of(&self.table)?);
        let runner = Runner::new(&self.snapshot, tx);
        let mut places = Derived {
            storage: &*storage,
            tx,
            ids: &self.ids,
            projection: &self.projection,
            condition: self.condition.as_ref(),
            runner: &runner,
        };
        let (at, row) = fetch(&mut places, self.at, to)?;
        self.at = at;
        Ok(row)
    }
}

/// An updatable cursor's places, each row derived as transaction `tx`
/// sees it now.
struct Derived<'a> {
    storage: &'a dyn TableStorage,
    tx: TxId,
    ids: &'a [RowId],
    projection: &'a [usize],
    condition: Option<&'a Cond>,
    runner: &'a dyn Subqueries,
}

impl Places for Derived<'_> {
    fn at(&mut self, i: usize) -> Result<Slot> {
        let Some(&id) = self.ids.get(i) else { return Ok(Slot::End) };
        let Some(row) = self.storage.get(self.tx, id)? else { return Ok(Slot::Gone) };
        match self.condition.map_or(Ok(Truth::True), |c| eval_cond(c, &row, self.runner))? {
            Truth::True => Ok(Slot::Row),
            Truth::False | Truth::Unknown => Ok(Slot::Gone),
        }
    }
    fn row(&mut self, i: usize) -> Result<Row> {
        let id = self.ids.get(i).copied().ok_or_else(|| api(format!("[DESIGN] a cursor has no row {i}")))?;
        let row = self.storage.get(self.tx, id)?.ok_or_else(|| api(format!("[DESIGN] row {id} vanished")))?;
        project(&row, self.projection).ok_or_else(|| api(format!("[DESIGN] row {id} is shorter than the cursor's columns")))
    }
    fn count(&mut self) -> Result<usize> {
        Ok(self.ids.len())
    }
}

// Unit tests: an updatable cursor's places (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ColumnDesc, TableDesc};
    use crate::engine::rows::{ROW_STORE, RowStore, TableOptions};
    use crate::error::Code;
    use crate::exec::NoSubqueries;
    use crate::storage::StorageEngine;
    use crate::txn::NO_TRANSACTION;
    use crate::value::Spelled;
    use crate::value::{DataType, Decimal, Value};

    fn number(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// Verifies: [X3.135-1986 §4.12] a row deleted since OPEN is gone, and
    /// the values come from the cursor's columns; [DESIGN] a projection
    /// past the end of a row is reported, not a panic.
    #[test]
    fn a_deleted_row_is_gone() {
        let column = |name: &str| ColumnDesc {
            name: name.into(),
            data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
            not_null: false,
            default: None,
            domain: None,
        };
        let desc = TableDesc {
            name: QualifiedName { auth: "A".into(), ident: "T".into() },
            columns: vec![column("K"), column("V")],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        let table = RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap();
        let tx = NO_TRANSACTION;
        let first = table.insert(tx, vec![number(1), number(10)]).unwrap();
        let second = table.insert(tx, vec![number(2), number(20)]).unwrap();
        table.delete(tx, first).unwrap();
        let ids = [first, second];
        let mut places = Derived { storage: table.as_ref(), tx, ids: &ids, projection: &[1], condition: None, runner: &NoSubqueries };
        assert!(matches!(places.at(0).unwrap(), Slot::Gone));
        assert!(matches!(places.at(1).unwrap(), Slot::Row));
        assert!(matches!(places.at(2).unwrap(), Slot::End));
        assert_eq!(places.row(1).unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(), vec!["20".to_owned()]);
        assert_eq!(places.count().unwrap(), 2);
        places.projection = &[5];
        assert_eq!(places.row(1).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(places.row(0).err().map(|e| e.code), Some(Code::Api), "deleted");
    }
}
