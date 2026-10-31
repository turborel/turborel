//! \[EXTENSION\] Plans kept between executions of a prepared statement
//! (RULES.md D45).
//!
//! A query, an INSERT, and a searched or positioned UPDATE or DELETE keep
//! their text parsed and their plan bound, each `?` a slot
//! (`Expr::Dynamic`): each execution fills the slots with its values
//! (`logical/dynamic.rs`), then rewrites and plans the plan, so the access
//! paths are chosen by its values and by the statistics of then, as a plan
//! bound with the values as constants would be. A query keeps its
//! `<cursor specification>` parsed, whether opened as an unnamed cursor or
//! as a dynamic cursor [§17.14]. Another statement keeps its text parsed
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

use super::super::write::Target;
use super::super::{Call, Cursor, Session, State};
use super::Prepared;
use crate::ast::{CursorSpecification, SqlStatement};
use crate::binder::{BoundWrite, Context, OutColumn, bind_cursor};
use crate::catalog::Catalog;
use crate::error::Result;
use crate::logical::{LogicalPlan, filled, filled_expr};
use crate::params::Params;
use crate::settings::Own;
use crate::sync::lock;
use crate::value::Value;

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
}

impl Kept {
    /// Nothing kept yet, with `key`.
    fn empty(key: &Key) -> Kept {
        Kept { key: key.clone(), statement: None, specification: None, plan: None }
    }
}

/// A statement's plan, bound, its `?`s slots.
#[derive(Debug, Clone)]
pub(in crate::db) enum KeptPlan {
    /// A query's columns and plan.
    Query(Vec<OutColumn>, LogicalPlan),
    /// A searched or positioned UPDATE's or DELETE's.
    Write(Box<BoundWrite>),
    /// An INSERT's table, and its query's plan, if it has one.
    Insert(Box<Target>, Option<LogicalPlan>),
}

/// What a plan is made with, besides the statement's text: as the module
/// says.
#[derive(Debug, Clone)]
pub(in crate::db) struct Key {
    generation: u64,
    catalog: Weak<dyn Catalog>,
    auth: String,
    schema: String,
    zone: i16,
    own: Own,
}

impl Key {
    /// Whether what is kept with `self` holds for `other`.
    fn holds_for(&self, other: &Key) -> bool {
        let definitions = self.generation == other.generation && Weak::ptr_eq(&self.catalog, &other.catalog);
        let session = (&self.auth, &self.schema, self.zone) == (&other.auth, &other.schema, other.zone);
        definitions && session && self.own.same_as(&other.own)
    }
}

/// A prepared statement's plans, as one execution of it sees them.
pub(in crate::db) struct Keeping<'a> {
    plans: &'a Plans,
    key: Key,
}

impl Keeping<'_> {
    /// What `get` takes of what is kept, if it is kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn kept<T>(&self, get: impl FnOnce(&Kept) -> Option<T>) -> Result<Option<T>> {
        let kept = lock(&self.plans.kept)?;
        Ok(kept.as_ref().filter(|k| k.key.holds_for(&self.key)).and_then(get))
    }

    /// Keeps what `put` puts, with what holds now: beside what is kept with
    /// it, in place of what was kept with what held before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    fn keep(&self, put: impl FnOnce(&mut Kept)) -> Result<()> {
        let mut kept = lock(&self.plans.kept)?;
        let holding = kept.take().filter(|k| k.key.holds_for(&self.key));
        put(kept.insert(holding.unwrap_or_else(|| Kept::empty(&self.key))));
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
}

/// `w`, its slots filled with `values`.
///
/// # Errors
///
/// As `filled`.
fn filled_write(w: BoundWrite, values: &[Value]) -> Result<BoundWrite> {
    let set = w.set.into_iter().map(|(c, e)| Ok((c, e.map(|e| filled_expr(e, values)).transpose()?))).collect::<Result<_>>()?;
    Ok(BoundWrite { set, rows: filled(w.rows, values)?, ..w })
}

impl Session {
    /// `plans`, as an execution in the snapshot `st` sees them.
    pub(in crate::db) fn keeping<'a>(&self, st: &State, plans: &'a Plans) -> Keeping<'a> {
        let key = Key {
            generation: st.generation,
            catalog: Arc::downgrade(&st.catalog),
            auth: self.auth().clone(),
            schema: self.default_schema(),
            zone: self.now.get().zone,
            own: self.own.borrow().clone(),
        };
        Keeping { plans, key }
    }

    /// An UPDATE or DELETE, searched or positioned, as `bind` binds it with
    /// `params`: of a prepared statement, as it keeps it, its values
    /// filled in.
    ///
    /// # Errors
    ///
    /// Those of `bind`; as `filled`.
    pub(in crate::db) fn write_bound(
        &self,
        st: &State,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
        bind: impl FnOnce(Context<'_>) -> Result<BoundWrite>,
    ) -> Result<BoundWrite> {
        let Some(k) = keeping else { return self.bound(st, params, bind) };
        let values = params.dynamic_values();
        if let Some(KeptPlan::Write(w)) = k.plan()? {
            return filled_write(*w, values);
        }
        let binding = params.keeping();
        let w = self.bound(st, &binding, bind)?;
        k.made((!binding.held()).then(|| KeptPlan::Write(Box::new(w.clone()))))?;
        filled_write(w, values)
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
        let timed = super::super::slow::Timed::of(st, p.sql.clone(), &self.auth());
        let spec = Self::cursor_specification(st, &p.sql, params, Some(&k))?;
        Self::unnamed_only(&spec)?;
        self.unnamed(call, timed, |snapshot, tx| self.open_rows(snapshot, tx, &spec, (params, Some(&k))))
    }
}
