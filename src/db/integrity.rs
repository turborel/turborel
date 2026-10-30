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

mod references;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::assignment::check_key;
use super::{State, api, plan};
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName, ReferentialAction, TableDesc};
use crate::error::{Code, Result, SqlError, Subject, fail};
use crate::exec::{NoSubqueries, eval_cond, execute};
use crate::logical::{Cond, LogicalPlan, Truth};
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
/// its end; the authorization identifier it runs under, CURRENT_USER to a
/// default its referential actions give [§11.5 GR1c]; and its session's
/// constraint modes, where the checks of deferred constraints wait; the
/// rows its session changed through cursors still open, and where to note
/// a referential action on one [§11.8 GR8].
pub(super) struct Effects<'a> {
    done: Vec<Effect>,
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
    /// constraint modes are `deferral`'s, and whose cursors `cursors` say.
    pub(super) fn new(user: &str, deferral: &'a RefCell<Deferral>, cursors: Cursored<'a>) -> Effects<'a> {
        Effects { done: Vec::new(), user: user.to_string(), now: crate::value::datetime::Now::default(), deferral, cursors }
    }

    /// The authorization identifier the statement runs under.
    pub(super) fn user(&self) -> &str {
        &self.user
    }

    /// Row `id` of `table` inserted.
    pub(super) fn inserted(&mut self, table: &QualifiedName, id: RowId) {
        self.done.push(Effect::Inserted { table: table.clone(), id });
    }

    /// Row `id` of `table` updated from `old`, giving values to `columns`.
    pub(super) fn updated(&mut self, table: &QualifiedName, id: RowId, old: Row, columns: &[usize]) {
        self.done.push(Effect::Updated { table: table.clone(), id, old, columns: columns.to_vec() });
    }

    /// Row `id` of `table`, `old`, deleted.
    pub(super) fn deleted(&mut self, table: &QualifiedName, id: RowId, old: Row) {
        self.done.push(Effect::Deleted { table: table.clone(), id, old });
    }
}

/// One constraint to check of what a statement did: at its end if the
/// constraint is immediate, else when it becomes so.
#[derive(Clone)]
pub(super) enum Check {
    /// NOT NULL, a CHECK without a subquery, or a FOREIGN KEY, of row `id`
    /// of `table`.
    Row { table: QualifiedName, constraint: QualifiedName, id: RowId },
    /// UNIQUE or PRIMARY KEY, of rows `ids` of `table`.
    Unique { table: QualifiedName, constraint: QualifiedName, ids: Vec<RowId> },
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
/// of deferred constraints not yet made.
#[derive(Default)]
pub(super) struct Deferral {
    /// The mode SET CONSTRAINTS ALL gave every deferrable constraint, last:
    /// deferred or not.
    pub(super) all: Option<bool>,
    /// The mode SET CONSTRAINTS gave each constraint it named since.
    pub(super) named: HashMap<QualifiedName, bool>,
    /// The checks waiting.
    pub(super) pending: Vec<Check>,
}

impl Deferral {
    /// [§10.6 GR1, GR2; §14.2 GR2, GR3] Whether constraint `c` is deferred:
    /// deferrable, and so set, or initially deferred.
    pub(super) fn deferred(&self, c: &ConstraintDesc) -> bool {
        c.attributes.deferrable && self.named.get(mode_name(c)).copied().or(self.all).unwrap_or(c.attributes.initially_deferred)
    }
}

/// The name SET CONSTRAINTS gives constraint `c` by: of a domain's
/// constraint on a column, the domain constraint's [§11.21 GR5].
pub(super) fn mode_name(c: &ConstraintDesc) -> &QualifiedName {
    c.domain.as_ref().unwrap_or(&c.name)
}

/// The checks of what a statement did, as they are gathered: those found,
/// the rows of each table and constraint whose UNIQUE is checked, and the
/// tables written.
#[derive(Default)]
struct Gathered {
    out: Vec<Check>,
    seen: HashSet<(QualifiedName, RowId)>,
    keys: HashMap<(QualifiedName, QualifiedName), Vec<RowId>>,
    changed: Vec<QualifiedName>,
}

impl Gathered {
    /// `table` written.
    fn change(&mut self, table: &QualifiedName) {
        if !self.changed.contains(table) {
            self.changed.push(table.clone());
        }
    }

    /// Row `id` of `desc` written, giving values to `columns`, or to every
    /// column if none: the checks of the constraints of its table on a
    /// column given a value, and of its CHECKs without a subquery.
    fn row(&mut self, desc: &TableDesc, id: RowId, columns: Option<&[usize]>) {
        let assigned = |of: &[usize]| columns.is_none_or(|set| of.iter().any(|x| set.contains(x)));
        for c in &desc.constraints {
            let of_row = match &c.kind {
                ConstraintKind::NotNull(at) => assigned(std::slice::from_ref(at)),
                ConstraintKind::ForeignKey(fk) => assigned(&fk.columns),
                ConstraintKind::Check { reads, .. } => reads.is_empty(),
                ConstraintKind::Unique { columns: of, .. } => {
                    if assigned(of) {
                        self.keys.entry((desc.name.clone(), c.name.clone())).or_default().push(id);
                    }
                    false
                }
            };
            if of_row && self.seen.insert((c.name.clone(), id)) {
                self.out.push(Check::Row { table: desc.name.clone(), constraint: c.name.clone(), id });
            }
        }
    }
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
        let mut deferred = Vec::new();
        {
            let deferral = effects.deferral.borrow();
            let mut bound = HashMap::new();
            for check in self.checks(&effects) {
                let Some(c) = self.constraint_of(&check) else { continue };
                if deferral.deferred(c) {
                    deferred.push(check);
                } else {
                    self.check(tx, &check, &mut bound)?;
                }
            }
        }
        effects.deferral.borrow_mut().pending.extend(deferred);
        Ok(())
    }

