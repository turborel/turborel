//! \[EXTENSION\] Plans kept between executions of a prepared statement
//! (RULES.md D45).
//!
//! A query, an INSERT, a CALL, and a searched or positioned UPDATE or DELETE keep
//! their text parsed and their plan bound, each `?` a slot (`Expr::Dynamic`): each
//! execution fills the slots with its values (`logical/dynamic.rs`), then
//! rewrites and plans the plan, so the access paths are chosen by its values and
//! by the statistics of then, as a plan bound with the values as constants would
//! be. A query, a searched UPDATE or a searched DELETE whose physical plan, of
//! its rows or its object rows, is one probe fixing a whole unique key, which no
//! value changes, keeps that plan too, made once with its slots, filled at each
//! execution instead (`physical/kept.rs`). Any other query learns its plan: each
//! of its first executions also plans it with its slots, its paths chosen by its
//! values; once `LEARNED_RUNS` in a row make the two plans one shape
//! (`physical/kept/shape.rs`), it keeps the one with its slots and fills it at
//! each execution. One whose plans keep changing stops learning after
//! `MOST_LEARNING_RUNS`, and is planned at each execution. A table it reads that
//! gains or loses rows as its statistics would be gathered again for has it learn
//! again. A query keeps its `<cursor specification>` parsed, whether opened as an
//! unnamed cursor or as a dynamic cursor [§17.14]. Another statement keeps its
//! text parsed alone.
//!
//! What is kept was made with the definitions and the session as they
//! were (`Key`): the snapshot's generation, which every published change
//! of definitions, settings, functions and the optimizer's rules makes
//! anew; the catalog, which every change made anew, the transaction's own
//! uncommitted ones and privileges granted and revoked among them, and
//! which the session's declared local temporary tables make anew only when
//! they or the catalog under them change (`db/declared.rs`); the
//! top cell of the authorization stack, its identifier and its role, which
//! privileges are checked and CURRENT_USER and CURRENT_ROLE are valued by,
//! and the SQL-session user identifier, SESSION_USER's; the default schema; the time zone displacement; the
//! session's own settings, SQL.FUNCTION_CALLS among them. An execution
//! that finds any of them changed binds the statement again, so its
//! Syntax and Access Rules are checked again, against the definitions of
//! then [ISO/IEC 9075:1992 §17.6 GR10]: a privilege revoked is missing at
//! the next execution. A binding that reads a value, a `?`'s otherwise
//! than as a slot or the statement's instant [§6.8 GR3], as CURRENT_DATE
//! or a default does, holds it, and is not kept: such a statement is bound
//! at each execution.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use super::super::merge::Inserting;
use super::super::write::{Target, Written};
use super::super::{Call, Cursor, Session, State};
use super::Prepared;
use super::shared::Place;
use crate::ast::{CursorSpecification, SqlStatement};
use crate::binder::{BoundMerge, BoundWrite, Context, OutColumn, bind_cursor};
use crate::catalog::{Catalog, QualifiedName};
use crate::error::Result;
use crate::exec::Slots;
use crate::logical::{LogicalPlan, filled};
use crate::params::Params;
use crate::physical::{Kept as KeptPhysical, keepable, prepared_plan};
use crate::settings::Own;
use crate::sync::write;
use crate::value::Value;
pub(in crate::db) use grown::{Fit, Learnt};
pub(crate) use learning::{Learned, Learning, MOST_LEARNING_RUNS};
pub(in crate::db) use point::Run;
mod delta;
pub(in crate::db) use delta::DeltaQuery;
mod grown;
mod learning;
mod point;

/// What a prepared statement keeps between its executions, shared by its
/// clones, and in its place, by the statements prepared alike
/// (`shared.rs`).
#[derive(Debug, Default)]
pub(crate) struct Plans {
    kept: Arc<Place>,
    /// Its own copy of what its place keeps (`shared.rs`, RULES.md D781).
    own: super::shared::Own,
    /// The plans its executions made.
    made: AtomicU64,
    /// Its text, normalized once (RULES.md D521).
    text: std::sync::OnceLock<Arc<crate::system::Text>>,
}

impl Plans {
    /// What keeps its plans in `place`.
    pub(super) fn in_place(place: Arc<Place>) -> Plans {
        Plans { kept: place, own: super::shared::Own::default(), made: AtomicU64::default(), text: std::sync::OnceLock::default() }
    }

