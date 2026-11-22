//! \[DESIGN\] Throwaway upper-bound experiment, branch `pointbound`, never
//! merged: the point statements of a kept program, run directly past the
//! general statement path, as TURBOREL_POINTBOUND=1 turns it on.
//!
//! A searched UPDATE by a whole unique key, alone or as the statement of a
//! SELECT ... INTO's FINAL or NEW TABLE, and an INSERT of one row of
//! values. Each is recognized once, after the general path made its plans;
//! then its kept bound write, its kept point read and its output
//! expressions are run with the frame's values. Kept: the table locks, the
//! row lock, MVCC write, undo, log and index upkeep (`State::rewrite_row`,
//! `State::insert_routed`). Skipped: binding lookups, effects and the end
//! of statement checks, the delta table's collection and query plan.

#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction, missing_docs)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::super::assignment::{assign_targets, candidate};
use super::super::prepared::{DeltaQuery, KeptPlan, Plans};
use super::super::triggers::Triggered;
use super::super::{Completion, Session, StatementContext};
use super::{Target, Written};
use crate::ast::{DeltaResult, InsertSource, SqlStatement, TableReference};
use crate::binder::BoundWrite;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::{Db, Runner, eval};
use crate::logical::{Expr, LogicalPlan};
use crate::params::Params;
use crate::physical::Kept as KeptPhysical;
use crate::storage::Row;
use crate::txn::Again;
use crate::value::datetime::show_in;

/// The statements run directly so far, of every thread.
pub(in crate::db) static HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the experiment is on: TURBOREL_POINTBOUND=1, read once.
pub(in crate::db) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("TURBOREL_POINTBOUND").is_ok_and(|v| v == "1"))
}

struct Update {
    bound: Arc<BoundWrite>,
    kept: Arc<KeptPhysical>,
    reads: Arc<[QualifiedName]>,
    locks: Vec<QualifiedName>,
    clocked: bool,
    /// Of a SELECT ... INTO over its FINAL TABLE, its select list over the new row.
    outputs: Option<Vec<Expr>>,
}

struct Insert {
    target: Arc<Target>,
    exprs: Vec<Expr>,
    projected: Option<Vec<Expr>>,
    clocked: bool,
}

#[derive(Clone)]
enum Fast {
    Pending(u8),
    No,
    Update(Arc<Update>),
    Insert(Arc<Insert>),
}

struct Entry {
    generation: u64,
    fast: Fast,
}

thread_local! {
    static CACHE: RefCell<HashMap<usize, Entry>> = RefCell::new(HashMap::new());
}

fn store(key: usize, generation: u64, fast: Fast) {
    CACHE.with(|c| {
        c.borrow_mut().insert(key, Entry { generation, fast });
    });
}

/// Runs `stmt`, the kept statement at address `key` whose plans are
/// `plans`, directly, if it is a point statement recognized: none if it
/// is to run the general way.
pub(in crate::db) fn run(
    session: &Session,
    cx: StatementContext<'_>,
    stmt: &SqlStatement,
    key: usize,
    frame: &mut Params,
    (plans, query): (&Plans, &DeltaQuery),
) -> Result<Option<Completion>> {
    // The definitions it was recognized with: the generation and the catalog, as a kept plan's `Key`.
    let generation = cx.st.generation ^ (Arc::as_ptr(&cx.st.catalog).addr() as u64).rotate_left(32);
    let fast = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let e = c.entry(key).or_insert(Entry { generation, fast: Fast::Pending(0) });
        if e.generation != generation {
            *e = Entry { generation, fast: Fast::Pending(0) };
        }
        e.fast.clone()
    });
    let fast = match fast {
        Fast::No => return Ok(None),
        Fast::Pending(0) => {
            store(key, generation, Fast::Pending(1));
            return Ok(None);
        }
        Fast::Pending(n) => match build(session, cx, stmt, (plans, query))? {
            Some(f) => {
                logged(stmt, &f, n);
                store(key, generation, f.clone());
                f
            }
            None => {
                let next = if n >= 8 { Fast::No } else { Fast::Pending(n + 1) };
                if n >= 8 {
                    logged(stmt, &next, n);
                }
                store(key, generation, next);
                return Ok(None);
            }
        },
        f => f,
    };
    let done = match fast {
        Fast::Update(u) => run_update(session, cx, stmt, &u, frame, plans),
        Fast::Insert(i) => run_insert(session, cx, &i, frame, plans),
        Fast::Pending(_) | Fast::No => Ok(None),
    };
    if matches!(done, Ok(Some(_))) {
        HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    done
}

