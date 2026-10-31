//! \[EXTENSION\] Plans kept between executions of a prepared statement
//! (RULES.md D45).
//!
//! A query, an INSERT, and a searched UPDATE or DELETE keep their text
//! parsed and their plan bound, each `?` a slot (`Expr::Dynamic`): each
//! execution fills the slots with its values (`logical/dynamic.rs`), then
//! rewrites and plans the plan, so the access paths are chosen by its
//! values and by the statistics of then, as a plan bound with the values
//! as constants would be. Another statement keeps its text parsed alone.
//!
//! What is kept was made with the definitions and the session as they
//! were (`Key`): the snapshot's generation, which every published change
//! of definitions, settings, functions and the optimizer's rules makes
//! anew; the catalog, which every change made anew, the transaction's own
//! uncommitted ones and privileges granted and revoked among them; the
//! authorization identifier, which privileges are checked and CURRENT_USER
//! is valued by; the default schema; the time zone displacement; the
//! session's own settings; and whether extensions are on. An execution
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
use super::super::{Call, Cursor, Session, State, Stream};
use super::Prepared;
use crate::ast::SqlStatement;
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
    plan: Option<KeptPlan>,
}

/// A statement's plan, bound, its `?`s slots.
#[derive(Debug, Clone)]
pub(in crate::db) enum KeptPlan {
    /// A query's columns and plan.
    Query(Vec<OutColumn>, LogicalPlan),
    /// A searched UPDATE's or DELETE's.
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
    extensions: bool,
    own: Own,
}

impl Key {
    /// Whether what is kept with `self` holds for `other`.
    fn holds_for(&self, other: &Key) -> bool {
        let definitions = self.generation == other.generation && Weak::ptr_eq(&self.catalog, &other.catalog);
        let session = (&self.auth, &self.schema, self.zone, self.extensions) == (&other.auth, &other.schema, other.zone, other.extensions);
        definitions && session && self.own.same_as(&other.own)
    }
}

/// A prepared statement's plans, as one execution of it sees them.
pub(in crate::db) struct Keeping<'a> {
    plans: &'a Plans,
    key: Key,
}

impl Keeping<'_> {
    /// Its parsed text, if kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn statement(&self) -> Result<Option<Arc<SqlStatement>>> {
        let kept = lock(&self.plans.kept)?;
        Ok(kept.as_ref().filter(|k| k.key.holds_for(&self.key)).and_then(|k| k.statement.clone()))
    }

    /// Its plan, if kept with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn plan(&self) -> Result<Option<KeptPlan>> {
        let kept = lock(&self.plans.kept)?;
        Ok(kept.as_ref().filter(|k| k.key.holds_for(&self.key)).and_then(|k| k.plan.clone()))
    }

    /// Keeps `statement`, its parsed text, with what holds now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn keep_statement(&self, statement: Arc<SqlStatement>) -> Result<()> {
        let mut kept = lock(&self.plans.kept)?;
        match kept.as_mut().filter(|k| k.key.holds_for(&self.key)) {
            Some(k) => k.statement = Some(statement),
            None => *kept = Some(Kept { key: self.key.clone(), statement: Some(statement), plan: None }),
        }
        Ok(())
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
        let mut kept = lock(&self.plans.kept)?;
        match kept.as_mut().filter(|k| k.key.holds_for(&self.key)) {
            Some(k) => k.plan = Some(plan),
            None => *kept = Some(Kept { key: self.key.clone(), statement: None, plan: Some(plan) }),
        }
        Ok(())
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
            extensions: self.extensions,
            own: self.own.borrow().clone(),
        };
        Keeping { plans, key }
    }

    /// A searched UPDATE or DELETE, as `bind` binds it with `params`: of a
    /// prepared statement, as it keeps it, its values filled in.
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

    /// [ISO/IEC 9075:1992 §17.14] `p`, a query, opened as an unnamed cursor
    /// in the call `call` with `params`: its plan as it keeps it, its
    /// values filled in.
    pub(in crate::db) fn open_kept(&self, call: &Call<'_>, p: &Prepared, params: &Params) -> Result<Cursor> {
        let st = call.state();
        let k = self.keeping(st, &p.plans);
        let timed = super::super::slow::Timed::of(st, p.sql.clone(), &self.auth());
        let values = params.dynamic_values();
        if let Some(KeptPlan::Query(columns, plan)) = k.plan()? {
            return self.unnamed(call, timed, |snapshot, tx| Ok((columns, Stream::planned(snapshot, tx, filled(plan, values)?)?)));
        }
        let spec = self.unnamed_specification(st, &p.sql, params)?;
        self.unnamed(call, timed, |snapshot, tx| {
            let binding = params.keeping();
            let bound = self.bound(snapshot, &binding, |cx| bind_cursor(cx, &spec))?;
            k.made((!binding.held()).then(|| KeptPlan::Query(bound.columns.clone(), bound.plan.clone())))?;
            Ok((bound.columns, Stream::planned(snapshot, tx, filled(bound.plan, values)?)?))
        })
    }
}
