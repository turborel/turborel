//! \[EXTENSION\] An assertion checked from the rows a statement changed
//! [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] (RULES.md D51), where its form
//! makes that the same as checking its whole condition; else whole.
//!
//! An immediate assertion is checked at the end of each statement that
//! changes a table it reads [§4.10.1], so it was satisfied as the statement
//! began: the statement before it was checked, or the assertion was
//! checked as it was made or made immediate, and a statement that
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
//! - A count compared with a constant: judged of the count kept, with the
//!   change's, the rows passing now less those that passed before
//!   (`kept.rs`).
//! - A count of distinct values compared with one: a row passing before
//!   and now with the same value, as DISTINCT takes it (`norm_row`),
//!   changes nothing; a row passing now otherwise may add a value, one
//!   passing before may take one away, whether others have it only reading
//!   T would tell. A count no greater satisfies `<` and `<=` if the count
//!   before did, one no smaller `>` and `>=`, one equal any of them; a
//!   change that may move it the other way is checked whole. The count
//!   before is the one this transaction sees, so this holds only for one
//!   that reads at one snapshot, its own: one reading at a snapshot of each
//!   statement checks whole. Whatever others commit meanwhile, a
//!   transaction whose change cannot move the count the wrong way leaves
//!   any state that satisfies it satisfying it once it commits; one whose
//!   change can reads T whole, its reads checked at commit (RULES.md D29).
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
//! Forms of no row passing, and MAX or MIN bounded, may be of a query's
//! rows, inner joins or a grouping, checked by the query run over the rows
//! changed (`queried.rs`, `delta.rs`). SUM and AVG of exact numbers are
//! judged of the totals the assertion keeps, and so are the nulls of a
//! table's form whose argument may be null (`kept.rs`): those of a change
//! are then known whatever rows it changed.
//!
//! A deferred assertion whose forms are each of no row passing is checked,
//! when it is, from each row the transaction wrote while it was deferred
//! (`Check::AssertedRow`): it held as it was deferred, as it was immediate
//! then, or as each transaction before checked it, and rows are
//! independent. These checks stop at the first row breaking it, so this is
//! for one NOT EXISTS, which stops so too, or for forms, MAX or MIN bounded
//! among them, of steps that raise no error and arguments never null:
//! whichever row comes first, none raises an error or a warning. One of
//! any other forms, of a query's rows among them, is checked once every
//! row written is read, from them all together, each form evaluated, as an
//! immediate one is from a statement's: each row once, with what it was
//! before its first change while deferred, kept where a form reads it (a
//! count, a greatest reached or unequal, a grouping). As it held then, a
//! count's change is the rows passing now less those passing then, so at
//! one snapshot, as an immediate count is. Any other deferred assertion is
//! checked whole.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::State;
use super::super::integrity::{Check, Effects};
use super::delta::{Changes, Held};
use super::forms::{Form, Kind, Known, Safe, Source, Step, forms_of};
use super::kept::{keeps, kept_name};
use super::queried::Shape;
use crate::catalog::{ConstraintKind, QualifiedName};
use crate::error::Result;
use crate::error::SqlState;
use crate::exec::{Heard, HeldRows, eval, eval_cond};
use crate::logical::Cond;
use crate::logical::Truth;
use crate::spill::{norm_row, row_bytes};
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
pub(super) struct Seen {
    /// A row passes now; a row passed before.
    pub(super) passing: Sides,
    /// A row passing now has `Kind::Reach`'s condition, or `Kind::Apart`'s
    /// `beyond`, true.
    reached: bool,
    /// A row passing now has `Kind::Apart`'s `within` not true.
    outside: bool,
    /// Of `Kind::Distinct`: a row passing now that did not before with the
    /// same value; one passing before that does not now with it.
    values: Sides,
    /// A row passing the steps before the argument's has it null.
    pub(super) null: bool,
}

/// Something of the rows of a change as they are now, and as they were.
#[derive(Default, Clone, Copy)]
pub(super) struct Sides {
    pub(super) now: bool,
    pub(super) before: bool,
}

impl Seen {
    /// What it shows of a form of `kind`, in a transaction that reads at
    /// one snapshot if `one_snapshot`.
    fn verdict(&self, kind: &Kind, one_snapshot: bool) -> Verdict {
        let holds = match kind {
            Kind::NoRow | Kind::Bound => return if self.passing.now { Verdict::Broken } else { Verdict::Holds },
            // The count of values before is the one of this transaction's snapshot.
            Kind::Distinct(safe, _) => one_snapshot && moved(*safe, (self.values.now, self.values.before)),
            Kind::Reach(_) => self.reached || (one_snapshot && !self.passing.now && !self.passing.before),
            Kind::Apart { .. } => self.reached || (one_snapshot && !self.passing.before && !self.outside),
            // Judged of the totals kept (`kept.rs`).
            Kind::Count(..) | Kind::Total(_) => false,
        };
        if holds { Verdict::Holds } else { Verdict::Unknown }
    }

