//! \[EXTENSION\] An assertion checked from the rows a statement changed
//! [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] (RULES.md D51), where its form
//! makes that the same as checking its whole condition; else whole.
//!
//! An immediate assertion is checked at the end of each statement that
//! changes a table it reads [§4.10.1], so it was satisfied as the statement
//! began: the statement before it was checked, or the assertion was
//! checked whole as it was made or made immediate, and a statement that
//! breaks it is undone. The forms of a conjunct of its condition checked
//! from the change alone (`forms.rs`) are each over the rows of one table
//! T that pass steps of conditions and expressions of the row alone:
//! columns, constants, arithmetic, comparisons, IS NULL, LIKE and CASE, no
//! function, CAST, subquery or outer reference. Such a step gives the same
//! truth, or the same error, every time it is evaluated on a row, and
//! raises no warning; it depends on nothing but the row, so not on the
//! time, the session, nor a function a plug-in may replace. Each row of T
//! was evaluated without an error as it was written, or as the assertion
//! was checked whole: a whole check that finds it satisfied has evaluated
//! every row.
//!
//! - No row passes (`NOT EXISTS`; MAX or MIN bounded, its step `NOT (x op
//!   k)`): as the statement began none did. A row the statement did not
//!   change gives what it gave; one it deleted is gone. So the conjunct is
//!   false after it exactly when a row it inserted or updated passes, and
//!   those rows alone are evaluated. Rows are independent of one another,
//!   so what other transactions write cannot combine with them: each
//!   transaction's rows are checked as it writes them, as a table's CHECK
//!   is. NOT EXISTS stops at the first row passing, as EXISTS does (RULES.md
//!   D15); a set function evaluates every row.
//! - A count compared with a constant: the count after is the count before
//!   plus the rows of the change that pass now less those that passed
//!   before. A count no greater satisfies `<` and `<=` if the count before
//!   did, one no smaller `>` and `>=`, one equal any of them; a change that
//!   moves the count the other way is checked whole. The count before is
//!   the one this transaction sees, so this holds only for one that reads
//!   at one snapshot, its own: one reading at a snapshot of each statement
//!   checks whole. Whatever others commit meanwhile, a transaction whose
//!   change cannot move the count the wrong way leaves any state that
//!   satisfies it satisfying it once it commits; one whose change can reads
//!   T whole, its reads checked at commit (RULES.md D29).
//! - MAX or MIN reached (`MAX(x) > k`): true after the statement if a row it
//!   inserted or updated passes with `x op k` true, whatever the others; if
//!   no row it changed passed before or passes now, the greatest or least is
//!   the one before, at one snapshot as a count is; else checked whole.
//!
//! Every form is evaluated on the change, as AND evaluates both its
//! operands, so an error any raises is raised (RULES.md D15); the condition
//! is false when one of them is. Any other form is checked whole.
//!
//! A deferred assertion whose condition is one form of no row passing is
//! checked, when it is, from each row the transaction wrote while it was
//! deferred (`Check::AssertedRow`): it held as it was deferred, as it was
//! immediate then, or as each transaction before checked it, and rows are
//! independent. Any other deferred assertion is checked whole.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::State;
use super::super::integrity::{Check, Effects};
use super::forms::{Form, Kind, Known, Safe, Step, forms_of};
use crate::catalog::{ConstraintKind, QualifiedName};
use crate::error::Result;
use crate::exec::{Heard, HeldRows, eval, eval_cond};
use crate::logical::Truth;
use crate::storage::{Row, RowId};
use crate::txn::Txn;

/// What a change shows of an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// It is not false.
    Holds,
    /// It is false.
    Broken,
    /// Only checking it whole tells.
    Unknown,
}

/// What the rows of a change showed of a form.
#[derive(Default)]
struct Seen {
    /// A row passes now.
    passes: bool,
    /// A row passed before or passes now.
    touched: bool,
    /// The rows passing now less those passing before.
    change: i64,
    /// A row passing now has `Kind::Reach`'s condition true.
    reached: bool,
}

impl Seen {
    /// What it shows of a form of `kind`, in a transaction that reads at
    /// one snapshot if `one_snapshot`.
    fn verdict(&self, kind: &Kind, one_snapshot: bool) -> Verdict {
        let holds = match kind {
            Kind::NoRow | Kind::Bound => return if self.passes { Verdict::Broken } else { Verdict::Holds },
            // The count before is the one of this transaction's snapshot.
            Kind::Count(safe) => {
                one_snapshot
                    && match safe {
                        Safe::Fewer => self.change <= 0,
                        Safe::More => self.change >= 0,
                        Safe::Same => self.change == 0,
                    }
            }
            Kind::Reach(_) => self.reached || (one_snapshot && !self.touched),
        };
        if holds { Verdict::Holds } else { Verdict::Unknown }
    }
}

/// How the checks of a deferred assertion are made, as `Check::AssertedRow`
/// meets it: of each row, by its one form; or, none, checked whole once,
/// made.
pub(in crate::db) struct Asserted(Option<Form>);

