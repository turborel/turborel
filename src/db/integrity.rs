//! \[EXTENSION\] Constraints checked at the end of each statement [ISO/IEC
//! 9075:1992 §10.6 GR3]: what a statement did to rows (`Effects`), its
//! referential actions run first [§11.8 GR5, GR6] (`integrity/references.rs`),
//! which may do more, then each constraint those rows may have broken
//! (`Check`): NOT NULL, CHECK [§11.6 GR3], each FOREIGN KEY of the rows
//! [§11.8 GR2], UNIQUE and PRIMARY KEY [§11.7 GR2], those to the rows
//! deleted or re-keyed [NO ACTION], each CHECK with a subquery that
//! reads a table written, over all of its own [§11.6 GR3], and each
//! assertion that reads one [§11.34 GR2] (`db/assertions.rs`).
//!
//! A constraint whose mode in the transaction is immediate is checked then:
//! a violation fails the statement, which is undone [X3.135-1986 §7.3
//! GR3b], SQLCODE -201, SQLSTATE 23000 [§10.6 GR4]. A deferred one's checks
//! wait in the session (`Deferral`), to be made when SET CONSTRAINTS makes
//! it immediate [§14.2], or at COMMIT, which a violation rolls back
//! [§14.3 GR4, GR5a] (`db/deferral.rs`).
//!
//! Every table read is read under the transaction's locks, or checked at
//! commit by its engine, at every isolation level, so the checks are
//! serializable with what others write [§4.28] (RULES.md D29).
//!
//! What a statement did, each row's id and an UPDATE's or DELETE's old
//! values, the checks gathered of it, and the checks of deferred
//! constraints waiting, are held as lists that spill past WORK_MEMORY
//! (`integrity/held.rs`, RULES.md D38): read back in order, the rows of
//! each UNIQUE constraint checked as they come (`integrity/keys.rs`).

mod held;
mod keys;
mod references;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use self::keys::KeyCheck;
use super::{State, api, plan};
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName, ReferentialAction, TableDesc};
use crate::error::{Code, Result, SqlError, Subject, fail};
use crate::exec::{Heard, HeldRows, eval_cond, execute};
use crate::logical::{Cond, LogicalPlan, Truth};
use crate::spill::ListSpace;
use crate::storage::{Row, RowId, project};
use crate::txn::Txn;
use crate::value::Value;

/// One thing a statement did to a row.
#[derive(Clone)]
enum Effect {
    Inserted {
        table: QualifiedName,
        id: RowId,
    },
    /// Updated, from `old`, the values of `columns` given.
    Updated {
        table: QualifiedName,
        id: RowId,
        old: Row,
        columns: Vec<usize>,
    },
    Deleted {
        table: QualifiedName,
        id: RowId,
        old: Row,
    },
}

/// What a statement did to rows, in order, for the constraints checked at
/// its end, held as a list that spills (`integrity/held.rs`), and where its
/// lists spill; the authorization identifier it runs under, CURRENT_USER
/// to a default its referential actions give [§11.5 GR1c]; and its
/// session's constraint modes, where the checks of deferred constraints
/// wait; the rows its session changed through cursors still open, and
/// where to note a referential action on one [§11.8 GR8].
pub(super) struct Effects<'a> {
    done: HeldRows<Effect>,
    space: ListSpace,
    user: String,
    /// The statement's instant and its session's time zone, of the
    /// defaults its referential actions give.
    pub(super) now: crate::value::datetime::Now,
    deferral: &'a RefCell<Deferral>,
    pub(super) cursors: Cursored<'a>,
}

/// [ISO/IEC 9075:1992 §11.8 GR8] The rows a session changed through each
/// cursor still open, by positioned UPDATE or DELETE: each cursor's name,
/// table and row; and the cursor whose row a referential action changed,
/// once one did.
pub(super) struct Cursored<'a> {
    pub(super) rows: Vec<(String, QualifiedName, RowId)>,
    pub(super) conflict: &'a RefCell<Option<String>>,
}