    /// How many plans its executions made.
    pub(super) fn made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }

    /// Its text, `sql`, normalized the first time, never at each run
    /// (RULES.md D521).
    pub(in crate::db) fn text(&self, sql: &str) -> &Arc<crate::system::Text> {
        self.text.get_or_init(|| Arc::new(crate::system::Text::of(sql)))
    }
}

/// What is kept, and what it was made with.
#[derive(Debug)]
pub(super) struct Kept {
    pub(super) key: Key,
    pub(super) statement: Option<Arc<SqlStatement>>,
    /// A query's `<cursor specification>`, parsed.
    pub(super) specification: Option<Arc<CursorSpecification>>,
    pub(super) plan: Option<KeptPlan>,
    /// Whether its plan reads the statement's instant as the slot past its
    /// `?`s (`Params::clock_slot`, RULES.md D613).
    pub(super) clocked: bool,
    /// A query's physical plan, or an UPDATE's or DELETE's of its object
    /// rows, if its paths follow no value of its.
    pub(super) physical: Physical,
}

impl Kept {
    /// Nothing kept yet, with `key`.
    fn empty(key: Key) -> Kept {
        Kept { key, statement: None, specification: None, plan: None, clocked: false, physical: Physical::Untried }
    }

    /// Its query's physical plan, kept, or its logical plan to make one
    /// of, if not tried yet; of a plan that varies with its values, what
    /// its tables were as it stopped learning, if it did; none of no query.
    fn found(&self) -> Option<Found> {
        let (columns, logical) = self.plan.as_ref()?.query()?;
        match &self.physical {
            Physical::Untried => Some(Found::Untried(columns, logical.clone())),
            Physical::Learning(learning) => Some(Found::Learning(columns, logical.clone(), learning.clone())),
            Physical::Varies(None) => None,
            Physical::Varies(Some(stopped)) => Some(Found::Varies(stopped.clone())),
            Physical::Fixed(plan, reads, rows) => Some(Found::Fixed(columns, Arc::clone(plan), Arc::clone(reads), rows.clone())),
        }
    }
}

/// A query's physical plan, kept if its paths follow no value of its
/// (`physical/kept.rs`).
#[derive(Debug)]
pub(super) enum Physical {
    /// Not yet planned with its slots.
    Untried,
    /// Its paths follow its values: planned at each execution; with what
    /// its tables were as it stopped learning, if it did, so it learns
    /// again once one drifts (`grown.rs`).
    Varies(Option<Learnt>),
    /// Planned at each execution with its values, which it runs, and with
    /// its slots, until `LEARNED_RUNS` in a row give both one shape.
    Learning(Learning),
    /// Kept, its slots read where they are, its scalar calls prepared
    /// once (RULES.md D610), shared by its executions; with the tables it
    /// reads and, of a plan learned, what it is measured from (`grown.rs`).
    Fixed(Arc<KeptPhysical>, Arc<[QualifiedName]>, Option<Learnt>),
}

/// What `Keeping::written` finds kept of a write's object rows' plan,
/// once tried: as `Physical` holds it.
enum WriteFound {
    Varies(Option<Learnt>),
    Learning(Learning),
    Fixed(Arc<KeptPhysical>, Arc<[QualifiedName]>, Option<Learnt>),
}

/// A query's columns, its physical plan, the tables it reads, and the
/// values of its slots, of a plan kept with them (RULES.md D610).
pub(in crate::db) type Opened = (Vec<OutColumn>, Run, Arc<[QualifiedName]>, Option<Slots>);

/// What `Keeping::physical` finds kept of a query: its columns, and its
/// physical plan with the tables it reads and their rows if learned, or
/// its logical plan to make one of, and what it learned.
enum Found {
    Untried(Vec<OutColumn>, LogicalPlan),
    Learning(Vec<OutColumn>, LogicalPlan, Learning),
    Varies(Learnt),
    Fixed(Vec<OutColumn>, Arc<KeptPhysical>, Arc<[QualifiedName]>, Option<Learnt>),
}

