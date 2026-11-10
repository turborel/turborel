//! \[EXTENSION\] Plans kept between executions of a prepared statement
//! (RULES.md D45).
//!
//! A query, an INSERT, and a searched or positioned UPDATE or DELETE keep
//! their text parsed and their plan bound, each `?` a slot
//! (`Expr::Dynamic`): each execution fills the slots with its values
//! (`logical/dynamic.rs`), then rewrites and plans the plan, so the access
//! paths are chosen by its values and by the statistics of then, as a plan
//! bound with the values as constants would be. A query, a searched
//! UPDATE or a searched DELETE whose physical plan, of its rows or its
//! object rows, is one probe fixing a whole unique key, which no value
//! changes, keeps that plan too, made once with its slots, and fills it
//! at each execution instead (`physical/kept.rs`). Any other query learns its
//! plan: each of its first executions also plans it with its slots, its
//! paths chosen by its values; once `LEARNED_RUNS` in a row make the two
//! plans one shape (`physical/kept/shape.rs`), it keeps the one with its
//! slots and fills it at each execution. One whose plans keep changing stops learning
//! after `MOST_LEARNING_RUNS`, and is planned at each execution. A table
//! it reads that gains or loses rows as its statistics would be gathered
//! again for has it learn again. A query keeps its `<cursor
//! specification>` parsed, whether opened as an unnamed cursor or as a
//! dynamic cursor [§17.14]. Another statement keeps its text parsed
//! alone.
//!
//! What is kept was made with the definitions and the session as they
//! were (`Key`): the snapshot's generation, which every published change
//! of definitions, settings, functions and the optimizer's rules makes
//! anew; the catalog, which every change made anew, the transaction's own
//! uncommitted ones and privileges granted and revoked among them, and
//! which the session's declared local temporary tables make anew only when
//! they or the catalog under them change (`db/declared.rs`); the
//! authorization identifier, which privileges are checked and CURRENT_USER
//! is valued by; the default schema; the time zone displacement; the
//! session's own settings, SQL.FUNCTION_CALLS among them. An execution
//! that finds any of them changed binds the statement again, so its
//! Syntax and Access Rules are checked again, against the definitions of
//! then [ISO/IEC 9075:1992 §17.6 GR10]: a privilege revoked is missing at
//! the next execution. A binding that reads a value, a `?`'s otherwise
//! than as a slot or the statement's instant [§6.8 GR3], as CURRENT_DATE
//! or a default does, holds it, and is not kept: such a statement is bound
//! at each execution.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::super::write::{Target, Written};
use super::super::{Call, Cursor, Session, State};
use super::Prepared;
use crate::ast::{CursorSpecification, SqlStatement};
use crate::binder::{BoundWrite, Context, OutColumn, bind_cursor};
use crate::catalog::{Catalog, QualifiedName};
use crate::error::Result;
use crate::logical::{LogicalPlan, filled};
use crate::params::Params;
use crate::physical::{Plan, filled_plan, keepable};
use crate::settings::Own;
use crate::sync::lock;
use crate::value::Value;
use learning::{Learning, moved, rows_of};

mod learning;

/// What a prepared statement keeps between its executions, shared by its
/// clones.
#[derive(Debug, Default)]
pub(crate) struct Plans {
    kept: Mutex<Option<Kept>>,
    /// The plans its executions made.
    made: AtomicU64,
}

impl Plans {
    /// How many plans its executions made.
    pub(super) fn made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }
}

/// What is kept, and what it was made with.
#[derive(Debug)]
struct Kept {
    key: Key,
    statement: Option<Arc<SqlStatement>>,
    /// A query's `<cursor specification>`, parsed.
    specification: Option<Arc<CursorSpecification>>,
    plan: Option<KeptPlan>,
    /// A query's physical plan, or an UPDATE's or DELETE's of its object
    /// rows, if its paths follow no value of its.
    physical: Physical,
}

impl Kept {
    /// Nothing kept yet, with `key`.
    fn empty(key: Key) -> Kept {
        Kept { key, statement: None, specification: None, plan: None, physical: Physical::Untried }
    }

    /// Its query's physical plan, kept, or its logical plan to make one
    /// of, if not tried yet; none of a plan that varies with its values,
    /// or of no query.
    fn found(&self) -> Option<Found> {
        let Some(KeptPlan::Query(columns, logical)) = &self.plan else { return None };
        match &self.physical {
            Physical::Untried => Some(Found::Untried(columns.clone(), logical.clone())),
            Physical::Learning(learning) => Some(Found::Learning(columns.clone(), logical.clone(), learning.clone())),
            Physical::Varies => None,
            Physical::Fixed(plan, reads, rows) => Some(Found::Fixed(columns.clone(), (**plan).clone(), Arc::clone(reads), rows.clone())),
        }
    }
}