impl Cursored<'_> {
    /// A referential action changes row `id` of `table`: if a cursor
    /// changed it, a cursor operation conflict, noted.
    pub(super) fn acted_on(&self, table: &QualifiedName, id: RowId) {
        if let Some((cursor, ..)) = self.rows.iter().find(|(_, t, r)| t == table && *r == id) {
            self.conflict.borrow_mut().get_or_insert_with(|| cursor.clone());
        }
    }
}

impl<'a> Effects<'a> {
    /// Nothing yet done by a statement run under `user`, in a session whose
    /// constraint modes are `deferral`'s, and whose cursors `cursors` say,
    /// what it did held in `space`.
    pub(super) fn new(user: &str, deferral: &'a RefCell<Deferral>, cursors: Cursored<'a>, space: ListSpace) -> Effects<'a> {
        let now = crate::value::datetime::Now::default();
        Effects { done: HeldRows::new(space.clone()), space, user: user.to_string(), now, deferral, cursors }
    }

    /// The authorization identifier the statement runs under.
    pub(super) fn user(&self) -> &str {
        &self.user
    }

    /// Row `id` of `table` inserted.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved.
    pub(super) fn inserted(&mut self, table: &QualifiedName, id: RowId) -> Result<()> {
        self.done.push(Effect::Inserted { table: table.clone(), id })
    }

    /// Row `id` of `table` updated from `old`, giving values to `columns`.
    ///
    /// # Errors
    ///
    /// As `inserted`.
    pub(super) fn updated(&mut self, table: &QualifiedName, id: RowId, old: Row, columns: &[usize]) -> Result<()> {
        self.done.push(Effect::Updated { table: table.clone(), id, old, columns: columns.to_vec() })
    }

    /// Row `id` of `table`, `old`, deleted.
    ///
    /// # Errors
    ///
    /// As `inserted`.
    pub(super) fn deleted(&mut self, table: &QualifiedName, id: RowId, old: Row) -> Result<()> {
        self.done.push(Effect::Deleted { table: table.clone(), id, old })
    }
}

/// One constraint to check of what a statement did: at its end if the
/// constraint is immediate, else when it becomes so.
#[derive(Clone)]
pub(super) enum Check {
    /// NOT NULL, a CHECK without a subquery, or a FOREIGN KEY, of row `id`
    /// of `table`.
    Row { table: QualifiedName, constraint: QualifiedName, id: RowId },
    /// UNIQUE or PRIMARY KEY, of row `id` of `table`: the rows of one
    /// constraint, one after another, checked together (`KeyCheck`).
    Unique { table: QualifiedName, constraint: QualifiedName, id: RowId },
    /// A CHECK with a subquery, over all of `table`.
    Table { table: QualifiedName, constraint: QualifiedName },
    /// FOREIGN KEY `constraint` of `child`, once `old`, a row it references,
    /// was deleted, or row `id` updated from it [NO ACTION].
    Referenced { child: QualifiedName, constraint: QualifiedName, old: Row, id: Option<RowId> },
    /// An assertion, of a table it reads written [ISO/IEC 9075:1992 §11.34].
    Assertion { constraint: QualifiedName },
}

impl Check {
    /// The name of the constraint it checks.
    pub(super) fn constraint(&self) -> &QualifiedName {
        match self {
            Check::Row { constraint, .. }
            | Check::Unique { constraint, .. }
            | Check::Table { constraint, .. }
            | Check::Referenced { constraint, .. }
            | Check::Assertion { constraint } => constraint,
        }
    }

    /// The table of the constraint it checks: none of an assertion's.
    pub(super) fn table(&self) -> Option<&QualifiedName> {
        match self {
            Check::Row { table, .. } | Check::Unique { table, .. } | Check::Table { table, .. } => Some(table),
            Check::Referenced { child, .. } => Some(child),
            Check::Assertion { .. } => None,
        }
    }
}

/// [ISO/IEC 9075:1992 §4.28, §14.2] The constraint mode of each constraint
/// in a session's transaction, as SET CONSTRAINTS left it, and the checks
/// of deferred constraints not yet made, held as a list that spills, until
/// they are made or the transaction ends (RULES.md D38).
pub(super) struct Deferral {
    /// The mode SET CONSTRAINTS ALL gave every deferrable constraint, last:
    /// deferred or not.
    pub(super) all: Option<bool>,
    /// The mode SET CONSTRAINTS gave each constraint it named since.
    pub(super) named: HashMap<QualifiedName, bool>,
    /// The checks waiting.
    pub(super) pending: HeldRows<Check>,
}