    /// Takes in a row changed, `now` and `before` it, passing the form of
    /// `kind` now and before as `(is_in, was_in)`.
    pub(super) fn row(
        &mut self,
        kind: &Kind,
        (now, before): (Option<&Row>, Option<&Row>),
        (is_in, was_in): (bool, bool),
        tx: &Txn,
    ) -> Result<()> {
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
            Kind::NoRow | Kind::Bound | Kind::Count(..) | Kind::Total(_) => {}
        }
        self.passing.now = self.passing.now || is_in;
        self.passing.before = self.passing.before || was_in;
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
/// meets it.
pub(in crate::db) struct Asserted(Made);

/// How `Asserted`'s checks are made.
enum Made {
    /// Checked whole once, made.
    Whole,
    /// Of each row, by its forms.
    Rows(Vec<Form>),
    /// Of the rows written, by its forms, once every one is read
    /// (`State::end_asserted`).
    Written(Vec<Form>, Written),
}

/// The rows written while an assertion was deferred, as its checks are
/// read: of each table, each row's id, once, and the rows before kept.
struct Written {
    rows: HashMap<QualifiedName, OfTable>,
    held: Held,
}

/// Of a table's rows written, the ids taken in, and each row's id and the
/// row before its first change while deferred, if kept.
type OfTable = (HashSet<RowId>, Vec<(RowId, Option<Row>)>);

impl Written {
    /// Takes in row `id` of `table`, as it was `before` if kept: once, as it
    /// was before its first change while deferred, so a row changed again
    /// counts once, from what it was to what it is; none once more are
    /// written than are held.
    fn push(&mut self, st: &State, (table, id): (&QualifiedName, RowId), before: Option<&Row>) {
        if self.held.over {
            return;
        }
        let (ids, rows) = self.rows.entry(table.clone()).or_default();
        let more = size_of::<(RowId, RowId, Option<Row>)>().saturating_add(before.map_or(0, row_bytes));
        if ids.insert(id) && self.held.take(st, more) {
            rows.push((id, before.cloned()));
        }
    }
}

impl Changes for Written {
    fn visit(&mut self, table: &QualifiedName, each: &mut dyn FnMut(RowId, Option<&Row>) -> Result<()>) -> Result<()> {
        let Some((_, rows)) = self.rows.get(table) else { return Ok(()) };
        rows.iter().try_for_each(|(id, before)| each(*id, before.as_ref()))
    }
}

impl Changes for Effects<'_> {
    fn visit(&mut self, table: &QualifiedName, each: &mut dyn FnMut(RowId, Option<&Row>) -> Result<()>) -> Result<()> {
        self.visit_changed(table, each)
    }
}

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
            Some(forms) => self.verdict(tx, (name, &forms), effects)?,
            None => Verdict::Unknown,
        };
        match verdict {
            Verdict::Holds => Ok(()),
            Verdict::Broken => Err(super::broken(name)),
            Verdict::Unknown => self.assertion_holds(tx, name, plan),
        }
    }

    /// What finding an assertion's forms takes of this state.
    pub(super) fn known(&self) -> Known<'_> {
        Known { functions: &self.functions, catalog: &*self.catalog }
    }

    /// What the change `changes` shows of assertion `name`, whose condition
    /// is the conjunction of `forms`: each evaluated, those keeping totals
    /// judged of them (`kept.rs`), whose changes are then appended unless
    /// it is false. Where an argument may be null, it holds only if a row
    /// of the change, or the nulls kept, show the warning raised of a null
    /// eliminated, which it raises then, as the whole check would; or if a
    /// warning is raised already, which the statement completes with, as
    /// only the first is kept (`Warnings`).
    fn verdict(self: &Arc<Self>, tx: &Txn, (name, forms): (&QualifiedName, &[Form]), changes: &mut dyn Changes) -> Result<Verdict> {
        let kept = Some(kept_name(name)).filter(|t| self.catalog.table(t).is_some());
        let (mut verdict, mut null, mut unsure, mut appended) = (Verdict::Holds, false, false, Vec::new());
        for (at, form) in forms.iter().enumerate() {
            let kept_here = kept.as_ref().filter(|_| keeps(form));
            let (seen, mut form_verdict) = match (kept_here, &form.kind) {
                (Some(_), Kind::Total(_) | Kind::Count(..)) => (Seen::default(), Verdict::Holds),
                (None | Some(_), _) => self.form_verdict(tx, form, changes)?,
            };
            null = null || seen.null;
            if let Some(table) = kept_here {
                let totals = self.kept_seen(tx, (table, at, form), changes)?;
                form_verdict = totals.truth.map_or(form_verdict, of_truth);
                null = null || totals.null;
                appended.push((table, at, form, totals.change));
            } else {
                unsure = unsure || form.nulls.is_some();
            }
            verdict = match (verdict, form_verdict) {
                (Verdict::Broken, _) | (_, Verdict::Broken) => Verdict::Broken,
                (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
                (Verdict::Holds, Verdict::Holds) => Verdict::Holds,
            };
        }
        let verdict = match (verdict, null, unsure) {
            (Verdict::Holds, true, _) => {
                // [ISO/IEC 9075:1992 §6.5 GR1b]
                tx.stop.warnings().raise(SqlState::NULL_VALUE_ELIMINATED);
                Verdict::Holds
            }
            (Verdict::Holds, false, true) => {
                if tx.stop.warnings().raised() {
                    Verdict::Holds
                } else {
                    Verdict::Unknown
                }
            }
            (Verdict::Holds, false, false) | (Verdict::Broken | Verdict::Unknown, _, _) => verdict,
        };
        if verdict != Verdict::Broken {
            for (table, at, form, change) in appended {
                self.append_totals(tx, (table, at), (change, form))?;
            }
        }
        Ok(verdict)
    }

    /// What the change `changes` shows of `form`, and what it saw.
    fn form_verdict(self: &Arc<Self>, tx: &Txn, form: &Form, changes: &mut dyn Changes) -> Result<(Seen, Verdict)> {
        let seen = match &form.source {
            Source::Table(table) => self.table_seen(tx, (form, table), changes)?,
            Source::Query(query) => {
                let Some(seen) = self.query_seen(tx, (form, query), changes)? else { return Ok((Seen::default(), Verdict::Unknown)) };
                seen
            }
        };
        let verdict = seen.verdict(&form.kind, tx.one_snapshot());
        Ok((seen, verdict))
    }

    /// What the rows of `table` changed, as `changes` holds them, show of
    /// `form`, of the rows of that table: each now and before.
    fn table_seen(&self, tx: &Txn, (form, table): (&Form, &QualifiedName), changes: &mut dyn Changes) -> Result<Seen> {
        let storage = self.storage_of(table)?;
        // But of no row passing, every form looks at the rows before too.
        let before_counts = !matches!(form.kind, Kind::NoRow | Kind::Bound);
        let mut seen = Seen::default();
        changes.visit(table, &mut |id, before| {
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
        Ok(seen)
    }

    /// Of assertion `name`, deferred, at the end of the statement that did
    /// `effects`: if its condition is of forms, as `deferred_forms` finds,
    /// the check of each row the statement changed in each form's tables,
    /// added to `out`, with the row before where a form reads it (a count,
    /// a greatest reached or unequal, a grouping); whether it was.
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
        let Some((forms, _)) = self.deferred_forms(name)? else { return Ok(false) };
        // Each table, and whether the rows before are kept.
        let mut tables: Vec<(QualifiedName, bool)> = Vec::new();
        for form in &forms {
            for table in form.source.tables() {
                let keep = keeps_before(form, &table);
                match tables.iter_mut().find(|(t, _)| *t == table) {
                    Some((_, kept)) => *kept = *kept || keep,
                    None => tables.push((table, keep)),
                }
            }
        }
        for (table, keep) in &tables {
            effects.visit_changed(table, &mut |id, before| {
                let before = before.filter(|_| *keep).cloned();
                out.push(Check::AssertedRow { table: table.clone(), constraint: name.clone(), id, before })
            })?;
        }
        Ok(true)
    }

    /// The forms of assertion `name`'s condition, if each has one, and
    /// whether they are checked of each row alone: each of no row passing,
    /// MAX or MIN bounded too, of a table's rows, one NOT EXISTS, which
    /// stops at its first row as EXISTS does, or forms of steps that raise
    /// no error and arguments never null, so whichever row is evaluated
    /// first, none raises an error or a warning. Else of the rows written
    /// together, each form evaluated (`Made::Written`): a count, or a
    /// greatest reached or unequal, from each row as it was before its
    /// first change while deferred, as it held then.
    fn deferred_forms(&self, name: &QualifiedName) -> Result<Option<(Vec<Form>, bool)>> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(None) };
        let forms = forms_of(&self.assertion_plan(a)?, &self.known()).unwrap_or_default();
        let of_table = |f: &Form| matches!(f.source, Source::Table(_));
        let quiet = |f: &Form| {
            let raising = f.steps.iter().any(|s| match s {
                Step::Keep(c) => c.can_raise(),
                Step::Compute(_) => true,
            });
            f.nulls.is_none() && !raising
        };
        let one = matches!(forms.as_slice(), [Form { kind: Kind::NoRow, .. }]);
        let none_passing = forms.iter().all(|f| matches!(f.kind, Kind::NoRow | Kind::Bound));
        let by_rows = none_passing && forms.iter().all(of_table) && (one || forms.iter().all(quiet));
        Ok((!forms.is_empty()).then_some((forms, by_rows)))
    }

    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] That row `id` of `table`, as
    /// transaction `tx` sees it, written while assertion `name` was
    /// deferred, leaves it not false: by its forms, kept in `made` for the
    /// rows after it; or taken in with the rows written, `before` it what
    /// it was if kept, to be checked once all are (`end_asserted`); or, if
    /// it has neither of that table now, whole, once.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is false.
    pub(in crate::db) fn check_asserted_row(
        self: &Arc<Self>,
        tx: &Txn,
        (table, name, id, before): (&QualifiedName, &QualifiedName, RowId, Option<&Row>),
        made: &mut HashMap<QualifiedName, Asserted>,
    ) -> Result<()> {
        if !made.contains_key(name) {
            let Some(a) = self.catalog.assertion(name) else { return Ok(()) };
            let ConstraintKind::Check { reads, .. } = &a.kind else { return Ok(()) };
            for t in reads {
                self.read_lock(tx, t)?;
            }
            let asserted = match self.deferred_forms(name)?.filter(|(forms, _)| forms.iter().any(|f| f.source.tables().contains(table))) {
                Some((forms, true)) => Made::Rows(forms),
                Some((forms, false)) => Made::Written(forms, Written { rows: HashMap::new(), held: Held::new(self) }),
                None => {
                    self.assertion_holds(tx, name, self.assertion_plan(a)?)?;
                    Made::Whole
                }
            };
            made.insert(name.clone(), Asserted(asserted));
        }
        let forms = match made.get_mut(name) {
            Some(Asserted(Made::Rows(forms))) => forms,
            Some(Asserted(Made::Written(_, written))) => {
                written.push(self, (table, id), before);
                return Ok(());
            }
            Some(Asserted(Made::Whole)) | None => return Ok(()),
        };
        let Some(now) = self.storage_of(table)?.get(tx.id, id)? else { return Ok(()) };
        for form in forms.iter().filter(|f| f.source.tables().contains(table)) {
            if passes(&form.steps, &now, tx)? {
                return Err(super::broken(name));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] Once every check of the
    /// deferred assertions in `made` is read: that each taken in with its
    /// rows written (`Made::Written`) is not false, as transaction `tx`
    /// sees what it reads, from them all together, as an immediate one is
    /// from a statement's change; or whole, if they tell nothing, or were
    /// more than are held. In the order of their names.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the first that is false.
    pub(in crate::db) fn end_asserted(self: &Arc<Self>, tx: &Txn, made: &mut HashMap<QualifiedName, Asserted>) -> Result<()> {
        let mut written: Vec<(QualifiedName, Vec<Form>, Written)> = made
            .drain()
            .filter_map(|(name, Asserted(made))| match made {
                Made::Written(forms, rows) => Some((name, forms, rows)),
                Made::Rows(_) | Made::Whole => None,
            })
            .collect();
        written.sort_by_key(|(name, ..)| name.to_string());
        for (name, forms, mut rows) in written {
            let verdict = if rows.held.over {
                // Its rows past what is held: its totals put right whole.
                self.rebuild_totals(tx, &name)?;
                Verdict::Unknown
            } else {
                self.verdict(tx, (&name, &forms), &mut rows)?
            };
            match (verdict, self.catalog.assertion(&name)) {
                (Verdict::Broken, _) => return Err(super::broken(&name)),
                (Verdict::Unknown, Some(a)) => self.assertion_holds(tx, &name, self.assertion_plan(a)?)?,
                (Verdict::Holds, _) | (Verdict::Unknown, None) => {}
            }
        }
        Ok(())
    }
}

/// Whether `form` reads the rows of `table` as they were before they were
/// changed: of a count, a greatest or least reached or unequal, which
/// compare them with the rows now; of totals kept, whose change they are;
/// of a grouping of `table`, whose groups they were in.
fn keeps_before(form: &Form, table: &QualifiedName) -> bool {
    if keeps(form) {
        return true;
    }
    match &form.source {
        Source::Table(_) => !matches!(form.kind, Kind::NoRow | Kind::Bound),
        Source::Query(query) => match &query.shape {
            Shape::Grouped { table: grouped, .. } => grouped == table,
            Shape::Joined => false,
        },
    }
}

/// What a form's truth after a change shows: false, broken; true or
/// unknown, it holds [ISO/IEC 9075:1992 §11.34 GR2].
fn of_truth(truth: Truth) -> Verdict {
    if truth == Truth::False { Verdict::Broken } else { Verdict::Holds }
}

/// Whether `row` passes every one of `steps`, in order.
///
/// # Errors
///
/// A step's evaluation's.
pub(super) fn passes(steps: &[Step], row: &Row, tx: &Txn) -> Result<bool> {
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