/// A statement's plan, bound, its `?`s slots.
#[derive(Debug, Clone)]
pub(in crate::db) enum KeptPlan {
    /// A query's columns and plan.
    Query(Vec<OutColumn>, LogicalPlan),
    /// A searched or positioned UPDATE's or DELETE's, shared with each
    /// execution, which fills its own SET values and object rows' plan.
    Write(Arc<BoundWrite>),
    /// An INSERT's table and its query's plan, if any, shared (D902).
    Insert(Arc<Target>, Option<Arc<LogicalPlan>>),
    /// A MERGE, bound, and where each of its WHEN NOT MATCHED clauses
    /// inserts, in order.
    Merge(Arc<BoundMerge>, Arc<[Inserting]>),
    /// A CALL, bound, its arguments filled at each execution (RULES.md D793).
    Call(Arc<crate::db::routines::BoundCall>),
}

/// What a plan is made with, besides the statement's text, as the module says.
#[derive(Debug, Clone)]
pub(in crate::db) struct Key {
    generation: u64,
    catalog: Weak<dyn Catalog>,
    auth: String,
    role: Option<String>,
    session_user: String,
    schema: String,
    zone: i16,
    own: Own,
}

/// A prepared statement's plans, as one execution of it sees them: in the
/// snapshot `st`, by `session`. What holds now is read from them as it is
/// compared with what a plan was kept with, copied only as one is kept.
pub(in crate::db) struct Keeping<'a> {
    plans: &'a Plans,
    st: &'a State,
    session: &'a Session,
    /// Of a SELECT ... INTO of a kept program, its query over its delta table (`delta.rs`).
    query: Option<&'a DeltaQuery>,
}

impl Key {
    /// What holds now for `s` in `st`, for what is kept to be kept with.
    pub(in crate::db) fn now(st: &State, s: &Session) -> Key {
        Key {
            generation: st.generation,
            catalog: Arc::downgrade(&st.catalog),
            auth: s.auth().clone(),
            role: s.role.borrow().clone(),
            session_user: s.user_identifier.borrow().clone(),
            schema: s.default_schema(),
            zone: s.now.get().zone,
            own: s.own.borrow().clone(),
        }
    }

    /// Whether it holds now for `s` in `st`.
    pub(in crate::db) fn holds(&self, st: &State, s: &Session) -> bool {
        let definitions = self.generation == st.generation && std::ptr::addr_eq(Weak::as_ptr(&self.catalog), Arc::as_ptr(&st.catalog));
        let cell = self.auth == *s.auth() && self.role == *s.role.borrow() && self.session_user == *s.user_identifier.borrow();
        let session = cell && self.schema == *s.schema.borrow() && self.zone == s.now.get().zone;
        definitions && session && self.own.same_as(&s.own.borrow())
    }
}