/// With TURBOREL_POINTBOUND_LOG=1, what was decided of `stmt` at its try `n`.
fn logged(stmt: &SqlStatement, f: &Fast, n: u8) {
    static LOG: OnceLock<bool> = OnceLock::new();
    if !*LOG.get_or_init(|| std::env::var("TURBOREL_POINTBOUND_LOG").is_ok_and(|v| v == "1")) {
        return;
    }
    let what = match f {
        Fast::Update(u) => format!("direct UPDATE of {}{}", u.bound.table, if u.outputs.is_some() { " (FINAL TABLE INTO)" } else { "" }),
        Fast::Insert(i) => format!("direct INSERT into {}", i.target.name),
        Fast::No | Fast::Pending(_) => "general path".to_owned(),
    };
    let text = format!("{stmt:?}");
    let text: String = text.chars().take(160).collect();
    eprintln!("pointbound: try {n}: {what}: {text}");
}

fn why(reason: &str) -> Result<Option<Fast>> {
    if std::env::var("TURBOREL_POINTBOUND_LOG").is_ok_and(|v| v == "1") {
        eprintln!("pointbound: not yet: {reason}");
    }
    Ok(None)
}

/// The UPDATE `stmt` is, or that its FINAL or NEW TABLE runs, and whether
/// it is the latter.
fn update_of(stmt: &SqlStatement) -> Option<(&SqlStatement, bool)> {
    match stmt {
        SqlStatement::UpdateSearched { portion: None, .. } => Some((stmt, false)),
        SqlStatement::Select { query, .. } => {
            let [TableReference::Delta(d)] = query.table_expression.from.as_slice() else { return None };
            if !matches!(d.result, DeltaResult::Final | DeltaResult::New) {
                return None;
            }
            matches!(&*d.statement, SqlStatement::UpdateSearched { portion: None, .. }).then_some((&*d.statement, true))
        }
        _ => None,
    }
}

fn build(session: &Session, cx: StatementContext<'_>, stmt: &SqlStatement, (plans, query): (&Plans, &DeltaQuery)) -> Result<Option<Fast>> {
    let st = cx.st;
    if Triggered::of(session, st).is_some() {
        return Ok(Some(Fast::No));
    }
    let k = session.keeping(st, plans);
    if let SqlStatement::Insert(ins) = stmt {
        if !matches!(ins.source, InsertSource::Query(_)) {
            return Ok(Some(Fast::No));
        }
        let Some((KeptPlan::Insert(target, Some(logical)), clocked)) = k.plan_clocked()? else { return Ok(None) };
        if !target.checks.is_empty() {
            return Ok(Some(Fast::No));
        }
        let (exprs, projected) = match &logical {
            LogicalPlan::Values { rows, .. } if rows.len() == 1 => (rows[0].clone(), None),
            LogicalPlan::Project { exprs: p, input } => match &**input {
                LogicalPlan::Values { rows, .. } if rows.len() == 1 => (rows[0].clone(), Some(p.clone())),
                _ => return Ok(Some(Fast::No)),
            },
            _ => return Ok(Some(Fast::No)),
        };
        if !exprs.iter().chain(projected.iter().flatten()).all(Expr::reads_nothing) {
            return Ok(Some(Fast::No));
        }
        let authorization = session.authorization();
        let target = Arc::unwrap_or_clone(target).refreshed(authorization.of(st.catalog.roles()), session.now.get())?;
        return Ok(Some(Fast::Insert(Arc::new(Insert { target: Arc::new(target), exprs, projected, clocked }))));
    }
    let Some((_, select)) = update_of(stmt) else { return Ok(Some(Fast::No)) };
    let Some((KeptPlan::Write(bound), clocked)) = k.plan_clocked()? else { return why("no kept write") };
    let Some((kept, reads)) = k.pointbound_fixed()? else { return why("no fixed plan") };
    if kept.point(st).is_none() {
        return why("fixed plan not a point read");
    }
    let plain = bound.named == bound.table
        && bound.checks.is_empty()
        && bound.portion.is_none()
        && bound.desc.typed.is_none()
        && !st.catalog.partitioning().is_partitioned(&bound.table);
    if !plain {
        return Ok(Some(Fast::No));
    }
    let outputs = if select {
        let q = session.keeping(st, plans).querying(Some(query));
        let Some(q) = q.query() else { return Ok(None) };
        let Some(KeptPlan::Query(_, plan)) = q.plan()? else { return Ok(None) };
        match outputs_of(&plan) {
            Some(o) => Some(o),
            None => return Ok(Some(Fast::No)),
        }
    } else {
        None
    };
    let locks = bound.set_tables();
    Ok(Some(Fast::Update(Arc::new(Update { bound, kept, reads, locks, clocked, outputs }))))
}

