//! [ISO/IEC 9075-2:2023 §7.6 SR11, GR5] The statement of a `<data change
//! delta table>`, run as the cursor whose query reads it opens, and the
//! rows it gives (RULES.md D183): the old or new delta table of its
//! operation on the subject table [§14.9, §14.12, §14.14, §15.11, §15.14].
//!
//! The statement runs in the cursor's statement, after which the query is
//! bound over the rows it recorded. Each is recorded as the statement
//! changes its row, before the statement's referential actions and AFTER
//! triggers. So NEW and FINAL give it as the statement left it, its
//! defaults, generated and identity columns given, and what its BEFORE
//! triggers set.
//! Of FINAL, no referential action of the statement may then change the
//! subject table (SQLSTATE 27001), nor may an AFTER trigger (0W001)
//! [§7.6 GR5a, §15.21 GR6i, §15.18 GR15]. A statement that fails gives no
//! rows. The cursor's statement fails with it, and both are undone.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{Session, State, StatementContext};
use crate::ast::{CursorSpecification, DeltaResult, DeltaTable, MatchedAction, MergeWhen, SqlStatement};
use crate::binder::qualify;
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::{HeldRows, TransitionRows};
use crate::params::Params;
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::txn::Txn;

/// \[DESIGN\] Where a statement records the rows of its delta table:
/// shared by the session and the statement's effects.
#[derive(Clone)]
pub(super) struct Collector(Arc<Gathered>);

/// What a `Collector` gathers.
struct Gathered {
    /// The subject table.
    table: QualifiedName,
    result: DeltaResult,
    rows: Mutex<HeldRows<Row>>,
    /// Whether the statement's effects took it. Those of any statement
    /// after, a trigger's, record nothing.
    claimed: AtomicBool,
}

impl Collector {
    /// Claims it for the effects of a statement: false if one did.
    pub(super) fn claim(&self) -> bool {
        !self.0.claimed.swap(true, Ordering::SeqCst)
    }

    /// Of FINAL, the subject table, which none may change once the
    /// statement's own changes are made [§7.6 GR5a].
    pub(super) fn restricted(&self) -> Option<&QualifiedName> {
        (self.0.result == DeltaResult::Final).then_some(&self.0.table)
    }

    /// Records the row of `table` changed from `old` to `new`, if `table`
    /// is the subject table and the delta table holds it.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn record(&self, table: &QualifiedName, old: Option<&Row>, new: Option<&Row>) -> Result<()> {
        if *table != self.0.table {
            return Ok(());
        }
        let row = match self.0.result {
            DeltaResult::Old => old,
            DeltaResult::New | DeltaResult::Final => new,
            DeltaResult::Changed => new.or(old),
        };
        row.map_or(Ok(()), |r| lock_builtin(&self.0.rows).push(r.clone()))
    }
}

/// [ISO/IEC 9075-2:2023 §15.18 GR15, §15.21 GR6i] A change of `table`, of
/// the restricted subject table name list, refused with SQLSTATE `state`.
pub(super) fn restricted(table: &QualifiedName, state: SqlState) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §7.6 GR5a] {table} is the subject table of a FINAL TABLE being read, so is not changed");
    SqlError::new(Code::Conflict, msg).with_state(state)
}

/// A refusal of `rule`: `msg`.
fn refused(rule: &str, msg: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §7.6 {rule}] {msg}"))
}

/// Whether `clauses` of a MERGE update, delete and insert.
fn merges(clauses: &[MergeWhen]) -> (bool, bool, bool) {
    let has = |f: &dyn Fn(&MergeWhen) -> bool| clauses.iter().any(f);
    let updates = has(&|c| matches!(c, MergeWhen::Matched { action: MatchedAction::Update(_), .. }));
    let deletes = has(&|c| matches!(c, MergeWhen::Matched { action: MatchedAction::Delete, .. }));
    (updates, deletes, has(&|c| matches!(c, MergeWhen::NotMatched { .. })))
}