    /// The constraint `check` checks, if its table still has it.
    pub(super) fn constraint_of(&self, check: &Check) -> Option<&ConstraintDesc> {
        let name = check.constraint();
        let Some(table) = check.table() else { return self.catalog.assertion(name) };
        self.catalog.table(table)?.constraints.iter().find(|c| c.name == *name)
    }

    /// The checks of what `effects` did: of each row inserted, every
    /// constraint of its table; of each updated, those on a column given a
    /// value, and its CHECKs; of each deleted or re-keyed, the foreign keys
    /// that reference it; each CHECK with a subquery of a table written, or
    /// reading one; and each assertion reading one.
    fn checks(&self, effects: &Effects<'_>) -> Vec<Check> {
        let mut g = Gathered::default();
        for effect in &effects.done {
            match effect {
                Effect::Inserted { table, id } => {
                    g.change(table);
                    if let Some(desc) = self.catalog.table(table) {
                        g.row(desc, *id, None);
                    }
                }
                Effect::Updated { table, id, columns, old } => {
                    g.change(table);
                    self.referenced_checks(table, Some(columns), (old, Some(*id)), &mut g.out);
                    if let Some(desc) = self.catalog.table(table) {
                        g.row(desc, *id, Some(columns));
                    }
                }
                Effect::Deleted { table, old, .. } => {
                    g.change(table);
                    self.referenced_checks(table, None, (old, None), &mut g.out);
                }
            }
        }
        let Gathered { mut out, keys, changed, .. } = g;
        for ((table, constraint), ids) in keys {
            out.push(Check::Unique { table, constraint, ids });
        }
        for desc in self.catalog.tables() {
            for c in &desc.constraints {
                let ConstraintKind::Check { reads, .. } = &c.kind else { continue };
                if !reads.is_empty() && (changed.contains(&desc.name) || reads.iter().any(|t| changed.contains(t))) {
                    out.push(Check::Table { table: desc.name.clone(), constraint: c.name.clone() });
                }
            }
        }
        for a in self.catalog.assertions() {
            let ConstraintKind::Check { reads, .. } = &a.kind else { continue };
            if reads.iter().any(|t| changed.contains(t)) {
                out.push(Check::Assertion { constraint: a.name.clone() });
            }
        }
        out
    }

    /// Adds to `out` the NO ACTION checks of the foreign keys that
    /// reference `table`, a row of which, `old`, was deleted, or (with its
    /// `id`) updated, giving values to `columns`: of those whose referenced
    /// columns it gave values to.
    fn referenced_checks(&self, table: &QualifiedName, columns: Option<&[usize]>, (old, id): (&Row, Option<RowId>), out: &mut Vec<Check>) {
        for child in self.catalog.tables() {
            for (name, fk) in child.foreign_keys().filter(|(_, fk)| fk.table == *table) {
                if columns.is_none_or(|set| fk.referenced.iter().any(|r| set.contains(r))) {
                    out.push(Check::Referenced { child: child.name.clone(), constraint: name.clone(), old: old.clone(), id });
                }
            }
        }
    }

    /// Makes `check`, in transaction `tx`, binding each CHECK condition
    /// once into `bound`.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, if the constraint is not satisfied:
    /// about the constraint and its table [ISO/IEC 9075:1992 §18.1 GR3f].
    pub(super) fn check(self: &Arc<Self>, tx: &Txn, check: &Check, bound: &mut HashMap<QualifiedName, Cond>) -> Result<()> {
        let Some(table) = check.table() else { return self.check_assertion(tx, check.constraint()) };
        let (Some(c), Some(desc)) = (self.constraint_of(check), self.catalog.table(table)) else { return Ok(()) };
        let broken = |e: SqlError| {
            if e.code == Code::Constraint && e.subject.is_none() {
                e.about(Subject::constraint(c.name.parts(), desc.name.parts()))
            } else {
                e
            }
        };
        self.check_one(tx, check, (desc, c), bound).map_err(broken)
    }

    /// `check`'s, of constraint `c` of `desc`.
    fn check_one(
        self: &Arc<Self>,
        tx: &Txn,
        check: &Check,
        (desc, c): (&TableDesc, &ConstraintDesc),
        bound: &mut HashMap<QualifiedName, Cond>,
    ) -> Result<()> {
        match check {
            Check::Row { id, .. } => self.check_row(tx, (desc, c), *id, bound),
            Check::Unique { table, ids, .. } => {
                let ConstraintKind::Unique { columns, primary } = &c.kind else { return Ok(()) };
                check_key(self, tx, table, (columns, *primary), ids)
            }
            Check::Table { .. } => self.check_table(tx, (desc, c), bound),
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
                if eval_cond(&cond, &row, &NoSubqueries)? == Truth::False {
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