/// The select list of a query over its delta table alone, as expressions
/// over the delta table's row.
fn outputs_of(plan: &LogicalPlan) -> Option<Vec<Expr>> {
    let LogicalPlan::Project { exprs, input } = plan else { return None };
    match &**input {
        LogicalPlan::Transition { .. } => Some(exprs.clone()),
        LogicalPlan::Project { exprs: inner, input } if matches!(&**input, LogicalPlan::Transition { .. }) => {
            // Composed, of an inner projection of columns alone.
            let columns: Option<Vec<usize>> = inner.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect();
            let columns = columns?;
            exprs.iter().map(|e| remapped(e, &columns)).collect()
        }
        _ => None,
    }
}

/// `e` over a row whose column `i` is column `columns[i]` of another, as an
/// expression over that other row; none if it reads other than columns
/// directly at its top levels the remap reaches.
fn remapped(e: &Expr, columns: &[usize]) -> Option<Expr> {
    if let Expr::Column(c) = e {
        return columns.get(*c).map(|&k| Expr::Column(k));
    }
    // Identity map only, for any other shape.
    if columns.iter().enumerate().all(|(i, &k)| i == k) { Some(e.clone()) } else { None }
}

fn slots_of(session: &Session, cx: StatementContext<'_>, frame: &Params, plans: &Plans, clocked: bool) -> crate::exec::Slots {
    let slots = frame.dynamic_slots().unwrap_or_default();
    if clocked { session.keeping(cx.st, plans).slots_of(slots, true) } else { slots }
}

fn run_update(
    session: &Session,
    cx: StatementContext<'_>,
    stmt: &SqlStatement,
    u: &Update,
    frame: &mut Params,
    plans: &Plans,
) -> Result<Option<Completion>> {
    let StatementContext { st, tx } = cx;
    if frame.given().is_some() {
        return Ok(None);
    }
    let slots = slots_of(session, cx, frame, plans, u.clocked);
    if !u.kept.places(&slots) {
        return Ok(None);
    }
    let Some(point) = u.kept.point(st) else { return Ok(None) };
    let table = &u.bound.table;
    st.update_lock(tx, table)?;
    for t in u.reads.iter().chain(&u.locks) {
        st.read_lock(tx, t)?;
    }
    let found = point.entry(st, tx.id, &slots)?;
    let Some((id, old)) = found else { return Ok(Some(session.affected(0))) };
    let w = Written::pointed(Arc::clone(&u.bound), (Arc::clone(&u.kept), Arc::clone(&u.reads)), slots);
    let db: Db = w.db(st);
    let runner = Runner::new(&db, tx.id);
    let new = candidate(&w, &old, ("ISO/IEC 9075:1992 §13.10 GR8", &runner))?;
    let mut recomputed = None;
    let mut recheck = |newest: &Row| {
        if !w.holds_of(newest, &runner)? {
            return Ok(Again::Skip);
        }
        let row = candidate(&w, newest, ("ISO/IEC 9075:1992 §13.10 GR8", &runner))?;
        recomputed = Some(row.clone());
        Ok(Again::Update(row))
    };
    let Some(_) = st.rewrite_row(tx, (table, id), Some(&new), &mut recheck)? else { return Ok(Some(session.affected(0))) };
    let done = session.affected(1);
    let Some(outputs) = &u.outputs else { return Ok(Some(done)) };
    let SqlStatement::Select { targets, .. } = stmt else { return Ok(Some(done)) };
    let newest = recomputed.as_ref().unwrap_or(&new);
    let mut row: Row = outputs.iter().map(|e| eval(e, newest, &runner)).collect::<Result<Row>>()?;
    show_in(&mut row, session.now.get().zone);
    assign_targets(&row, targets, frame, "X3.135-1986 §8.10 GR8", "X3.135-1986 §8.10 GR9").map(Some)
}

fn run_insert(session: &Session, cx: StatementContext<'_>, i: &Insert, frame: &mut Params, plans: &Plans) -> Result<Option<Completion>> {
    let StatementContext { st, tx } = cx;
    if frame.given().is_some() || frame.load().is_some() {
        return Ok(None);
    }
    let slots = slots_of(session, cx, frame, plans, i.clocked);
    let db = Db::slotted(st, slots);
    let runner = Runner::new(&db, tx.id);
    let empty = Row::new();
    let mut values: Row = i.exprs.iter().map(|e| eval(e, &empty, &runner)).collect::<Result<Row>>()?;
    if let Some(p) = &i.projected {
        values = p.iter().map(|e| eval(e, &values, &runner)).collect::<Result<Row>>()?;
    }
    let row = i.target.candidate(st, values, &runner)?;
    st.insert_routed(tx, &i.target.name, &row)?;
    Ok(Some(session.affected(1)))
}