impl Keeping<'_> {
    /// Whether what is kept with `key` holds now.
    fn holds(&self, key: &Key) -> bool {
        key.holds(self.st, self.session)
    }

    /// What holds now, for a plan to be kept with.
    fn key(&self) -> Key {
        Key::now(self.st, self.session)
    }

    /// What `get` takes of what is kept, if it is kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn kept<T>(&self, get: impl FnOnce(&Kept) -> Option<T>) -> Result<Option<T>> {
        self.plans.own.read(&self.plans.kept, |kept| kept.filter(|k| self.holds(&k.key)).and_then(get))
    }

    /// Keeps what `put` puts, with what holds now: beside what is kept with
    /// it, in place of what was kept with what held before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn keep(&self, put: impl FnOnce(&mut Kept)) -> Result<()> {
        let mut kept = write(&self.plans.kept.kept)?;
        let holding = kept.take().filter(|k| self.holds(&k.key));
        put(kept.insert(holding.unwrap_or_else(|| Kept::empty(self.key()))));
        self.plans.kept.changed();
        Ok(())
    }

    /// Its parsed text, if kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn statement(&self) -> Result<Option<Arc<SqlStatement>>> {
        self.kept(|k| k.statement.clone())
    }

    /// Its plan, if kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn plan(&self) -> Result<Option<KeptPlan>> {
        self.kept(|k| k.plan.clone())
    }

    /// Keeps `statement`, its parsed text, with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn keep_statement(&self, statement: Arc<SqlStatement>) -> Result<()> {
        self.keep(|k| k.statement = Some(statement))
    }

    /// Counts a plan made, and keeps it with what holds now, if given: if
    /// it holds no value it read.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn made(&self, plan: Option<KeptPlan>) -> Result<()> {
        self.made_clocked(plan, false)
    }

    /// As `made`, of a plan that reads the statement's instant as a slot if
    /// `clocked`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn made_clocked(&self, plan: Option<KeptPlan>, clocked: bool) -> Result<()> {
        self.plans.made.fetch_add(1, Ordering::Relaxed);
        let Some(plan) = plan else { return Ok(()) };
        self.keep(|k| {
            k.plan = Some(plan);
            k.clocked = clocked;
        })
    }

    /// Its plan and whether it reads the statement's instant as a slot, if
    /// kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn plan_clocked(&self) -> Result<Option<(KeptPlan, bool)>> {
        self.kept(|k| k.plan.clone().map(|p| (p, k.clocked)))
    }

    /// `slots`, an execution's values of its `?`s, and of a plan that reads
    /// the statement's instant as a slot, `clocked`, that instant after
    /// them, as a TIMESTAMP(6) WITH TIME ZONE [ISO/IEC 9075:1992 §6.8 GR3]
    /// (RULES.md D613).
    pub(in crate::db) fn slots_of(&self, slots: Slots, clocked: bool) -> Slots {
        if !clocked {
            return slots;
        }
        let instant = Value::Datetime(self.session.now.get().timestamp());
        slots.iter().cloned().chain(std::iter::once(instant)).collect()
    }

    /// A query's columns, its physical plan and the tables it reads, if
    /// its plan is kept with what holds now: its plan with slots, shared,
    /// with `params`' values for its slots to read where they are
    /// (RULES.md D610), if its paths follow no value
    /// (`physical::keepable`) or it learned them, and no table it reads has
    /// moved since (`moved`); else, while it learns, planned with its
    /// values.
    ///
    /// # Errors
    ///
    /// As `logical::filled`; SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn physical(&self, params: &Params) -> Result<Option<Opened>> {
        let values = params.dynamic_values();
        match self.kept(Kept::found)? {
            None => Ok(None),
            Some(Found::Untried(columns, logical)) => self.untried((columns, logical), params).map(Some),
            Some(Found::Learning(columns, logical, learning)) => self.learn((columns, logical), learning, values).map(Some),
            Some(Found::Varies(stopped)) => {
                if matches!(stopped.now(self.st), Fit::Moved) {
                    self.keep(|k| k.physical = Physical::Learning(Learning::default()))?;
                }
                Ok(None)
            }
            Some(Found::Fixed(columns, plan, reads, learnt)) => {
                if !self.fits(learnt.as_ref(), |l| Physical::Fixed(Arc::clone(&plan), Arc::clone(&reads), Some(l)))? {
                    self.keep(|k| k.physical = Physical::Learning(Learning::default()))?;
                    return Ok(None);
                }
                let (run, slots) = point::run(&plan, params.dynamic_slots());
                Ok(Some((columns, run, reads, slots)))
            }
        }
    }

    /// Whether a plan kept, measured from `learnt` if learned, fits its
    /// tables still (`grown.rs`): if they grew within it, kept again as
    /// `kept` makes it of what it is measured from now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn fits(&self, learnt: Option<&Learnt>, kept: impl FnOnce(Learnt) -> Physical) -> Result<bool> {
        match learnt.map(|l| l.now(self.st)) {
            None | Some(Fit::Still) => Ok(true),
            Some(Fit::Grown(now)) => self.keep(|k| k.physical = kept(now)).map(|()| true),
            Some(Fit::Moved) => Ok(false),
        }
    }

    /// Of a query not yet planned with its slots: its plan with them, made
    /// once, kept if its paths follow no value; else it learns.
    ///
    /// # Errors
    ///
    /// As `physical`.
    fn untried(&self, (columns, logical): (Vec<OutColumn>, LogicalPlan), params: &Params) -> Result<Opened> {
        let (optimized, _, plan) = super::super::plan(self.st, logical.clone());
        if !keepable(&plan, self.st) {
            return self.learn((columns, logical), Learning::default(), params.dynamic_values());
        }
        let reads: Arc<[QualifiedName]> = optimized.tables().into();
        let plan = Arc::new(prepared_plan(plan, crate::optimizer::prepared::budget(self.st))?);
        self.keep(|k| k.physical = Physical::Fixed(Arc::clone(&plan), Arc::clone(&reads), None))?;
        let (run, slots) = point::run(&plan, params.dynamic_slots());
        Ok((columns, run, reads, slots))
    }

    /// `w`, a kept UPDATE or DELETE, run with `slots`: its object rows by
    /// its physical plan made once with its slots, kept, if its paths
    /// follow no value (`physical::keepable`) or it learned them (RULES.md
    /// D422) and no table it reads has moved since; else, while it learns
    /// or once it stopped, planned with its values as it runs.
    ///
    /// # Errors
    ///
    /// As `Written::slotted`; SQLCODE -910 if a lock is poisoned.
    fn written(&self, w: Arc<BoundWrite>, slots: Slots) -> Result<Written> {
        let found = |k: &Kept| match &k.physical {
            Physical::Untried => None,
            Physical::Fixed(plan, reads, rows) => Some(WriteFound::Fixed(Arc::clone(plan), Arc::clone(reads), rows.clone())),
            Physical::Learning(learning) => Some(WriteFound::Learning(learning.clone())),
            Physical::Varies(stopped) => Some(WriteFound::Varies(stopped.clone())),
        };
        match self.kept(found)? {
            Some(WriteFound::Varies(stopped)) => {
                if stopped.is_some_and(|s| matches!(s.now(self.st), Fit::Moved)) {
                    return self.learn_write(w, Learning::default(), slots);
                }
                Written::slotted(w, None, slots)
            }
            Some(WriteFound::Learning(learning)) => self.learn_write(w, learning, slots),
            Some(WriteFound::Fixed(plan, reads, learnt)) => {
                if !self.fits(learnt.as_ref(), |l| Physical::Fixed(Arc::clone(&plan), Arc::clone(&reads), Some(l)))? {
                    return self.learn_write(w, Learning::default(), slots);
                }
                point::written(self.st, w, (&plan, reads), slots)
            }
            None => {
                let (_, _, plan) = super::super::plan_written(self.st, w.rows.clone());
                if !keepable(&plan, self.st) {
                    return self.learn_write(w, Learning::default(), slots);
                }
                let reads: Arc<[QualifiedName]> = w.rows.tables().into();
                let plan = Arc::new(prepared_plan(plan, crate::optimizer::prepared::budget(self.st))?);
                self.keep(|k| k.physical = Physical::Fixed(Arc::clone(&plan), Arc::clone(&reads), None))?;
                point::written(self.st, w, (&plan, reads), slots)
            }
        }
    }

    /// An execution of `w`, a kept UPDATE or DELETE learning its object
    /// rows' plan as a query does (`learn`, RULES.md D422): planned with
    /// `values`, which it runs, and with its slots.
    ///
    /// # Errors
    ///
    /// As `written`.
    fn learn_write(&self, w: Arc<BoundWrite>, was: Learning, slots: Slots) -> Result<Written> {
        let values = &*slots;
        let (_, _, plan) = super::super::plan_written(self.st, filled(w.rows.clone(), values)?);
        let reads: Arc<[QualifiedName]> = w.rows.tables().into();
        let learned = was.after(&plan, super::super::plan_written_slotted(self.st, w.rows.clone(), values));
        let physical = learned.kept(&reads, self.st);
        self.keep(|k| k.physical = physical)?;
        Written::slotted(w, Some((plan, reads)), slots)
    }

    /// An execution of a query learning its plan, which `was` says: its
    /// plan made with `values`, to run, and what is kept after it, as
    /// `Learning::kept` says, from it and its plan made with slots.
    ///
    /// # Errors
    ///
    /// As `physical`.
    fn learn(&self, (columns, logical): (Vec<OutColumn>, LogicalPlan), was: Learning, values: &[Value]) -> Result<Opened> {
        let (optimized, _, plan) = super::super::plan(self.st, filled(logical.clone(), values)?);
        let reads: Arc<[QualifiedName]> = optimized.tables().into();
        let learned = was.after(&plan, super::super::plan_slotted(self.st, logical, values));
        let physical = learned.kept(&reads, self.st);
        self.keep(|k| k.physical = physical)?;
        Ok((columns, Run::Plan(Box::new(plan)), reads, None))
    }
}

