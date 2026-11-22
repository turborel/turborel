//! The rows a cursor is moved among by FETCH (`db/position.rs`): a scrollable
//! read-only cursor's, kept as they are read, and an updatable cursor's, each
//! derived as the cursor reaches it. Each cursor holds its rows, or their ids,
//! up to `settings::held_bytes`, the rest spilled (`exec/held.rs`, RULES.md D38).

use std::sync::Arc;

use super::position::{Move, Places, Position, Slot, fetch};
use super::{State, Stream, api};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{HeldRows, RowIter, Runner, Subqueries, eval, eval_cond, sort_rows, table_entries};
use crate::logical::{Cond, Expr, SortKey, Truth};
use crate::physical::Plan;
use crate::storage::{Row, RowId, TableStorage, project};
use crate::txn::TxId;
use crate::value::{Decimal, Value};

/// \[DESIGN\] A read-only SCROLL cursor's rows [ISO/IEC 9075:1992 §13.1]:
/// read from its stream as FETCH reaches them, and kept, so it may move
/// back.
pub(super) struct Kept {
    /// The rows still to read, and the tables they are read from.
    pub(super) stream: Stream,
    rows: HeldRows<Row>,
    /// Every row is read.
    done: bool,
    at: Position,
}

impl Kept {
    /// The rows of `stream`, the cursor before the first, kept as the
    /// statement's work area holds them (`State::list_space`).
    pub(super) fn new(stream: Stream, st: &State) -> Kept {
        Kept { stream, rows: HeldRows::new(st.list_space()), done: false, at: Position::Before(0) }
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
                Some(row) => self.rows.push(row)?,
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
        self.rows.get(i)?.ok_or_else(|| api(format!("[DESIGN] a cursor has no row {i}")))
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
    pub(super) ids: HeldRows<RowId>,
    /// [ISO/IEC 9075-2:2023 §4.17.9] Of a typed table read with its proper
    /// subtables, those subtables, and of each row the table it is of: 0
    /// for `table`, k for the subtable k - 1. Empty, and none, of another.
    pub(super) subtables: Vec<QualifiedName>,
    pub(super) of: Option<HeldRows<RowId>>,
    /// [ISO/IEC 9075-2:2023 §14.8 SR9, §14.13 SR9] Its `<table reference>`
    /// is ONLY: a positioned UPDATE or DELETE through it names ONLY too.
    pub(super) only: bool,
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
    /// The rows updated or deleted through it, each with its table
    /// [§11.8 GR8].
    pub(super) changed: Vec<(QualifiedName, RowId)>,
    /// The row it is on, if it is on one: the id at `at`, and the table
    /// it is of, as `of` gives it.
    pub(super) on: Option<(usize, RowId)>,
}

impl Updatable {
    /// The row the cursor is on, if it is on one, and the table it is of:
    /// `table`, or one of its proper subtables.
    pub(super) fn current(&self) -> Option<(&QualifiedName, RowId)> {
        let (k, id) = self.on?;
        let table = if k == 0 { Some(&self.table) } else { self.subtables.get(k.saturating_sub(1)) };
        Some((table?, id))
    }

    /// Whether it reads `table`: its own, or one of its subtables.
    pub(super) fn reads(&self, table: &QualifiedName) -> bool {
        self.table == *table || self.subtables.contains(table)
    }

