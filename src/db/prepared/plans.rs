//! \[EXTENSION\] Plans kept between executions of a prepared statement
//! (RULES.md D45).
//!
//! A query, an INSERT, and a searched or positioned UPDATE or DELETE keep
//! their text parsed and their plan bound, each `?` a slot
//! (`Expr::Dynamic`): each execution fills the slots with its values
//! (`logical/dynamic.rs`), then rewrites and plans the plan, so the access
//! paths are chosen by its values and by the statistics of then, as a plan
//! bound with the values as constants would be. A query whose physical
//! plan is one probe fixing a whole unique key, which no value changes,
//! keeps that plan too, made once with its slots, and fills it at each
//! execution instead (`physical/kept.rs`). A query keeps its `<cursor
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

use super::super::write::Target;
use super::super::{Call, Cursor, Session, State};
use super::Prepared;
use crate::ast::{CursorSpecification, SqlStatement};
use crate::binder::{BoundWrite, Context, OutColumn, bind_cursor};
use crate::catalog::{Catalog, QualifiedName};
use crate::error::Result;
use crate::logical::{LogicalPlan, filled, filled_expr};
use crate::params::Params;
use crate::physical::{Plan, filled_plan, keepable};
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
    /// A query's physical plan, if its paths follow no value of its.
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
            Physical::Varies => None,
            Physical::Fixed(plan, reads) => Some(Found::Fixed(columns.clone(), (**plan).clone(), Arc::clone(reads))),
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
    /// Kept, with the tables it reads. Boxed: few statements keep one,
    /// and a plan is wide.
    Fixed(Box<Plan>, Arc<[QualifiedName]>),
}

/// A query's columns, its physical plan, and the tables it reads.
pub(in crate::db) type Opened = (Vec<OutColumn>, Plan, Arc<[QualifiedName]>);

/// What `Keeping::physical` finds kept of a query: its columns, and its
/// physical plan with the tables it reads, or its logical plan to make
/// one of.
enum Found {
    Untried(Vec<OutColumn>, LogicalPlan),
    Fixed(Vec<OutColumn>, Plan, Arc<[QualifiedName]>),
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
    /// tables it reads, if its plan is kept with what holds now and its
    /// paths follow no value (`physical::keepable`): made here once, from
    /// the kept plan with its slots, and kept if so.
    ///
    /// # Errors
    ///
    /// As `physical::filled_plan`; SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn physical(&self, values: &[Value]) -> Result<Option<Opened>> {
        let (columns, plan, reads) = match self.kept(Kept::found)? {
            None => return Ok(None),
            Some(Found::Fixed(columns, plan, reads)) => (columns, plan, reads),
            Some(Found::Untried(columns, logical)) => {
                let (optimized, _, plan) = super::super::plan(self.st, logical);
                let fixed = keepable(&plan, self.st);
                let reads: Arc<[QualifiedName]> = optimized.tables().into();
                let physical = if fixed { Physical::Fixed(Box::new(plan.clone()), Arc::clone(&reads)) } else { Physical::Varies };
                self.keep(|k| k.physical = physical)?;
                if !fixed {
                    return Ok(None);
                }
                (columns, plan, reads)
            }
        };
        Ok(Some((columns, filled_plan(plan, values)?, reads)))
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
    pub(in crate::db) fn keeping<'a>(&'a self, st: &'a State, plans: &'a Plans) -> Keeping<'a> {
        Keeping { plans, st, session: self }
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
        let timed = super::super::slow::Timed::of(st, || p.sql.clone(), &self.auth());
        let spec = Self::cursor_specification(st, &p.sql, params, Some(&k))?;
        Self::unnamed_only(&spec)?;
        self.unnamed(call, timed, |snapshot, tx| self.open_rows(snapshot, tx, &spec, (params, Some(&k))))
    }
}

// Unit tests: what is kept of a physical plan, which no API shows
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::value::Decimal;

    /// Verifies: [DESIGN] RULES.md D45: a prepared query's physical plan is
    /// kept once an execution finds it one probe fixing a whole unique
    /// key, after the execution that bound it; a query whose paths follow
    /// its values keeps none; neither binds again.
    #[test]
    fn a_point_querys_physical_plan_is_kept_after_its_first_executions() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER, PRIMARY KEY (A, B))")
            .unwrap();
        let s = db.session("A");
        for start in (0_i32..1200_i32).step_by(100) {
            let values: Vec<String> =
                (start..start + 100_i32).map(|i| format!("({}, {}, {i})", i.div_euclid(10), i.rem_euclid(10))).collect();
            s.run(&format!("INSERT INTO P VALUES {}", values.join(", "))).unwrap();
        }
        s.commit().unwrap();
        let int = |n: i128| Value::Exact(Decimal::new(n, 0));
        let physical = |p: &Prepared| format!("{:?}", lock(&p.plans.kept).unwrap().as_ref().map(|k| &k.physical));
        let point = s.prepare("SELECT C FROM P WHERE A = ? AND B = ?").unwrap();
        let range = s.prepare("SELECT C FROM P WHERE A < ?").unwrap();
        drop(s.open_prepared(&point, &[int(1), int(2)]).unwrap());
        assert!(physical(&point).contains("Untried"), "{}", physical(&point));
        drop(s.open_prepared(&point, &[int(1), int(2)]).unwrap());
        assert!(physical(&point).contains("Fixed"), "{}", physical(&point));
        drop(s.open_prepared(&range, &[int(3)]).unwrap());
        drop(s.open_prepared(&range, &[int(3)]).unwrap());
        assert!(physical(&range).contains("Varies"), "{}", physical(&range));
        assert_eq!([point.plans_made(), range.plans_made()], [1, 1]);
    }
}