impl State {
    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] That assertion `name`,
    /// immediate, so satisfied as the statement that did `effects` began,
    /// is not false after it, as transaction `tx` sees what it reads: from
    /// the rows the statement changed, where its form allows, else whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is false.
    pub(in crate::db) fn check_assertion_after(self: &Arc<Self>, tx: &Txn, name: &QualifiedName, effects: &mut Effects<'_>) -> Result<()> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(()) };
        let ConstraintKind::Check { reads, .. } = &a.kind else { return Ok(()) };
        for t in reads {
            self.read_lock(tx, t)?;
        }
        let plan = self.assertion_plan(a)?;
        let verdict = match forms_of(&plan, &self.known()) {
            Some(forms) => self.verdict(tx, &forms, effects)?,
            None => Verdict::Unknown,
        };
        match verdict {
            Verdict::Holds => Ok(()),
            Verdict::Broken => Err(super::broken(name)),
            Verdict::Unknown => self.assertion_holds(tx, name, plan),
        }
    }

    /// What finding an assertion's forms takes of this state.
    fn known(&self) -> Known<'_> {
        Known { functions: &self.functions, catalog: &*self.catalog }
    }

    /// What the change `effects` shows of an assertion whose condition is
    /// the conjunction of `forms`: each evaluated.
    fn verdict(&self, tx: &Txn, forms: &[Form], effects: &mut Effects<'_>) -> Result<Verdict> {
        let mut verdict = Verdict::Holds;
        for form in forms {
            verdict = match (verdict, self.form_verdict(tx, form, effects)?) {
                (Verdict::Broken, _) | (_, Verdict::Broken) => Verdict::Broken,
                (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
                (Verdict::Holds, Verdict::Holds) => Verdict::Holds,
            };
        }
        Ok(verdict)
    }

    /// What the change `effects` shows of `form`.
    fn form_verdict(&self, tx: &Txn, form: &Form, effects: &mut Effects<'_>) -> Result<Verdict> {
        let storage = self.storage_of(&form.table)?;
        // Only a count and a greatest or least reached look at the rows before.
        let before_counts = matches!(form.kind, Kind::Count(_) | Kind::Reach(_));
        let mut seen = Seen::default();
        effects.visit_changed(&form.table, &mut |id, before| {
            if seen.passes && matches!(form.kind, Kind::NoRow) {
                return Ok(());
            }
            let now = storage.get(tx.id, id)?;
            let is_in = now.as_ref().map(|row| passes(&form.steps, row, tx)).transpose()?.unwrap_or(false);
            let was_in = before.filter(|_| before_counts).map(|row| passes(&form.steps, row, tx)).transpose()?.unwrap_or(false);
            if let (Kind::Reach(reach), Some(row), true) = (&form.kind, &now, is_in) {
                let reached = eval_cond(reach, row, &Heard(tx.stop.warnings()))? == Truth::True;
                seen.reached = seen.reached || reached;
            }
            seen.passes = seen.passes || is_in;
            seen.touched = seen.touched || is_in || was_in;
            seen.change = seen.change.saturating_add(i64::from(is_in)).saturating_sub(i64::from(was_in));
            Ok(())
        })?;
        Ok(seen.verdict(&form.kind, tx.one_snapshot()))
    }

    /// Of assertion `name`, deferred, at the end of the statement that did
    /// `effects`: if its condition is one form of no row passing, the check
    /// of each row the statement changed in that form's table, added to
    /// `out`; whether it was.
    ///
    /// # Errors
    ///
    /// Its condition does not bind; the spill area's.
    pub(in crate::db) fn defer_asserted_rows(
        &self,
        name: &QualifiedName,
        effects: &mut Effects<'_>,
        out: &mut HeldRows<Check>,
    ) -> Result<bool> {
        let Some(form) = self.row_form(name)? else { return Ok(false) };
        effects.visit_changed(&form.table, &mut |id, _| {
            out.push(Check::AssertedRow { table: form.table.clone(), constraint: name.clone(), id })
        })?;
        Ok(true)
    }

    /// The one form of assertion `name`'s condition, if it has one, of no
    /// row passing, stopping at the first.
    fn row_form(&self, name: &QualifiedName) -> Result<Option<Form>> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(None) };
        let mut forms = forms_of(&self.assertion_plan(a)?, &self.known()).unwrap_or_default();
        let one = forms.pop().filter(|_| forms.is_empty());
        Ok(one.filter(|form| matches!(form.kind, Kind::NoRow)))
    }

    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] That row `id` of `table`, as
    /// transaction `tx` sees it, written while assertion `name` was
    /// deferred, leaves it not false: by its form, kept in `made` for the
    /// rows after it; or, if it has none of that table now, whole, once.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is false.
    pub(in crate::db) fn check_asserted_row(
        self: &Arc<Self>,
        tx: &Txn,
        (table, name, id): (&QualifiedName, &QualifiedName, RowId),
        made: &mut HashMap<QualifiedName, Asserted>,
    ) -> Result<()> {
        if !made.contains_key(name) {
            let Some(a) = self.catalog.assertion(name) else { return Ok(()) };
            let ConstraintKind::Check { reads, .. } = &a.kind else { return Ok(()) };
            for t in reads {
                self.read_lock(tx, t)?;
            }
            let form = self.row_form(name)?.filter(|form| form.table == *table);
            if form.is_none() {
                self.assertion_holds(tx, name, self.assertion_plan(a)?)?;
            }
            made.insert(name.clone(), Asserted(form));
        }
        let Some(Asserted(Some(form))) = made.get(name) else { return Ok(()) };
        let now = self.storage_of(table)?.get(tx.id, id)?;
        if now.map(|row| passes(&form.steps, &row, tx)).transpose()?.unwrap_or(false) {
            return Err(super::broken(name));
        }
        Ok(())
    }
}

/// Whether `row` passes every one of `steps`, in order.
///
/// # Errors
///
/// A step's evaluation's.
fn passes(steps: &[Step], row: &Row, tx: &Txn) -> Result<bool> {
    let heard = Heard(tx.stop.warnings());
    for step in steps {
        match step {
            Step::Keep(c) => {
                if eval_cond(c, row, &heard)? != Truth::True {
                    return Ok(false);
                }
            }
            Step::Compute(exprs) => {
                for e in exprs {
                    eval(e, row, &heard)?;
                }
            }
        }
    }
    Ok(true)
}