    /// [X3.135-1986 §4.12] The current row deleted: the cursor is before
    /// the row after it.
    pub(super) fn deleted(&mut self) {
        if let Position::On(i) = self.at {
            self.at = Position::Before(i.saturating_add(1));
            self.on = None;
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
        let storages = std::iter::once(&self.table)
            .chain(&self.subtables)
            .map(|t| self.snapshot.storage_of(t).map(Arc::clone))
            .collect::<Result<Vec<_>>>()?;
        let runner = Runner::new(&self.snapshot, tx);
        let mut places = Derived {
            storages: &storages,
            tx,
            ids: &mut self.ids,
            of: self.of.as_mut(),
            projection: &self.projection,
            condition: self.condition.as_ref(),
            runner: &runner,
        };
        let (at, row) = fetch(&mut places, self.at, to)?;
        let on = match at {
            Position::On(i) => places.place(i)?,
            Position::Before(_) | Position::After => None,
        };
        (self.at, self.on) = (at, on);
        Ok(row)
    }
}

/// The rows an updatable cursor may cover, in its order, read by `plans`,
/// one for each table it reads (`Updating`): those of each plan's table
/// access, sorted, if a Sort is in it, on the keys it has. Each key is a
/// column of the projections between the Sort and the access, of a column
/// of the cursor or of a value its ORDER BY computes [ISO/IEC 9075-2:2023
/// §7.17 SR26 d i 10], so each row is projected as they project it. Read
/// by transaction `tx`, their ids held, and sorted, as the statement's
/// work area holds them; and of more than one plan, the table of each,
/// the plan's place.
///
/// # Errors
///
/// Reading the rows, or holding or sorting their ids, failed.
pub(super) fn ordered_ids(plans: Vec<Plan>, st: &Arc<State>, tx: TxId) -> Result<(HeldRows<RowId>, Option<HeldRows<RowId>>)> {
    let tagged = plans.len() > 1;
    let mut ids = HeldRows::new(st.list_space());
    let mut of = tagged.then(|| HeldRows::new(st.list_space()));
    let mut sorted: Vec<(Vec<usize>, Vec<SortKey>, RowIter)> = Vec::new();
    for (k, plan) in plans.into_iter().enumerate() {
        let (keys, input) = match sort_of(plan) {
            Ok(found) => found,
            Err(access) => {
                for entry in table_entries(*access, st, tx)? {
                    ids.push(entry?.0)?;
                    if let Some(of) = of.as_mut() {
                        of.push(RowId::try_from(k).unwrap_or(RowId::MAX))?;
                    }
                }
                continue;
            }
        };
        let (layers, access) = layers_of(input);
        // Each row as its sort keys' values, then its table's place, then
        // its id.
        let columns: Vec<usize> = keys.iter().map(|key| key.column).collect();
        let on_keys: Vec<SortKey> = keys.iter().enumerate().map(|(i, key)| SortKey::new(i, key.order())).collect();
        let runner = Runner::new(st, tx);
        let at = columns.clone();
        let keyed: RowIter = Box::new(table_entries(access, st, tx)?.map(move |read| {
            read.and_then(|(id, row)| {
                let mut values = keyed(&projected(row, &layers, &runner)?, &at, id)?;
                if tagged {
                    values.insert(at.len(), Value::Exact(Decimal::new(i128::try_from(k).unwrap_or(i128::MAX), 0)));
                }
                Ok(values)
            })
        }));
        sorted.push((columns, on_keys, keyed));
    }
    let Some((columns, on_keys, _)) = sorted.first() else { return Ok((ids, of)) };
    let (width, on_keys) = (columns.len(), on_keys.clone());
    let all: RowIter = Box::new(sorted.into_iter().flat_map(|(_, _, rows)| rows));
    for row in sort_rows(&on_keys, all, st, tx) {
        let row = row?;
        if let Some(of) = of.as_mut() {
            of.push(id_of(&row, width)?)?;
        }
        ids.push(id_of(&row, width.saturating_add(usize::from(tagged)))?)?;
    }
    Ok((ids, of))
}

/// The keys of the Sort in `plan`, under the projections atop it that take
/// away what it alone reads, and its input; or, of a plan with no Sort,
/// its table access.
fn sort_of(plan: Plan) -> std::result::Result<(Vec<SortKey>, Plan), Box<Plan>> {
    let mut node = plan;
    loop {
        match node {
            Plan::Project { input, .. } => node = *input,
            Plan::Sort { keys, input, .. } => return Ok((keys, *input)),
            other @ (Plan::SeqScan { .. }
            | Plan::Limit { .. }
            | Plan::Nearest(_)
            | Plan::IndexScan { .. }
            | Plan::Filter { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Product { .. }
            | Plan::HashJoin { .. }
            | Plan::OuterJoin { .. }
            | Plan::IndexJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::SemiJoin(_)
            | Plan::MarkJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::Values { .. }
            | Plan::System { .. }
            | Plan::Recursive { .. }
            | Plan::Unnest(..)
            | Plan::Window { .. }
            | Plan::Polymorphic(..)
            | Plan::Recognize { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Parallel(..)
            | Plan::Transfer(_)
            | Plan::Counted(_)) => return Err(Box::new(other)),
        }
    }
}

/// The expressions of each projection atop `plan`'s table access, the
/// lowest first, and the access.
fn layers_of(plan: Plan) -> (Vec<Vec<Expr>>, Plan) {
    let mut layers = Vec::new();
    let mut node = plan;
    loop {
        match node {
            Plan::Project { exprs, input, .. } => {
                layers.push(exprs);
                node = *input;
            }
            other @ (Plan::SeqScan { .. }
            | Plan::Nearest(_)
            | Plan::IndexScan { .. }
            | Plan::Sort { .. }
            | Plan::Limit { .. }
            | Plan::Filter { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Product { .. }
            | Plan::HashJoin { .. }
            | Plan::OuterJoin { .. }
            | Plan::IndexJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::SemiJoin(_)
            | Plan::MarkJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::Values { .. }
            | Plan::System { .. }
            | Plan::Recursive { .. }
            | Plan::Unnest(..)
            | Plan::Window { .. }
            | Plan::Polymorphic(..)
            | Plan::Recognize { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Parallel(..)
            | Plan::Transfer(_)
            | Plan::Counted(_)) => {
                layers.reverse();
                return (layers, other);
            }
        }
    }
}

/// `row` of a table, projected by each of `layers` in turn.
fn projected(row: Row, layers: &[Vec<Expr>], runner: &Runner) -> Result<Row> {
    layers.iter().try_fold(row, |row, exprs| exprs.iter().map(|e| eval(e, &row, runner)).collect())
}

/// The values of `row`'s `columns`, then `id`.
fn keyed(row: &Row, columns: &[usize], id: RowId) -> Result<Row> {
    let mut keyed = project(row, columns).ok_or_else(|| api(format!("[DESIGN] row {id} is shorter than the cursor's order")))?;
    keyed.push(Value::Exact(Decimal::new(i128::from(id), 0)));
    Ok(keyed)
}

/// The id `keyed` put at `at` of `row`.
fn id_of(row: &Row, at: usize) -> Result<RowId> {
    let not_one = || api("[DESIGN] a sorted row of a cursor lost its id".to_owned());
    let Some(Value::Exact(id)) = row.get(at) else { return Err(not_one()) };
    RowId::try_from(id.units).map_err(|_| not_one())
}

/// An updatable cursor's places, each row derived as transaction `tx`
/// sees it now, from the storage of the table it is of: of `storages`, the
/// one `of` gives, or the first if none does.
struct Derived<'a> {
    storages: &'a [Arc<dyn TableStorage>],
    tx: TxId,
    ids: &'a mut HeldRows<RowId>,
    of: Option<&'a mut HeldRows<RowId>>,
    projection: &'a [usize],
    condition: Option<&'a Cond>,
    runner: &'a dyn Subqueries,
}

impl Derived<'_> {
    /// The table and id of the row at place `i`, if it has one.
    fn place(&mut self, i: usize) -> Result<Option<(usize, RowId)>> {
        let Some(id) = self.ids.get(i)? else { return Ok(None) };
        let k = match self.of.as_mut() {
            Some(of) => {
                of.get(i)?.and_then(|k| usize::try_from(k).ok()).ok_or_else(|| api(format!("[DESIGN] a cursor's row {i} has no table")))?
            }
            None => 0,
        };
        Ok(Some((k, id)))
    }

    /// The row at place `i` as it is now, if it is there still.
    fn now(&mut self, i: usize) -> Result<Option<(RowId, Row)>> {
        let Some((k, id)) = self.place(i)? else { return Ok(None) };
        let storage = self.storages.get(k).ok_or_else(|| api(format!("[DESIGN] a cursor reads no table {k}")))?;
        Ok(storage.get(self.tx, id)?.map(|row| (id, row)))
    }
}

impl Places for Derived<'_> {
    fn at(&mut self, i: usize) -> Result<Slot> {
        if i >= self.ids.len() {
            return Ok(Slot::End);
        }
        let Some((_, row)) = self.now(i)? else { return Ok(Slot::Gone) };
        match self.condition.map_or(Ok(Truth::True), |c| eval_cond(c, &row, self.runner))? {
            Truth::True => Ok(Slot::Row),
            Truth::False | Truth::Unknown => Ok(Slot::Gone),
        }
    }
    fn row(&mut self, i: usize) -> Result<Row> {
        if i >= self.ids.len() {
            return Err(api(format!("[DESIGN] a cursor has no row {i}")));
        }
        let (id, row) = self.now(i)?.ok_or_else(|| api(format!("[DESIGN] the row at {i} vanished")))?;
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
    use crate::spill::ListSpace;
    use crate::storage::StorageEngine;
    use crate::txn::NO_TRANSACTION;
    use crate::value::DataType;
    use crate::value::Spelled;

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
            collation: None,
            generated: None,
        };
        let desc = TableDesc {
            periods: Vec::new(),
            typed: None,
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
        let mut ids = HeldRows::new(ListSpace::default());
        ids.push(first).unwrap();
        ids.push(second).unwrap();
        let storages: [Arc<dyn TableStorage>; 1] = [Arc::from(table)];
        let mut places =
            Derived { storages: &storages, tx, ids: &mut ids, of: None, projection: &[1], condition: None, runner: &NoSubqueries };
        assert!(matches!(places.at(0).unwrap(), Slot::Gone));
        assert!(matches!(places.at(1).unwrap(), Slot::Row));
        assert!(matches!(places.at(2).unwrap(), Slot::End));
        assert_eq!(places.row(1).unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(), vec!["20".to_owned()]);
        assert_eq!(places.count().unwrap(), 2);
        places.projection = &[5];
        assert_eq!(places.row(1).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(places.row(0).err().map(|e| e.code), Some(Code::Api), "deleted");
        assert_eq!(places.row(2).err().map(|e| e.code), Some(Code::Api), "past the last");
    }

    /// Verifies: [DESIGN] a sorted row of an updatable cursor gives back
    /// the id put last in it; one shorter than its keys, or whose id is
    /// lost, is reported (SQLCODE -910), not a panic.
    #[test]
    fn a_sorted_row_keeps_its_id() {
        let row = vec![number(1), number(2)];
        let keyed_row = keyed(&row, &[1], 7).unwrap();
        assert_eq!(id_of(&keyed_row, 1).unwrap(), 7);
        assert_eq!(keyed(&row, &[2], 7).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(id_of(&keyed_row, 2).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(id_of(&vec![number(-1)], 0).err().map(|e| e.code), Some(Code::Api));
    }
}
