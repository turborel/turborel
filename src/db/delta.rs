//! [ISO/IEC 9075-2:2023 §7.6 SR11, GR5] The statement of a `<data change
//! delta table>`, run as the cursor whose query reads it opens, and the
//! rows it gives (RULES.md D174): the old or new delta table of its
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

use super::prepared::Keeping;
use super::{Session, State, StatementContext};
use crate::ast::{
    CheckLevels, CursorSpecification, DeltaResult, DeltaTable, InsertSource, MatchedAction, MergeWhen, SqlStatement, TableReference,
};
use crate::binder::{qualify, write_target};
use crate::catalog::{ColumnDesc, QualifiedName, proper_subtables};
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
    /// The base table whose rows change, the subject table or the base
    /// table of the subject view.
    table: QualifiedName,
    /// The proper subtables of `table`, a typed table, whose rows the
    /// statement changes by their superrows in it.
    members: Vec<QualifiedName>,
    /// The column of `table` each column of the subject table is.
    map: Vec<usize>,
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
    /// statement's own changes are made [§7.6 GR5a], and the tables whose
    /// rows are its rows, its subtables or partitions.
    pub(super) fn restricted(&self) -> Option<Vec<QualifiedName>> {
        (self.0.result == DeltaResult::Final).then(|| std::iter::once(self.0.table.clone()).chain(self.0.members.iter().cloned()).collect())
    }

    /// Records the row of `table` changed from `old` to `new`, if the
    /// delta table holds it, if `table` is the base table whose rows
    /// change, or one of its proper subtables, the row then its superrow
    /// [ISO/IEC 9075-2:2023 §14.9 GR4, §14.14 GR6]. Of a view, the row is
    /// as the view shows it [§15.11, §15.13].
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn record(&self, table: &QualifiedName, old: Option<&Row>, new: Option<&Row>) -> Result<()> {
        if *table != self.0.table && !self.0.members.contains(table) {
            return Ok(());
        }
        let row = match self.0.result {
            DeltaResult::Old => old,
            DeltaResult::New | DeltaResult::Final => new,
        };
        let Some(row) = row else { return Ok(()) };
        let shown: Row = self.0.map.iter().map(|&c| row.get(c).cloned().unwrap_or(crate::value::Value::Null)).collect();
        lock_builtin(&self.0.rows).push(shown)
    }
}

/// The tables whose rows a statement changing `base` changes as its own,
/// a typed table's proper subtables [§14.9 GR4, §14.14 GR6], and
/// \[EXTENSION\] a partitioned table's partitions (RULES.md D153).
fn delta_members(st: &State, base: &QualifiedName) -> Vec<QualifiedName> {
    if st.catalog.table(base).is_some_and(|t| t.typed.is_some()) {
        return proper_subtables(&*st.catalog, base);
    }
    st.catalog.partitioning().descendants(base)
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
    /// SQLCODE -103 as `check_shape` or `delta_subject` refuses it; any of
    /// the statement's.
    pub(super) fn run_delta(&self, st: &Arc<State>, tx: &Txn, spec: &CursorSpecification, params: &Params) -> Result<bool> {
        let deltas = spec.query.deltas();
        for d in &deltas {
            self.run_delta_table(st, tx, d, (params, None))?;
        }
        Ok(!deltas.is_empty())
    }

    /// Runs the statements of the delta tables `stmt`, an INSERT or a
    /// MERGE, reads, in the statement of `tx`, before it runs, true if it
    /// reads one (RULES.md D175).
    ///
    /// # Errors
    ///
    /// As `run_delta`.
    pub(super) fn run_change_deltas(&self, st: &Arc<State>, tx: &Txn, stmt: &SqlStatement, params: &Params) -> Result<bool> {
        let mut deltas: Vec<&DeltaTable> = Vec::new();
        if let SqlStatement::Insert(i) = stmt
            && let InsertSource::Query(q) = &i.source
        {
            deltas = q.deltas();
        }
        if let SqlStatement::Merge(m) = stmt
            && let TableReference::Delta(d) = &m.source
        {
            deltas.push(d);
        }
        for d in &deltas {
            self.run_delta_table(st, tx, d, (params, None))?;
        }
        Ok(!deltas.is_empty())
    }

    /// Runs the statement of the delta table `d` in the statement of `tx`,
    /// giving `d` its rows: of a statement of a program kept, by the plans
    /// `keeping` keeps of it (RULES.md D792).
    ///
    /// # Errors
    ///
    /// As `run_delta`.
    pub(super) fn run_delta_table(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        d: &DeltaTable,
        (params, keeping): (&Params, Option<&Keeping<'_>>),
    ) -> Result<()> {
        check_shape(d)?;
        let subject = d.subject().ok_or_else(|| refused("Format", "a <data change delta table> has a subject table"))?;
        let name = qualify(subject, &self.default_schema());
        let (base, map, columns) = self.delta_subject(st, &name, params)?;
        self.may_run(st, &d.statement)?;
        let members = delta_members(st, &base);
        let rows = Mutex::new(HeldRows::new(st.list_space()));
        let gathered = Gathered { table: base, members, map, result: d.result, rows, claimed: AtomicBool::new(false) };
        let collector = Collector(Arc::new(gathered));
        // A delta table of the statement of another's keeps the other's
        // collector, given back once it ran.
        let outer = self.inner.borrow_mut().delta.replace(collector.clone());
        let ran = self.run_statement(StatementContext { st, tx }, &d.statement, &mut params.clone(), keeping);
        self.inner.borrow_mut().delta = outer;
        ran?;
        let held = std::mem::replace(&mut *lock_builtin(&collector.0.rows), HeldRows::new(st.list_space()));
        d.rows.give((TransitionRows::with(held), name, columns));
        Ok(())
    }

    /// The base table whose rows a delta table's statement of the subject
    /// table `name` changes, the column of it each of the subject's is,
    /// and the subject's columns. Of a base table it is itself, of a view the
    /// base table under it [ISO/IEC 9075-2:2023 §7.6 SR11b].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of a view without CASCADED CHECK OPTION [SR11b i]; as
    /// `write_target` of a view not updatable, or of no table.
    fn delta_subject(
        &self,
        st: &Arc<State>,
        name: &QualifiedName,
        params: &Params,
    ) -> Result<(QualifiedName, Vec<usize>, Vec<ColumnDesc>)> {
        if let Some(t) = st.catalog.table(name) {
            return Ok((name.clone(), (0..t.columns.len()).collect(), t.columns.clone()));
        }
        // A view with CHECK OPTION has no INSTEAD OF trigger, nor does any
        // view under it [§11.49 SR4c, SR4d], so none is trigger updatable
        // [SR11b ii].
        if st.catalog.view(name).is_some_and(|v| v.check_option != Some(CheckLevels::Cascaded)) {
            return Err(refused("SR11b i", &format!("{name}, the subject view of a <data change delta table>, has CASCADED CHECK OPTION")));
        }
        let rule = "ISO/IEC 9075-2:2023 §7.6 SR11";
        let target = self.bound(st, params, |b| write_target(b.db, name, (rule, b.user), None))?;
        Ok((target.base.name.clone(), target.map, target.named.columns))
    }

    /// The collector of the statement whose effects are made now, if one
    /// is a delta table's and none took it yet; else, of FINAL, the table
    /// none may change.
    pub(super) fn delta_effects(&self) -> (Option<Collector>, Option<Vec<QualifiedName>>) {
        let inner = self.inner.borrow();
        let Some(c) = inner.delta.as_ref() else { return (None, None) };
        if c.claim() {
            return (Some(c.clone()), None);
        }
        (None, c.restricted())
    }
}
