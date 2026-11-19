//! \[EXTENSION\] Queries that lock the rows they read, of a `<locking
//! clause>` (RULES.md D185), and LOCK TABLE (RULES.md D186).
//!
//! A locking query is planned as any other, its rows carrying their ids;
//! then a filter that locks each row (`Gate`) is put between its sort and
//! its row limit, so the limit counts only rows locked, and SKIP LOCKED
//! passes over the rest. Its rows are read in full as it opens, so every
//! lock is taken, or refused, then. At READ COMMITTED, a row changed since
//! the statement's snapshot runs the statement again at a new one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::stream::Stream;
use super::super::{Completion, Session, State, StatementContext};
use crate::ast::{CursorSpecification, LockTable, LockTableMode, LockWaits, LockingClause, Updatability};
use crate::binder::{self, Locking, OutColumn, qualify};
use crate::catalog::{Actions, QualifiedName};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Cond, Expr};
use crate::params::Params;
use crate::physical::Plan;
use crate::storage::row_id_of;
use crate::txn::{Concurrency, LockMode, Locked, RowLocking, Txn, Waits};
use crate::value::{DataType, Value};

/// \[DESIGN\] How many times a locking query runs again at READ COMMITTED,
/// each at a new snapshot, before its conflict is reported.
const MOST_RESTARTS: usize = 64;

/// What a lock does of one held, as the session's lock functions read it.
fn waits_of(waits: LockWaits) -> Waits {
    match waits {
        LockWaits::Wait => Waits::Setting,
        LockWaits::NoWait => Waits::NoWait,
        LockWaits::SkipLocked => Waits::Skip,
    }
}

impl Session {
    /// \[EXTENSION\] The `<locking clause>` of `spec`, the specification of
    /// an unnamed cursor, if it has one: FOR UPDATE alone is one there, no
    /// positioned statement naming an unnamed cursor (RULES.md D185).
    pub(in crate::db) fn locking_of(spec: &CursorSpecification) -> Option<LockingClause> {
        if let Some(locking) = &spec.locking {
            return Some(locking.clone());
        }
        let Some(Updatability::Update(of)) = &spec.updatability else { return None };
        Some(LockingClause { exclusive: true, of: of.clone(), waits: LockWaits::Wait, at: 0 })
    }

    /// \[EXTENSION\] The columns and rows of `spec`, whose locking clause is
    /// `lock`, each row locked as it is read (RULES.md D185).
    ///
    /// # Errors
    ///
    /// As `bind_locking`; SQLCODE -911 if a lock is refused or a row changed
    /// since the snapshot, after `MOST_RESTARTS` runs at READ COMMITTED.
    pub(in crate::db) fn open_locked(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        spec: &CursorSpecification,
        (lock, params): (&LockingClause, &Params),
    ) -> Result<(Vec<OutColumn>, Stream)> {
        let mut runs = 0_usize;
        loop {
            let (columns, physical, reads, locking) = self.locked_plan(st, tx, spec, (lock, params))?;
            let e = match Stream::kept(st, tx, physical, reads).and_then(Stream::read_now) {
                Ok(stream) => return Ok((columns, stream)),
                Err(e) => e,
            };
            runs = runs.saturating_add(1);
            let changed = locking.is_some_and(|l| l.changed.load(Ordering::SeqCst));
            if !changed || tx.one_snapshot() || self.cursors_open() || runs >= MOST_RESTARTS {
                return Err(e);
            }
            tx.begin_statement(|| true)?;
        }
    }

    /// The columns, plan and tables read of the locking query `spec`, and
    /// how it locks its rows, if by row.
    fn locked_plan(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        spec: &CursorSpecification,
        (lock, params): (&LockingClause, &Params),
    ) -> Result<LockedPlan> {
        let Locking { bound, table, id } = self.bound(st, params, |cx| binder::bind_locking(cx, spec, lock.of.as_deref()))?;
        let visible = bound.columns.len();
        let (optimized, _, physical) = super::super::plan(st, bound.plan);
        let reads: Arc<[QualifiedName]> = optimized.tables().into();
        let (physical, locking) = match st.capabilities(&table)?.concurrency {
            // A table whose engine leaves concurrency to the coordinator is
            // locked whole, its lock standing for its rows'.
            Concurrency::Coordinator => {
                let mode = if lock.exclusive { LockMode::Update } else { LockMode::Shared };
                let waits = if lock.waits == LockWaits::NoWait { Waits::NoWait } else { Waits::Setting };
                st.acquire_waiting(tx, &table, mode, waits)?;
                (physical, None)
            }
            Concurrency::Engine { .. } => {
                let lk = Arc::clone(st.lock_of(&table)?);
                tx.hold_rows(&lk);
                let storage = Arc::clone(st.storage_of(&table)?);
                let (who, waits, exclusive, snapshot) = (tx.waiter(), waits_of(lock.waits), lock.exclusive, tx.snapshot());
                let locking =
                    Arc::new(RowLocking { who, waits, exclusive, table, lock: lk, storage, snapshot, changed: AtomicBool::new(false) });
                (gated(physical, &locking, id), Some(locking))
            }
        };
        let exprs = (0..visible).map(Expr::Column).collect();
        let physical = Plan::Project { exprs, detail: "the query's columns".to_owned(), input: Box::new(physical) };
        Ok((bound.columns, physical, reads, locking))
    }