impl Default for Deferral {
    /// None waiting, and no mode set: as a transaction begins.
    fn default() -> Deferral {
        Deferral { all: None, named: HashMap::new(), pending: HeldRows::new(ListSpace::default()) }
    }
}

impl Deferral {
    /// [§10.6 GR1, GR2; §14.2 GR2, GR3] Whether constraint `c` is deferred:
    /// deferrable, and so set, or initially deferred.
    pub(super) fn deferred(&self, c: &ConstraintDesc) -> bool {
        c.attributes.deferrable && self.named.get(mode_name(c)).copied().or(self.all).unwrap_or(c.attributes.initially_deferred)
    }

    /// `more` checks to wait, after those waiting.
    pub(super) fn defer(&mut self, more: HeldRows<Check>) {
        if self.pending.is_empty() {
            self.pending = more;
        } else {
            self.pending.append(more);
        }
    }
}

/// What the checks of a statement, or those made at once, keep as they
/// are made: each CHECK condition bound once, and the check of the rows of
/// one UNIQUE constraint, given one after another.
#[derive(Default)]
pub(super) struct Checking<'s> {
    bound: HashMap<QualifiedName, Cond>,
    key: Option<KeyCheck<'s>>,
}

impl Checking<'_> {
    /// The end of the rows of the UNIQUE constraint being checked, if any.
    ///
    /// # Errors
    ///
    /// Its failure held back (`KeyCheck::finish`).
    pub(super) fn finish(&mut self) -> Result<()> {
        self.key.take().map_or(Ok(()), KeyCheck::finish)
    }
}

/// The name SET CONSTRAINTS gives constraint `c` by: of a domain's
/// constraint on a column, the domain constraint's [§11.21 GR5].
pub(super) fn mode_name(c: &ConstraintDesc) -> &QualifiedName {
    c.domain.as_ref().unwrap_or(&c.name)
}

impl State {
    /// `table`, and every table a referential action of a change to it may
    /// write, through others too: those whose cursors a statement writing
    /// `table` settles first (`Session::settle_cursors`).
    pub(super) fn written_by(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        let mut tables: Vec<QualifiedName> = Vec::new();
        let mut next = vec![table.clone()];
        while let Some(t) = next.pop() {
            if tables.contains(&t) {
                continue;
            }
            for child in self.catalog.tables() {
                let acts = child.foreign_keys().any(|(_, fk)| {
                    fk.table == t && (fk.on_delete != ReferentialAction::NoAction || fk.on_update != ReferentialAction::NoAction)
                });
                if acts {
                    next.push(child.name.clone());
                }
            }
            tables.push(t);
        }
        tables
    }

    /// [ISO/IEC 9075:1992 §10.6 GR3] The end of a statement of transaction
    /// `tx` that did `effects`: its referential actions, then the checks
    /// of its immediate constraints made, and those of its deferred ones
    /// left to wait.
    ///
    /// # Errors
    ///
    /// SQLCODE -201 for a constraint violated, SQLSTATE 23000 [§10.6 GR4],
    /// or 27000 for a triggered data change violation [§11.8 GR7]; the
    /// reads' and writes'.
    pub(super) fn end_statement(self: &Arc<Self>, tx: &Txn, effects: Effects<'_>) -> Result<()> {
        // [ISO/IEC 9075:1992 §4.28] serializable at every level.
        tx.check_reads(true)?;
        let checked = self.checked(tx, effects);
        checked.and(tx.check_reads(false))
    }

    /// `end_statement`'s work, its reads checked.
    fn checked(self: &Arc<Self>, tx: &Txn, mut effects: Effects<'_>) -> Result<()> {
        self.referential_actions(tx, &mut effects)?;
        let checks = self.checks(&mut effects)?;
        let mut deferred = HeldRows::new(effects.space.clone());
        {
            let deferral = effects.deferral.borrow();
            let mut checking = Checking::default();
            for check in checks {
                let check = check?;
                let Some(c) = self.constraint_of(&check) else { continue };
                if deferral.deferred(c) {
                    deferred.push(check)?;
                } else {
                    self.check(tx, &check, &mut checking)?;
                }
            }
            checking.finish()?;
        }
        effects.deferral.borrow_mut().defer(deferred);
        Ok(())
    }