/// A query's physical plan, kept if its paths follow no value of its
/// (`physical/kept.rs`).
#[derive(Debug)]
enum Physical {
    /// Not yet planned with its slots.
    Untried,
    /// Its paths follow its values: planned at each execution.
    Varies,
    /// Planned at each execution with its values, which it runs, and with
    /// its slots, until `LEARNED_RUNS` in a row give both one shape.
    Learning(Learning),
    /// Kept, with the tables it reads and, of a plan learned, their rows
    /// when it was. Boxed: few statements keep one, and a plan is wide.
    Fixed(Box<Plan>, Arc<[QualifiedName]>, Option<Arc<[usize]>>),
}

/// A query's columns, its physical plan, and the tables it reads.
pub(in crate::db) type Opened = (Vec<OutColumn>, Plan, Arc<[QualifiedName]>);

/// What `Keeping::physical` finds kept of a query: its columns, and its
/// physical plan with the tables it reads and their rows if learned, or
/// its logical plan to make one of, and what it learned.
enum Found {
    Untried(Vec<OutColumn>, LogicalPlan),
    Learning(Vec<OutColumn>, LogicalPlan, Learning),
    Fixed(Vec<OutColumn>, Plan, Arc<[QualifiedName]>, Option<Arc<[usize]>>),
}

/// A statement's plan, bound, its `?`s slots.
#[derive(Debug, Clone)]
pub(in crate::db) enum KeptPlan {
    /// A query's columns and plan.
    Query(Vec<OutColumn>, LogicalPlan),
    /// A searched or positioned UPDATE's or DELETE's, shared with each
    /// execution, which fills its own SET values and object rows' plan.
    Write(Arc<BoundWrite>),
    /// An INSERT's table, and its query's plan, if it has one.
    Insert(Box<Target>, Option<LogicalPlan>),
}

/// What a plan is made with, besides the statement's text: as the module
/// says.
#[derive(Debug)]
struct Key {
    generation: u64,
    catalog: Weak<dyn Catalog>,
    auth: String,
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
}