    /// \[EXTENSION\] LOCK TABLE (RULES.md D186): each table locked in the
    /// table lock mode its mode is, to the end of the transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a table is not a base table; SQLCODE -551 without
    /// the privilege; as `acquire_waiting`.
    pub(in crate::db) fn lock_table(&self, cx: StatementContext<'_>, lock: &LockTable) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        let mode = table_mode(lock.mode);
        let waits = if lock.nowait { Waits::NoWait } else { Waits::Setting };
        for (name, only, at) in &lock.tables {
            let qn = qualify(name, &self.default_schema());
            let desc = st.catalog.table(&qn).ok_or_else(|| {
                SqlError::at(Code::SyntaxRule, *at, format!("[EXTENSION] LOCK TABLE names {qn}, which is not a base table (RULES.md D186)"))
            })?;
            let needed = if mode == LockMode::Shared {
                Actions { select: true, ..Actions::default() }
            } else {
                Actions { delete: true, ..Actions::default() }
            };
            self.bound(st, &Params::new(), |b| binder::require(b.db, b.user.cell, desc, &needed, "EXTENSION; RULES.md D186")).or_at(*at)?;
            st.acquire_waiting(tx, &qn, mode, waits).or_at(*at)?;
            let subtables = if *only { Vec::new() } else { crate::catalog::proper_subtables(&*st.catalog, &qn) };
            for sub in subtables {
                st.acquire_waiting(tx, &sub, mode, waits).or_at(*at)?;
            }
        }
        Ok(Completion::Success)
    }
}

/// \[EXTENSION\] The table lock mode a `<lock table mode>` takes (RULES.md
/// D186).
fn table_mode(mode: LockTableMode) -> LockMode {
    match mode {
        LockTableMode::AccessShare | LockTableMode::RowShare | LockTableMode::Share => LockMode::Shared,
        LockTableMode::RowExclusive | LockTableMode::ShareUpdateExclusive => LockMode::Update,
        LockTableMode::ShareRowExclusive | LockTableMode::Exclusive | LockTableMode::AccessExclusive => LockMode::Exclusive,
    }
}

/// `physical`, a locking query's plan, with the filter that locks each row,
/// whose id is its value `id`, put between its sort and its row limit. The
/// sort is then made of all its rows, not of the first the limit keeps.
fn gated(physical: Plan, locking: &Arc<RowLocking>, id: usize) -> Plan {
    let cond = Cond::Value(Expr::Call(ScalarRef(Arc::new(Gate(Arc::clone(locking)))), vec![Expr::Column(id)]));
    let filter = |input: Plan| Plan::Filter { cond, detail: "each row locked".to_owned(), est: None, input: Box::new(input) };
    let Plan::Limit { limit, input } = physical else { return filter(physical) };
    let mut input = *input;
    if let Plan::Sort { most, .. } = &mut input {
        *most = None;
    }
    Plan::Limit { limit, input: Box::new(filter(input)) }
}

/// A locking query's columns, its plan, the tables it reads, and its row
/// locking, if its table's engine keeps row locks.
type LockedPlan = (Vec<OutColumn>, Plan, Arc<[QualifiedName]>, Option<Arc<RowLocking>>);

/// \[EXTENSION\] The filter of a locking query: true of a row it locked,
/// false of one SKIP LOCKED passes over (RULES.md D185).
struct Gate(Arc<RowLocking>);

impl ScalarFunction for Gate {
    fn name(&self) -> &'static str {
        "ROW LOCK"
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let id = args.first().map(row_id_of).transpose()?.flatten();
        let Some(id) = id else { return Ok(Value::Bool(false)) };
        Ok(Value::Bool(self.0.lock(id)? == Locked::Got))
    }

    fn deterministic(&self) -> bool {
        false
    }

    /// Its refusals are of locks, not of the row's values. So a row limit
    /// reads no row past those it keeps, and none past them is locked
    /// (RULES.md D15, D185).
    fn raise_free(&self) -> bool {
        true
    }
}

impl Session {
    /// \[EXTENSION\] Unlocks the advisory locks it holds, as it ends
    /// (RULES.md D187).
    pub(in crate::db) fn unlock_advisory(&self) {
        if let Ok(st) = self.db.snapshot() {
            st.tx.advisory().release_session(self.stop.session);
        }
    }
}