    /// The constraint `check` checks, if its table still has it.
    pub(super) fn constraint_of(&self, check: &Check) -> Option<&ConstraintDesc> {
        let name = check.constraint();
        let Some(table) = check.table() else { return self.catalog.assertion(name) };
        self.catalog.table(table)?.constraints.iter().find(|c| c.name == *name)
    }

    /// The checks of what `effects` did, held as it holds them: of each row
    /// inserted, every constraint of its table; of each updated, those on a
    /// column given a value, and its CHECKs; of each deleted or re-keyed,
    /// the foreign keys that reference it; each CHECK with a subquery of a
    /// table written, or reading one; and each assertion reading one. The
    /// rows of each UNIQUE constraint come one after another, read in a
    /// pass of their own over what it did.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn checks(&self, effects: &mut Effects<'_>) -> Result<HeldRows<Check>> {
        let mut out = HeldRows::new(effects.space.clone());
        let mut changed: Vec<QualifiedName> = Vec::new();
        effects.done.visit(&mut |effect| {
            let (table, id, columns) = match effect {
                Effect::Inserted { table, id } => (table, Some(*id), None),
                Effect::Updated { table, id, columns, old } => {
                    self.referenced_checks(table, Some(columns), (old, Some(*id)), &mut out)?;
                    (table, Some(*id), Some(columns.as_slice()))
                }
                Effect::Deleted { table, old, .. } => {
                    self.referenced_checks(table, None, (old, None), &mut out)?;
                    (table, None, None)
                }
            };
            if !changed.contains(table) {
                changed.push(table.clone());
            }
            match (id, self.catalog.table(table)) {
                (Some(id), Some(desc)) => row_checks(desc, id, columns, &mut out),
                (None, _) | (_, None) => Ok(()),
            }
        })?;
        self.unique_checks(effects, &changed, &mut out)?;
        for desc in self.catalog.tables() {
            for c in &desc.constraints {
                let ConstraintKind::Check { reads, .. } = &c.kind else { continue };
                if !reads.is_empty() && (changed.contains(&desc.name) || reads.iter().any(|t| changed.contains(t))) {
                    out.push(Check::Table { table: desc.name.clone(), constraint: c.name.clone() })?;
                }
            }
        }
        for a in self.catalog.assertions() {
            let ConstraintKind::Check { reads, .. } = &a.kind else { continue };
            if reads.iter().any(|t| changed.contains(t)) {
                out.push(Check::Assertion { constraint: a.name.clone() })?;
            }
        }
        Ok(out)
    }

    /// Adds to `out` the checks of each UNIQUE constraint of the tables
    /// `changed`, of each row `effects` inserted, or updated giving a value
    /// to one of its columns: one pass over what it did for each, so the
    /// rows of a constraint come one after another.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn unique_checks(&self, effects: &mut Effects<'_>, changed: &[QualifiedName], out: &mut HeldRows<Check>) -> Result<()> {
        for desc in changed.iter().filter_map(|t| self.catalog.table(t)) {
            for c in &desc.constraints {
                let ConstraintKind::Unique { columns: of, .. } = &c.kind else { continue };
                effects.done.visit(&mut |effect| match keyed(effect, desc, of) {
                    Some(id) => out.push(Check::Unique { table: desc.name.clone(), constraint: c.name.clone(), id }),
                    None => Ok(()),
                })?;
            }
        }
        Ok(())
    }

    /// Adds to `out` the NO ACTION checks of the foreign keys that
    /// reference `table`, a row of which, `old`, was deleted, or (with its
    /// `id`) updated, giving values to `columns`: of those whose referenced
    /// columns it gave values to.
    ///
    /// # Errors
    ///
    /// The spill area's.
    fn referenced_checks(
        &self,
        table: &QualifiedName,
        columns: Option<&[usize]>,
        (old, id): (&Row, Option<RowId>),
        out: &mut HeldRows<Check>,
    ) -> Result<()> {
        for child in self.catalog.tables() {
            for (name, fk) in child.foreign_keys().filter(|(_, fk)| fk.table == *table) {
                if columns.is_none_or(|set| fk.referenced.iter().any(|r| set.contains(r))) {
                    out.push(Check::Referenced { child: child.name.clone(), constraint: name.clone(), old: old.clone(), id })?;
                }
            }
        }
        Ok(())
    }

    /// Makes `check`, in transaction `tx`, with what `checking` keeps:
    /// each CHECK condition bound once, and the rows of a UNIQUE constraint
    /// checked together, those of the one before ended first.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, if the constraint is not satisfied:
    /// about the constraint and its table [ISO/IEC 9075:1992 §18.1 GR3f].
    pub(super) fn check<'s>(self: &'s Arc<Self>, tx: &'s Txn, check: &Check, checking: &mut Checking<'s>) -> Result<()> {
        if !matches!(check, Check::Unique { .. }) {
            checking.finish()?;
        }
        let Some(table) = check.table() else { return self.check_assertion(tx, check.constraint()) };
        let (Some(c), Some(desc)) = (self.constraint_of(check), self.catalog.table(table)) else { return Ok(()) };
        let broken = |e: SqlError| {
            if e.code == Code::Constraint && e.subject.is_none() {
                e.about(Subject::constraint(c.name.parts(), desc.name.parts()))
            } else {
                e
            }
        };
        self.check_one(tx, check, (desc, c), checking).map_err(broken)
    }

    /// `check`'s, of constraint `c` of `desc`.
    fn check_one<'s>(
        self: &'s Arc<Self>,
        tx: &'s Txn,
        check: &Check,
        (desc, c): (&'s TableDesc, &'s ConstraintDesc),
        checking: &mut Checking<'s>,
    ) -> Result<()> {
        match check {
            Check::Row { id, .. } => self.check_row(tx, (desc, c), *id, &mut checking.bound),
            Check::Unique { id, .. } => {
                if checking.key.as_ref().is_some_and(|key| !key.checks(&c.name)) {
                    checking.finish()?;
                }
                if checking.key.is_none() {
                    checking.key = KeyCheck::new((self, tx), desc, (&c.name, &c.kind))?;
                }
                checking.key.as_mut().map_or(Ok(()), |key| key.row(*id))
            }
            Check::Table { .. } => self.check_table(tx, (desc, c), &mut checking.bound),
            Check::Referenced { old, id, .. } => {
                let ConstraintKind::ForeignKey(fk) = &c.kind else { return Ok(()) };
                self.check_referenced(tx, (desc, &c.name, fk), old, *id)
            }
            Check::Assertion { constraint } => self.check_assertion(tx, constraint),
        }
    }

    /// Row `id` of `desc`, as it is now, satisfies its constraint `c`: NOT
    /// NULL, a CHECK without a subquery [§11.6 GR3], or a FOREIGN KEY
    /// [§11.8 GR2].
    fn check_row(
        &self,
        tx: &Txn,
        (desc, c): (&TableDesc, &ConstraintDesc),
        id: RowId,
        bound: &mut HashMap<QualifiedName, Cond>,
    ) -> Result<()> {
        let table = &desc.name;
        let Some(row) = self.storage_of(table)?.get(tx.id, id)? else { return Ok(()) };
        match &c.kind {
            ConstraintKind::NotNull(at) => {
                if row.get(*at).is_some_and(Value::is_null) {
                    let column = desc.columns.get(*at).map_or("?", |col| col.name.as_str());
                    return fail(Code::Constraint, format!("[X3.135-1986 §6.3 SR3, GR1] column {column} of {table} is NOT NULL"));
                }
            }
            ConstraintKind::Check { text, names, horizon, .. } => {
                let cond = self.bound(table, (c, (text, names, horizon)), bound)?;
                if eval_cond(&cond, &row, &Heard(tx.stop.warnings()))? == Truth::False {
                    return fail(Code::Constraint, format!("[ISO/IEC 9075:1992 §11.6 GR3] CHECK {} of {table} is false for a row", c.name));
                }
            }
            ConstraintKind::ForeignKey(fk) => {
                let values = project(&row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {table} is short")))?;
                if !self.matches(tx, fk, &values)? {
                    return fail(
                        Code::Constraint,
                        format!("[ISO/IEC 9075:1992 §11.8 GR2] a row of {table} references no row of {}: FOREIGN KEY {}", fk.table, c.name),
                    );
                }
            }
            ConstraintKind::Unique { .. } => {}
        }
        Ok(())
    }

    /// The condition `text` of CHECK `c` of `table`, bound once into
    /// `bound`.
    fn bound(
        &self,
        table: &QualifiedName,
        (c, check): (&ConstraintDesc, (&str, &str, &Horizon)),
        bound: &mut HashMap<QualifiedName, Cond>,
    ) -> Result<Cond> {
        if let Some(cond) = bound.get(&c.name) {
            return Ok(cond.clone());
        }
        let cond = self.check_condition(table, check)?;
        bound.insert(c.name.clone(), cond.clone());
        Ok(cond)
    }

    /// [§11.6 GR3] CHECK `c` of `desc`, with a subquery: of table T with
    /// condition SC, it is not satisfied if and only if EXISTS (SELECT *
    /// FROM T WHERE NOT (SC)) is true; so it is evaluated, over all of T.
    fn check_table(
        self: &Arc<Self>,
        tx: &Txn,
        (desc, c): (&TableDesc, &ConstraintDesc),
        bound: &mut HashMap<QualifiedName, Cond>,
    ) -> Result<()> {
        let ConstraintKind::Check { text, names, reads, horizon } = &c.kind else { return Ok(()) };
        let cond = self.bound(&desc.name, (c, (text, names, horizon)), bound)?;
        self.read_lock(tx, &desc.name)?;
        for t in reads {
            self.read_lock(tx, t)?;
        }
        let columns = desc.columns.iter().map(|column| column.name.clone()).collect();
        let scan = LogicalPlan::Scan { table: desc.name.clone(), names: columns, needed: (0..desc.columns.len()).collect() };
        let falsified = LogicalPlan::Filter { cond: Cond::Not(Box::new(cond)), input: Box::new(scan) };
        let (_, _, physical) = plan(self, falsified);
        if execute(physical, self, tx.id).next().transpose()?.is_some() {
            return fail(Code::Constraint, format!("[ISO/IEC 9075:1992 §11.6 GR3] CHECK {} of {} is false for a row", c.name, desc.name));
        }
        Ok(())
    }
}