/// Whether `statement`, a `<data change statement>`, makes old rows and
/// new, and is FOR PORTION OF; none if it is no such statement.
fn shape_of(statement: &SqlStatement) -> Option<(bool, bool, bool)> {
    if let SqlStatement::Insert(_) = statement {
        return Some((false, true, false));
    }
    if let SqlStatement::UpdateSearched { portion, .. } = statement {
        return Some((true, true, portion.is_some()));
    }
    if let SqlStatement::DeleteSearched { portion, .. } = statement {
        return Some((true, false, portion.is_some()));
    }
    let SqlStatement::Merge(m) = statement else { return None };
    let (updates, deletes, inserts) = merges(&m.clauses);
    Some((updates || deletes, updates || inserts, false))
}

/// [ISO/IEC 9075-2:2023 §7.6 SR11a] Refuses `d` if its result option
/// names rows its statement makes none of, or its statement is FOR
/// PORTION OF.
fn check_shape(d: &DeltaTable) -> Result<()> {
    let (old, new) = (d.result == DeltaResult::Old, matches!(d.result, DeltaResult::New | DeltaResult::Final));
    let (olds, news, portion) = shape_of(&d.statement)
        .ok_or_else(|| refused("Format", "a <data change delta table>'s statement is a searched INSERT, UPDATE, DELETE or MERGE"))?;
    if portion {
        return Err(refused("SR11", "a <data change delta table>'s statement does not contain FOR PORTION OF"));
    }
    if (old && !olds) || (new && !news) {
        return Err(refused("SR11a", "the <result option> names rows its <data change statement> makes none of"));
    }
    Ok(())
}

impl Session {
    /// Runs the statement of the delta table `spec`'s query reads, if it
    /// reads one, in the statement of `tx`, giving the delta table its
    /// rows: true if it ran one.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 as `check_shape` refuses it; -900 of a view's; any of
    /// the statement's.
    pub(super) fn run_delta(&self, st: &Arc<State>, tx: &Txn, spec: &CursorSpecification, params: &Params) -> Result<bool> {
        let Some(d) = spec.query.delta() else { return Ok(false) };
        self.run_delta_table(st, tx, d, params).map(|()| true)
    }

    /// Runs the statement of the delta table `d` in the statement of `tx`,
    /// giving `d` its rows.
    ///
    /// # Errors
    ///
    /// As `run_delta`.
    pub(super) fn run_delta_table(&self, st: &Arc<State>, tx: &Txn, d: &DeltaTable, params: &Params) -> Result<()> {
        check_shape(d)?;
        let subject = d.subject().ok_or_else(|| refused("Format", "a <data change delta table> has a subject table"))?;
        let name = qualify(subject, &self.default_schema());
        let columns = st.catalog.table(&name).map(|t| t.columns.clone()).ok_or_else(|| {
            SqlError::new(
                Code::Unsupported,
                format!("[EXTENSION] a <data change delta table> of {name}, not a base table, is not supported (RULES.md D183)"),
            )
        })?;
        self.may_run(st, &d.statement)?;
        let rows = Mutex::new(HeldRows::new(st.list_space()));
        let collector = Collector(Arc::new(Gathered { table: name.clone(), result: d.result, rows, claimed: AtomicBool::new(false) }));
        self.inner.borrow_mut().delta = Some(collector.clone());
        let ran = self.run_statement(StatementContext { st, tx }, &d.statement, &mut params.clone(), None);
        self.inner.borrow_mut().delta = None;
        ran?;
        let held = std::mem::replace(&mut *lock_builtin(&collector.0.rows), HeldRows::new(st.list_space()));
        d.rows.give((TransitionRows::with(held), name, columns));
        Ok(())
    }

    /// The collector of the statement whose effects are made now, if one
    /// is a delta table's and none took it yet; else, of FINAL, the table
    /// none may change.
    pub(super) fn delta_effects(&self) -> (Option<Collector>, Option<QualifiedName>) {
        let inner = self.inner.borrow();
        let Some(c) = inner.delta.as_ref() else { return (None, None) };
        if c.claim() {
            return (Some(c.clone()), None);
        }
        (None, c.restricted().cloned())
    }
}