impl Keeping<'_> {
    /// Whether what is kept with `key` holds now.
    fn holds(&self, key: &Key) -> bool {
        let definitions =
            key.generation == self.st.generation && std::ptr::addr_eq(Weak::as_ptr(&key.catalog), Arc::as_ptr(&self.st.catalog));
        let session =
            key.auth == *self.session.auth() && key.schema == *self.session.schema.borrow() && key.zone == self.session.now.get().zone;
        definitions && session && key.own.same_as(&self.session.own.borrow())
    }

    /// What holds now, for a plan to be kept with.
    fn key(&self) -> Key {
        Key {
            generation: self.st.generation,
            catalog: Arc::downgrade(&self.st.catalog),
            auth: self.session.auth().clone(),
            schema: self.session.default_schema(),
            zone: self.session.now.get().zone,
            own: self.session.own.borrow().clone(),
        }
    }

    /// What `get` takes of what is kept, if it is kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn kept<T>(&self, get: impl FnOnce(&Kept) -> Option<T>) -> Result<Option<T>> {
        let kept = lock(&self.plans.kept)?;
        Ok(kept.as_ref().filter(|k| self.holds(&k.key)).and_then(get))
    }

    /// Keeps what `put` puts, with what holds now: beside what is kept with
    /// it, in place of what was kept with what held before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn keep(&self, put: impl FnOnce(&mut Kept)) -> Result<()> {
        let mut kept = lock(&self.plans.kept)?;
        let holding = kept.take().filter(|k| self.holds(&k.key));
        put(kept.insert(holding.unwrap_or_else(|| Kept::empty(self.key()))));
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
        self.plans.made.fetch_add(1, Ordering::Relaxed);
        let Some(plan) = plan else { return Ok(()) };
        self.keep(|k| k.plan = Some(plan))
    }

    /// A query's columns, its physical plan filled with `values`, and the
    /// tables it reads, if its plan is kept with what holds now: its plan
    /// with slots, filled, if its paths follow no value
    /// (`physical::keepable`) or it learned them, and no table it reads has
    /// moved since (`moved`); else, while it learns, planned with `values`.
    ///
    /// # Errors
    ///
    /// As `physical::filled_plan` and `logical::filled`; SQLCODE -910 if a
    /// lock is poisoned.
    pub(in crate::db) fn physical(&self, values: &[Value]) -> Result<Option<Opened>> {
        match self.kept(Kept::found)? {
            None => Ok(None),
            Some(Found::Untried(columns, logical)) => self.untried((columns, logical), values).map(Some),
            Some(Found::Learning(columns, logical, learning)) => self.learn((columns, logical), learning, values).map(Some),
            Some(Found::Fixed(columns, plan, reads, rows)) => {
                if rows.as_deref().is_some_and(|rows| moved(self.st, &reads, rows)) {
                    self.keep(|k| k.physical = Physical::Learning(Learning::default()))?;
                    return Ok(None);
                }
                Ok(Some((columns, filled_plan(plan, values)?, reads)))
            }
        }
    }

    /// Of a query not yet planned with its slots: its plan with them, made
    /// once, kept and filled if its paths follow no value; else it learns.
    ///
    /// # Errors
    ///
    /// As `physical`.
    fn untried(&self, (columns, logical): (Vec<OutColumn>, LogicalPlan), values: &[Value]) -> Result<Opened> {
        let (optimized, _, plan) = super::super::plan(self.st, logical.clone());
        if !keepable(&plan, self.st) {
            return self.learn((columns, logical), Learning::default(), values);
        }
        let reads: Arc<[QualifiedName]> = optimized.tables().into();
        self.keep(|k| k.physical = Physical::Fixed(Box::new(plan.clone()), Arc::clone(&reads), None))?;
        Ok((columns, filled_plan(plan, values)?, reads))
    }

    /// `w`, a kept UPDATE or DELETE, filled with `values`: its object rows
    /// by its physical plan made once with its slots, kept, if its paths
    /// follow no value (`physical::keepable`); else by its logical plan,
    /// planned as it runs.
    ///
    /// # Errors
    ///
    /// As `Written::filled`; SQLCODE -910 if a lock is poisoned.
    fn written(&self, w: Arc<BoundWrite>, values: &[Value]) -> Result<Written> {
        let fixed = |k: &Kept| match &k.physical {
            Physical::Untried => None,
            Physical::Fixed(plan, reads, _) => Some(Some(((**plan).clone(), Arc::clone(reads)))),
            Physical::Varies | Physical::Learning(_) => Some(None),
        };
        if let Some(kept) = self.kept(fixed)? {
            return Written::filled(w, kept, values);
        }
        let (_, _, plan) = super::super::plan(self.st, w.rows.clone());
        let reads: Arc<[QualifiedName]> = w.rows.tables().into();
        let kept = keepable(&plan, self.st).then_some((plan, reads));
        let physical = kept.as_ref().map_or(Physical::Varies, |(p, r)| Physical::Fixed(Box::new(p.clone()), Arc::clone(r), None));
        self.keep(|k| k.physical = physical)?;
        Written::filled(w, kept, values)
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
        let physical = learned.kept(&reads, rows_of(self.st, &reads));
        self.keep(|k| k.physical = physical)?;
        Ok((columns, plan, reads))
    }
}

impl Session {
    /// `plans`, as an execution in the snapshot `st` sees them.
    pub(in crate::db) fn keeping<'a>(&'a self, st: &'a State, plans: &'a Plans) -> Keeping<'a> {
        Keeping { plans, st, session: self }
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
        let values = params.dynamic_values();
        if let Some(w) = k.kept(|kept| if let Some(KeptPlan::Write(w)) = &kept.plan { Some(Arc::clone(w)) } else { None })? {
            return k.written(w, values);
        }
        let binding = params.keeping();
        let w = Arc::new(self.bound(st, &binding, bind)?);
        if binding.held() {
            k.made(None)?;
            return Written::filled(w, None, values);
        }
        k.made(Some(KeptPlan::Write(Arc::clone(&w))))?;
        k.written(w, values)
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
        let timed = super::super::slow::Timed::of(st, || p.sql.clone(), &self.auth());
        let spec = Self::cursor_specification(st, &p.sql, params, Some(&k))?;
        Self::unnamed_only(&spec)?;
        self.unnamed(call, timed, |snapshot, tx| self.open_rows(snapshot, tx, &spec, (params, Some(&k))))
    }
}

// Unit tests: what is kept of a physical plan, which no API shows
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
