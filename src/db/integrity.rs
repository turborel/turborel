//! \[EXTENSION\] Constraints checked at the end of each statement [ISO/IEC
//! 9075:1992 §10.6 GR3]: what a statement did to rows (`Effects`), its
//! referential actions run first [§11.8 GR5, GR6] (`integrity/actions.rs`),
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
//! constraints waiting, are held as lists that spill past MEMORY.WORK_BYTES
//! (`integrity/held.rs`, RULES.md D38): read back in order, the rows of
//! each UNIQUE constraint checked as they come (`integrity/keys.rs`). What
//! the referential actions keep is bounded alike (`integrity/actions.rs`).

mod actions;
mod gather;
mod held;
mod keys;
mod references;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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

    /// Calls `each` with the rows of `of` the statement changed, each
    /// once, in the order first changed, with what each was before it:
    /// none of one it inserted. What it did is read back as it is held,
    /// spilled or not; only the ids seen are kept.
    ///
    /// # Errors
    ///
    /// The spill area's, reading back what it did, or `each`'s.
    pub(super) fn visit_changed(&mut self, of: &QualifiedName, each: &mut dyn FnMut(RowId, Option<&Row>) -> Result<()>) -> Result<()> {
        let mut seen = HashSet::new();
        self.done.visit(&mut |effect| {
            let (table, id, before) = match effect {
                Effect::Inserted { table, id } => (table, *id, None),
                Effect::Updated { table, id, old, .. } | Effect::Deleted { table, id, old } => (table, *id, Some(old)),
            };
            if table == of && seen.insert(id) {
                return each(id, before);
            }
            Ok(())
        })
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

    /// Of a check of a constraint over all it reads, not of a row: which it
    /// is, the same whichever statement asked for it. One made when the
    /// constraint is checked [ISO/IEC 9075:1992 §4.10.1] does the work of
    /// all.
    pub(super) fn whole(&self) -> Option<Whole> {
        match self {
            Check::Table { table, constraint } => Some((Some(table.clone()), constraint.clone())),
            Check::Assertion { constraint } => Some((None, constraint.clone())),
            Check::Row { .. } | Check::Unique { .. } | Check::Referenced { .. } => None,
        }
    }
}

/// A check over all a constraint reads (`Check::whole`): its table, none of
/// an assertion's, and the constraint.
pub(super) type Whole = (Option<QualifiedName>, QualifiedName);

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
    /// The checks waiting: a check over all a constraint reads only once.
    pub(super) pending: HeldRows<Check>,
    /// Which checks over all a constraint reads are waiting, so a statement
    /// that asks for one again adds none.
    pub(super) wholes: HashSet<Whole>,
}

impl Default for Deferral {
    /// None waiting, and no mode set: as a transaction begins.
    fn default() -> Deferral {
        Deferral { all: None, named: HashMap::new(), pending: HeldRows::new(ListSpace::default()), wholes: HashSet::new() }
    }
}

impl Deferral {
    /// [§10.6 GR1, GR2; §14.2 GR2, GR3] Whether constraint `c` is deferred:
    /// deferrable, and so set, or initially deferred.
    pub(super) fn deferred(&self, c: &ConstraintDesc) -> bool {
        c.attributes.deferrable && self.named.get(mode_name(c)).copied().or(self.all).unwrap_or(c.attributes.initially_deferred)
    }

    /// Whether `check` is one over all a constraint reads, waiting already.
    pub(super) fn waits(&self, check: &Check) -> bool {
        check.whole().is_some_and(|w| self.wholes.contains(&w))
    }

    /// `more` checks to wait, after those waiting; `wholes`, those of them
    /// over all a constraint reads, none waiting already.
    pub(super) fn defer(&mut self, more: HeldRows<Check>, wholes: Vec<Whole>) {
        self.wholes.extend(wholes);
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
        let (mut deferred, mut wholes) = (HeldRows::new(effects.space.clone()), Vec::new());
        let modes = effects.deferral;
        {
            let deferral = modes.borrow();
            let mut checking = Checking::default();
            for check in checks {
                let check = check?;
                let Some(c) = self.constraint_of(&check) else { continue };
                let deferred_now = deferral.deferred(c);
                if deferred_now && deferral.waits(&check) {
                    // A check over all it reads, waiting already: once
                    // does for every statement that asks for it.
                    continue;
                }
                if deferred_now {
                    wholes.extend(check.whole());
                    deferred.push(check)?;
                } else if let Check::Assertion { constraint } = &check {
                    // Immediate, so it held as the statement began.
                    self.check_assertion_after(tx, constraint, &mut effects)?;
                } else {
                    self.check(tx, &check, &mut checking)?;
                }
            }
            checking.finish()?;
        }
        effects.deferral.borrow_mut().defer(deferred, wholes);
        Ok(())
    }

    /// The constraint `check` checks, if its table still has it.
    pub(super) fn constraint_of(&self, check: &Check) -> Option<&ConstraintDesc> {
        let name = check.constraint();
        let Some(table) = check.table() else { return self.catalog.assertion(name) };
        self.catalog.table(table)?.constraints.iter().find(|c| c.name == *name)
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
