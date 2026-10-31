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
//! - A count of distinct values, alike: a row passing before and now with
//!   the same value, as DISTINCT takes it (`norm_row`), changes nothing; a
//!   row passing now otherwise may add a value, one passing before may take
//!   one away, whether others have it only reading T would tell.
//! - MAX or MIN reached (`MAX(x) > k`): true after the statement if a row it
//!   inserted or updated passes with `x op k` true, whatever the others; if
//!   no row it changed passed before or passes now, the greatest or least is
//!   the one before, at one snapshot as a count is; else checked whole.
//! - MAX or MIN apart (`MAX(x) <> k`): true if a row inserted or updated
//!   passes beyond k. Else, if rows were only added, no row changed having
//!   passed before, each short of k, the greatest after is the one before,
//!   on its side of k, or one of them, short of it: so it holds at one
//!   snapshot, as a count does; else checked whole.
//!
//! Every form is evaluated on the change, as AND evaluates both its
//! operands, so an error any raises is raised (RULES.md D15); the condition
//! is false when one of them is. Where a set function's argument may be
//! null, the whole check raises the warning of a null eliminated [§6.5
//! GR1b] if any row passing has it null: a row of the change shows that,
//! and then the warning is raised; with none, it is checked whole, as only
//! every row tells. Any other form is checked whole.
//!
//! A deferred assertion whose forms are each of no row passing is checked,
//! when it is, from each row the transaction wrote while it was deferred
//! (`Check::AssertedRow`): it held as it was deferred, as it was immediate
//! then, or as each transaction before checked it, and rows are
//! independent. These checks stop at the first row breaking it, so this is
//! for one NOT EXISTS, which stops so too, or for forms, MAX or MIN bounded
//! among them, of steps that raise no error and arguments never null:
//! whichever row comes first, none raises an error or a warning. Any other
//! deferred assertion is checked whole.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::State;
use super::super::integrity::{Check, Effects};
use super::forms::{Form, Kind, Known, Safe, Step, forms_of};
use crate::catalog::{ConstraintKind, QualifiedName};
use crate::error::Result;
use crate::error::SqlState;
use crate::exec::{Heard, HeldRows, eval, eval_cond};
use crate::logical::Cond;
use crate::logical::Truth;
use crate::spill::norm_row;
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
    /// A row passes now; a row passed before.
    passing: Sides,
    /// The rows passing now less those passing before.
    change: i64,
    /// A row passing now has `Kind::Reach`'s condition, or `Kind::Apart`'s
    /// `beyond`, true.
    reached: bool,
    /// A row passing now has `Kind::Apart`'s `within` not true.
    outside: bool,
    /// Of `Kind::Distinct`: a row passing now that did not before with the
    /// same value; one passing before that does not now with it.
    values: Sides,
    /// A row passing the steps before the argument's has it null.
    null: bool,
}

/// Something of the rows of a change as they are now, and as they were.
#[derive(Default, Clone, Copy)]
struct Sides {
    now: bool,
    before: bool,
}

impl Seen {
    /// What it shows of a form of `kind`, in a transaction that reads at
    /// one snapshot if `one_snapshot`.
    fn verdict(&self, kind: &Kind, one_snapshot: bool) -> Verdict {
        let holds = match kind {
            Kind::NoRow | Kind::Bound => return if self.passing.now { Verdict::Broken } else { Verdict::Holds },
            // The count before is the one of this transaction's snapshot.
            Kind::Count(safe) => one_snapshot && moved(*safe, (self.change > 0, self.change < 0)),
            Kind::Distinct(safe, _) => one_snapshot && moved(*safe, (self.values.now, self.values.before)),
            Kind::Reach(_) => self.reached || (one_snapshot && !self.passing.now && !self.passing.before),
            Kind::Apart { .. } => self.reached || (one_snapshot && !self.passing.before && !self.outside),
        };
        if holds { Verdict::Holds } else { Verdict::Unknown }
    }

    /// Takes in a row changed, `now` and `before` it, passing the form of
    /// `kind` now and before as `(is_in, was_in)`.
    fn row(&mut self, kind: &Kind, (now, before): (Option<&Row>, Option<&Row>), (is_in, was_in): (bool, bool), tx: &Txn) -> Result<()> {
        let heard = Heard(tx.stop.warnings());
        let true_of = |c: &Cond, row: Option<&Row>| row.map(|r| eval_cond(c, r, &heard)).transpose().map(|t| t == Some(Truth::True));
        match kind {
            Kind::Reach(reach) => self.reached = self.reached || (is_in && true_of(reach, now)?),
            Kind::Apart { beyond, within } => {
                if is_in {
                    self.reached = self.reached || true_of(beyond, now)?;
                    self.outside = self.outside || !true_of(within, now)?;
                }
            }
            Kind::Distinct(_, x) => {
                let value = |row: Option<&Row>| row.map(|r| eval(x, r, &heard).map(|v| norm_row(&[v]))).transpose();
                let kept = is_in && was_in && value(now)? == value(before)?;
                self.values.now = self.values.now || (is_in && !kept);
                self.values.before = self.values.before || (was_in && !kept);
            }
            Kind::NoRow | Kind::Bound | Kind::Count(_) => {}
        }
        self.passing.now = self.passing.now || is_in;
        self.passing.before = self.passing.before || was_in;
        self.change = self.change.saturating_add(i64::from(is_in)).saturating_sub(i64::from(was_in));
        Ok(())
    }
}

/// Whether a change that adds to a count if `adds` and takes from it if
/// `takes` is one of `safe`'s, that cannot make its comparison false.
fn moved(safe: Safe, (adds, takes): (bool, bool)) -> bool {
    match safe {
        Safe::Fewer => !adds,
        Safe::More => !takes,
        Safe::Same => !adds && !takes,
    }
}