/// Whether `columns` given values, or every column if none, include one of
/// `of`.
fn assigned(of: &[usize], columns: Option<&[usize]>) -> bool {
    columns.is_none_or(|set| of.iter().any(|x| set.contains(x)))
}

/// Adds to `out` the checks of row `id` of `desc` written, giving values
/// to `columns`, or to every column if none: of the constraints of its
/// table on a column given a value, but UNIQUE ones (`State::checks`), and
/// of its CHECKs without a subquery.
///
/// # Errors
///
/// The spill area's.
fn row_checks(desc: &TableDesc, id: RowId, columns: Option<&[usize]>, out: &mut HeldRows<Check>) -> Result<()> {
    for c in &desc.constraints {
        let of_row = match &c.kind {
            ConstraintKind::NotNull(at) => assigned(std::slice::from_ref(at), columns),
            ConstraintKind::ForeignKey(fk) => assigned(&fk.columns, columns),
            ConstraintKind::Check { reads, .. } => reads.is_empty(),
            ConstraintKind::Unique { .. } => false,
        };
        if of_row {
            out.push(Check::Row { table: desc.name.clone(), constraint: c.name.clone(), id })?;
        }
    }
    Ok(())
}

/// The row of `desc` `effect` inserted, or updated giving a value to one of
/// the columns `of`, if it did.
fn keyed(effect: &Effect, desc: &TableDesc, of: &[usize]) -> Option<RowId> {
    let (table, id, columns) = match effect {
        Effect::Inserted { table, id } => (table, *id, None),
        Effect::Updated { table, id, columns, .. } => (table, *id, Some(columns.as_slice())),
        Effect::Deleted { .. } => return None,
    };
    (*table == desc.name && assigned(of, columns)).then_some(id)
}