impl Session {
    /// `plans`, as an execution in the snapshot `st` sees them.
    pub(in crate::db) fn keeping<'a>(&'a self, st: &'a State, plans: &'a Plans) -> Keeping<'a> {
        Keeping { plans, st, session: self, query: None }
    }

    /// An UPDATE or DELETE, searched or positioned, as `bind` binds it with
    /// `params`: of a prepared statement, as it keeps it, its values
    /// filled in (`Keeping::written`).
    ///
    /// # Errors
    ///
    /// Those of `bind`; as `Keeping::written`.
    pub(in crate::db) fn write_bound(
        &self,
        st: &State,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
        bind: impl FnOnce(Context<'_>) -> Result<BoundWrite>,
    ) -> Result<Written> {
        let Some(k) = keeping else { return self.bound(st, params, bind).map(Written::of) };
        let slots = params.dynamic_slots().unwrap_or_default();
        let found = |kept: &Kept| if let Some(KeptPlan::Write(w)) = &kept.plan { Some((Arc::clone(w), kept.clocked)) } else { None };
        if let Some((w, clocked)) = k.kept(found)? {
            return k.written(w, k.slots_of(slots, clocked));
        }
        let binding = params.keeping_clocked();
        let w = Arc::new(self.bound(st, &binding, bind)?);
        let slots = k.slots_of(slots, binding.clocked());
        if binding.held() {
            k.made(None)?;
            return Written::slotted(w, None, slots);
        }
        k.made_clocked(Some(KeptPlan::Write(Arc::clone(&w))), binding.clocked())?;
        k.written(w, slots)
    }

    /// [X3.135-1986 §8.8 GR3a] The `<cursor specification>` `sql`, parsed
    /// with `params`: of a prepared statement, as it keeps it.
    ///
    /// # Errors
    ///
    /// It does not parse; SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn cursor_specification(
        st: &State,
        sql: &str,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Arc<CursorSpecification>> {
        if let Some(kept) = keeping.map(|k| k.kept(|kept| kept.specification.clone())).transpose()?.flatten() {
            return Ok(kept);
        }
        let spec = Arc::new(Self::parse_cursor(st, sql, params)?);
        if let Some(k) = keeping {
            k.keep(|kept| kept.specification = Some(Arc::clone(&spec)))?;
        }
        Ok(spec)
    }

    /// The columns and plan of the cursor specification `spec`, bound with
    /// `params`: of a prepared statement, as it keeps them, its values
    /// filled in.
    ///
    /// # Errors
    ///
    /// Those of binding it; as `filled`.
    pub(in crate::db) fn cursor_bound(
        &self,
        st: &State,
        spec: &CursorSpecification,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<(Vec<OutColumn>, LogicalPlan)> {
        let Some(k) = keeping else {
            let bound = self.bound(st, params, |cx| bind_cursor(cx, spec))?;
            return Ok((bound.columns, bound.plan));
        };
        let values = params.dynamic_values();
        if let Some(KeptPlan::Query(columns, plan)) = k.plan()? {
            return Ok((columns, filled(plan, values)?));
        }
        let binding = params.keeping();
        let bound = self.bound(st, &binding, |cx| bind_cursor(cx, spec))?;
        k.made((!binding.held()).then(|| KeptPlan::Query(bound.columns.clone(), bound.plan.clone())))?;
        Ok((bound.columns, filled(bound.plan, values)?))
    }

    /// [ISO/IEC 9075:1992 §17.14] `p`, a query, opened as an unnamed cursor
    /// in the call `call` with `params`: its specification and plan as it
    /// keeps them, its values filled in.
    pub(in crate::db) fn open_kept(&self, call: &Call<'_>, p: &Prepared, params: &Params) -> Result<Cursor> {
        let st = call.state();
        let k = self.keeping(st, &p.plans);
        let timed = self.timed(st, || p.sql.clone(), p.plans.text(&p.sql));
        let spec = Self::cursor_specification(st, &p.sql, params, Some(&k))?;
        Self::unnamed_only(&spec)?;
        self.unnamed(call, timed, |snapshot, tx| self.open_rows(snapshot, tx, &spec, (params, Some(&k))))
    }
}

// Unit tests: what is kept of a physical plan, which no API shows
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