/// How the checks of a deferred assertion are made, as `Check::AssertedRow`
/// meets it: of each row, by its forms; or, none, checked whole once,
/// made.
pub(in crate::db) struct Asserted(Option<Vec<Form>>);

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
    /// the conjunction of `forms`: each evaluated. Where an argument may be
    /// null, it holds only if a row of the change shows the warning raised
    /// of a null eliminated, which it raises then, as the whole check would.
    fn verdict(&self, tx: &Txn, forms: &[Form], effects: &mut Effects<'_>) -> Result<Verdict> {
        let (mut verdict, mut null) = (Verdict::Holds, false);
        for form in forms {
            let (seen, form_verdict) = self.form_verdict(tx, form, effects)?;
            null = null || seen.null;
            verdict = match (verdict, form_verdict) {
                (Verdict::Broken, _) | (_, Verdict::Broken) => Verdict::Broken,
                (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
                (Verdict::Holds, Verdict::Holds) => Verdict::Holds,
            };
        }
        let nullable = forms.iter().any(|f| f.nulls.is_some());
        Ok(match (verdict, nullable, null) {
            (Verdict::Holds, true, false) => Verdict::Unknown,
            (Verdict::Holds, _, true) => {
                // [ISO/IEC 9075:1992 §6.5 GR1b]
                tx.stop.warnings().raise(SqlState::NULL_VALUE_ELIMINATED);
                Verdict::Holds
            }
            (Verdict::Holds, false, false) | (Verdict::Broken | Verdict::Unknown, _, _) => verdict,
        })
    }

    /// What the change `effects` shows of `form`, and what it saw.
    fn form_verdict(&self, tx: &Txn, form: &Form, effects: &mut Effects<'_>) -> Result<(Seen, Verdict)> {
        let storage = self.storage_of(&form.table)?;
        // But of no row passing, every form looks at the rows before too.
        let before_counts = !matches!(form.kind, Kind::NoRow | Kind::Bound);
        let mut seen = Seen::default();
        effects.visit_changed(&form.table, &mut |id, before| {
            if seen.passing.now && matches!(form.kind, Kind::NoRow) {
                return Ok(());
            }
            let now = storage.get(tx.id, id)?;
            let is_in = now.as_ref().map(|row| passes(&form.steps, row, tx)).transpose()?.unwrap_or(false);
            let before = before.filter(|_| before_counts);
            let was_in = before.map(|row| passes(&form.steps, row, tx)).transpose()?.unwrap_or(false);
            if let (Some((at, x)), Some(row), false) = (&form.nulls, &now, seen.null) {
                let heard = Heard(tx.stop.warnings());
                seen.null = passes(form.steps.get(..*at).unwrap_or_default(), row, tx)? && eval(x, row, &heard)?.is_null();
            }
            seen.row(&form.kind, (now.as_ref(), before), (is_in, was_in), tx)
        })?;
        let verdict = seen.verdict(&form.kind, tx.one_snapshot());
        Ok((seen, verdict))
    }

    /// Of assertion `name`, deferred, at the end of the statement that did
    /// `effects`: if its condition is of forms of no row passing, as
    /// `row_forms` finds, the check of each row the statement changed in
    /// each form's table, added to `out`; whether it was.
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
        let Some(forms) = self.row_forms(name)? else { return Ok(false) };
        let mut tables: Vec<&QualifiedName> = Vec::new();
        for form in &forms {
            if !tables.contains(&&form.table) {
                tables.push(&form.table);
            }
        }
        for table in tables {
            effects
                .visit_changed(table, &mut |id, _| out.push(Check::AssertedRow { table: table.clone(), constraint: name.clone(), id }))?;
        }
        Ok(true)
    }

    /// The forms of assertion `name`'s condition, if each is of no row
    /// passing and a row breaks it or not alone: one NOT EXISTS, which
    /// stops at its first row as EXISTS does; or forms of no row passing,
    /// MAX or MIN bounded too, of steps that raise no error and arguments
    /// never null, so whichever row is evaluated first, none raises an
    /// error or a warning.
    fn row_forms(&self, name: &QualifiedName) -> Result<Option<Vec<Form>>> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(None) };
        let forms = forms_of(&self.assertion_plan(a)?, &self.known()).unwrap_or_default();
        let quiet = |f: &Form| {
            let raising = f.steps.iter().any(|s| match s {
                Step::Keep(c) => c.can_raise(),
                Step::Compute(_) => true,
            });
            matches!(f.kind, Kind::NoRow | Kind::Bound) && f.nulls.is_none() && !raising
        };
        let one = matches!(forms.as_slice(), [Form { kind: Kind::NoRow, .. }]);
        Ok(Some(forms).filter(|forms| !forms.is_empty() && (one || forms.iter().all(quiet))))
    }

    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] That row `id` of `table`, as
    /// transaction `tx` sees it, written while assertion `name` was
    /// deferred, leaves it not false: by its forms, kept in `made` for the
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
            let forms = self.row_forms(name)?.filter(|forms| forms.iter().any(|f| f.table == *table));
            if forms.is_none() {
                self.assertion_holds(tx, name, self.assertion_plan(a)?)?;
            }
            made.insert(name.clone(), Asserted(forms));
        }
        let Some(Asserted(Some(forms))) = made.get(name) else { return Ok(()) };
        let Some(now) = self.storage_of(table)?.get(tx.id, id)? else { return Ok(()) };
        for form in forms.iter().filter(|f| f.table == *table) {
            if passes(&form.steps, &now, tx)? {
                return Err(super::broken(name));
            }
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
