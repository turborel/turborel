//! The rows a data-changing statement works on: the candidate rows of INSERT,
//! from a query expression [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8], and
//! the object rows of searched UPDATE and DELETE [X3.135-1986 §8.5, §8.12].

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use super::assignment::{candidate, check_option};
use super::defaults::default_row;
use super::generated::{Generating, Inserted};
use super::integrity::Effects;
use super::prepared::{Keeping, KeptPlan};
use super::routines::{Hosted, hosted_rows};
use super::{Session, State, StatementContext, plan, plan_written, rule, set_columns};
use crate::ast::{InsertSource, InsertStatement, QueryExpression, SelectList, SelectSublist};
use crate::binder::{self, BoundWrite, INSERT_RULE, ObjectColumn, ViewCheck};
use crate::catalog::{ColumnDesc, Horizon, Principal, QualifiedName, TableDesc};
use crate::error::{At, Result};
use crate::exec::{Db, HeldRows, Runner, Slots, Subqueries, eval_cond, execute, row_entries};
use crate::logical::{Expr, LogicalPlan, Truth, filled};
use crate::params::Params;
use crate::physical::{Kept as KeptPhysical, Plan};
use crate::storage::{Row, RowId};
use crate::txn::{Again, Txn};
use crate::value::datetime::Now;
use crate::value::{Value, assign};

mod bulk;
mod degree;
mod direct;
mod given;
mod inserted;
mod into;
mod pointed;
mod portion;
pub(crate) mod system_time;
mod targets;

use degree::degree_at;
pub(super) use direct::{Pointed, point_change};
pub(super) use given::not_given;
pub(super) use into::{INSERT_RULES, InsertInto, InsertRules, object_columns};
pub(super) use targets::{Targets, insert_target_in};

/// The table an INSERT puts rows in, and its object columns [X3.135-1986
/// §8.7 SR3, SR4].
#[derive(Debug, Clone)]
pub(super) struct Target {
    /// The base table: it, or an updatable view's [X3.135-1986 §6.5 GR2].
    name: QualifiedName,
    desc: Arc<TableDesc>,
    /// The object columns, as columns of the base table.
    object: Vec<usize>,
    /// [X3.135-1986 §8.7 GR2; ISO/IEC 9075:1992 §11.19 GR11] the views
    /// WITH CHECK OPTION the INSERT spans.
    checks: Vec<ViewCheck>,
    /// [ISO/IEC 9075:1992 §13.8 GR4b i] the default of each column of the
    /// base table.
    defaults: Row,
    /// [ISO/IEC 9075-2:2023 §15.11 GR3a, GR6a] what each row is given of
    /// the identity column, if no value is, and of each generated column.
    generating: Generating,
}

impl Target {
    /// The object columns, in order.
    fn columns(&self) -> impl Iterator<Item = &ColumnDesc> {
        self.object.iter().filter_map(|&c| self.desc.columns.get(c))
    }

    /// The base table.
    pub(super) fn name(&self) -> &QualifiedName {
        &self.name
    }

    /// The object columns, as columns of the base table, in order.
    pub(super) fn object(&self) -> &[usize] {
        &self.object
    }

    /// The views WITH CHECK OPTION the insert spans.
    pub(super) fn checks(&self) -> &[ViewCheck] {
        &self.checks
    }

    /// The object columns as a binder takes them: each its column, its
    /// default, and the generator DEFAULT draws from, of an identity
    /// column whose values the insert gives.
    pub(super) fn objects(&self) -> Vec<ObjectColumn> {
        self.columns()
            .zip(&self.object)
            .map(|(col, &c)| ObjectColumn {
                desc: col.clone(),
                default: self.defaults.get(c).cloned().unwrap_or(Value::Null),
                draws: col.identity().filter(|_| !self.generating.draws()).map(|i| Arc::new(i.sequence.clone())),
            })
            .collect()
    }

    /// It, its defaults those of a statement `user`'s at `now`: as a
    /// prepared statement's kept plan takes it again (RULES.md D45).
    ///
    /// # Errors
    ///
    /// As `default_row`.
    pub(super) fn refreshed(self, user: Principal<'_>, now: Now) -> Result<Target> {
        Ok(Target { defaults: default_row(&self.desc, user, now)?, ..self })
    }

    /// [X3.135-1986 §8.7 GR1a, b, GR5, GR7] the candidate row for the
    /// insert values `values`: each column's default [ISO/IEC 9075:1992
    /// §13.8 GR4b i], null if it has none, but for the object columns, each
    /// set to its value [GR4b ii]; then its identity column's next value,
    /// if it takes it, and each generated column's value [ISO/IEC
    /// 9075-2:2023 §15.11 GR3a, GR6a], by `st`, warnings to `subqueries`.
    pub(super) fn candidate(&self, st: &State, values: Row, subqueries: &dyn Subqueries) -> Result<Row> {
        let mut row = self.defaults.clone();
        for (v, &c) in values.into_iter().zip(&self.object) {
            // The object columns are columns of T [X3.135-1986 §8.7 SR3], so both lookups succeed.
            let (Some(col), Some(slot)) = (self.desc.columns.get(c), row.get_mut(c)) else {
                return Err(rule(format!("[X3.135-1986 §8.7 SR3] {} has no column {c}", self.name)));
            };
            *slot = assign(v, &col.data_type, "ISO/IEC 9075:1992 §13.8 GR4b iii")?;
        }
        self.generating.give(st, &mut row, subqueries)?;
        Ok(row)
    }
}

/// An UPDATE or DELETE as one execution runs it: as bound, shared with
/// the plan a prepared statement keeps (RULES.md D45), and, of a kept
/// plan, the execution's values, which its SET values, its search
/// condition and its object rows' plan kept read where they are as slots
/// (RULES.md D610).
pub(super) struct Written {
    /// As bound; of a kept plan, its slots unfilled.
    pub(super) bound: Arc<BoundWrite>,
    rows: ObjectRows,
    /// The values of its slots, of a kept plan.
    slots: Option<Slots>,
}

/// The plan of an UPDATE's or DELETE's object rows.
enum ObjectRows {
    /// As bound, planned as the statement runs.
    Bound,
    /// A kept plan's, filled, planned as the statement runs.
    Filled(LogicalPlan),
    /// Its kept plan, as this execution runs it (`physical::Kept`), or one
    /// planned for it as a plan is learned; the tables it reads.
    Kept(Plan, Arc<[QualifiedName]>),
    /// Its kept plan, a point read, its row read without the executor's
    /// operators (RULES.md D752); the tables it reads.
    Point(Arc<KeptPhysical>, Arc<[QualifiedName]>),
}

impl Written {
    /// `bound`, as a statement not prepared runs it.
    pub(super) fn of(bound: BoundWrite) -> Written {
        Written { bound: Arc::new(bound), rows: ObjectRows::Bound, slots: None }
    }

    /// `bound`, a kept plan, run with the values `slots`: its object rows
    /// by `kept`, its plan and the tables it reads, if given; else by its
    /// logical plan, filled to be planned with its values.
    ///
    /// # Errors
    ///
    /// As `logical::filled`.
    pub(super) fn slotted(bound: Arc<BoundWrite>, kept: Option<(Plan, Arc<[QualifiedName]>)>, slots: Slots) -> Result<Written> {
        let rows = match kept {
            Some((plan, reads)) => ObjectRows::Kept(plan, reads),
            None => ObjectRows::Filled(filled(bound.rows.clone(), &slots)?),
        };
        Ok(Written { bound, rows, slots: Some(slots) })
    }

    /// `bound`, kept, whose plan kept `kept` is a point read, run with
    /// `slots` (RULES.md D752).
    pub(super) fn pointed(bound: Arc<BoundWrite>, (kept, reads): (Arc<KeptPhysical>, Arc<[QualifiedName]>), slots: Slots) -> Written {
        Written { bound, rows: ObjectRows::Point(kept, reads), slots: Some(slots) }
    }

    /// Its SET values: column position and new value (`None` is NULL), in
    /// the order written.
    pub(super) fn set(&self) -> &[(usize, Option<Expr>)] {
        &self.bound.set
    }

    /// `st`, as its plans and expressions are executed in: with its slots'
    /// values, of a kept plan.
    pub(super) fn db(&self, st: &Arc<State>) -> Db {
        self.slots.as_ref().map_or_else(|| Db::of(st), |slots| Db::slotted(st, Arc::clone(slots)))
    }

    /// Whether row `row` of the object table, a version another
    /// transaction committed after the statement's snapshot, is one it
    /// changes: its views' conditions and its search condition hold of it
    /// [ISO/IEC 9075:1992 §4.28] (RULES.md D29), its slots read by
    /// `runner`, which runs it (`db`).
    ///
    /// # Errors
    ///
    /// As `eval_cond`.
    pub(in crate::db) fn holds_of(&self, row: &Row, runner: &Runner) -> Result<bool> {
        let LogicalPlan::Filter { cond, .. } = &self.bound.rows else { return Ok(true) };
        Ok(eval_cond(cond, row, runner)? == Truth::True)
    }
}

impl Session {
    /// [X3.135-1986 §8.7] `<insert statement>`; of a prepared statement,
    /// with the plans it keeps (RULES.md D45).
    /// The rows it inserted [ISO/IEC 9075:1992 §18.1 GR1e i].
    pub(super) fn insert_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &InsertStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<usize> {
        let (target, query, clocked) = self.insert_bound(cx.st, stmt, params, keeping)?;
        let slots = keeping.and_then(|k| Some(k.slots_of(params.dynamic_slots()?, clocked)));
        let kept = keeping.filter(|_| !clocked).map(|k| (k, params));
        self.insert_rows(cx, (stmt, &target), (query, slots, kept), params)
    }

    /// The rows of `stmt`, an INSERT into `target` of the rows of `query`,
    /// if it has one, inserted, their slots' values `slots`, as
    /// `insert_statement` inserts them: so too of a compiled program's
    /// INSERT, its target and query kept (RULES.md D839).
    pub(super) fn insert_rows(
        &self,
        cx: StatementContext<'_>,
        (stmt, target): (&InsertStatement, &Target),
        (query, slots, kept): (Option<LogicalPlan>, Option<Slots>, Option<(&Keeping<'_>, &Params)>),
        params: &Params,
    ) -> Result<usize> {
        let StatementContext { st, tx } = cx;
        let rows = if let Some(given) = params.given() {
            // [EXTENSION] the rows given it, each its parameters' values (RULES.md D530).
            given::given_rows(st, (stmt, target), given, &Runner::new(st, tx.id))?
        } else if let Some(logical) = query {
            query_rows((self, cx), target, (logical, slots, kept))?
        } else {
            // [ISO/IEC 9075:1992 §13.8 SR2] VALUES (DEFAULT, ...): every
            // column its default, or its generated value.
            let mut one = HeldRows::new(st.list_space());
            one.push(target.candidate(st, Row::new(), &Runner::new(st, tx.id))?)?;
            one
        };
        let mut effects = self.effects(st);
        // [X3.135-1986 §8.7 GR4] R is empty: no row is inserted, though its
        // statement-level triggers execute [ISO/IEC 9075-2:2023 §4.46.2].
        if rows.is_empty() && effects.triggered.is_none() {
            // [EXTENSION] of none that ends a load, the load ended (RULES.md D532).
            bulk::load::end(st, tx, params.load())?;
            return Ok(0);
        }
        self.settle_cursors(st, &target.name);
        // [ISO/IEC 9075-2:2023 §15.11 GR4] its BEFORE triggers first.
        let mut rows = effects.before_insert(cx, &target.name, rows)?;
        // [X3.135-1986 §8.7 GR1c], [EXTENSION] in its partition (RULES.md D155), a batch at a time where it may be (D530).
        bulk::insert_held((st, tx), &target.name, &mut rows, &mut effects, params.load())?;
        if !target.checks.is_empty() {
            let runner = Runner::new(st, tx.id);
            rows.visit(&mut |row| check_option(&target.checks, [row], ("X3.135-1986 §8.7 GR2", &runner)))?;
        }
        st.end_statement(tx, effects)?;
        self.note_drawn(target.generating.drawn_identity(), &mut rows)?;
        self.note_generated(target.generating.identity_column(), &mut rows)?;
        Ok(rows.len())
    }

    /// The table `stmt` inserts into and the plan of the rows its query
    /// gives, if it has one: of a prepared statement, as it keeps them,
    /// its slots unfilled, for `query_rows` to read (RULES.md D45, D610);
    /// and whether the plan reads the statement's instant as a slot
    /// (RULES.md D613).
    fn insert_bound(
        &self,
        st: &State,
        stmt: &InsertStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<(Target, Option<LogicalPlan>, bool)> {
        if let Some((KeptPlan::Insert(target, query), clocked)) = keeping.map(Keeping::plan_clocked).transpose()?.flatten() {
            let authorization = self.authorization();
            let target = Arc::unwrap_or_clone(target).refreshed(authorization.of(st.catalog.roles()), self.now.get())?;
            return Ok((target, query, clocked));
        }
        let kept = keeping.map(|_| params.keeping_clocked());
        let binding = kept.as_ref().unwrap_or(params);
        // Run directly, its table is resolved once while what it was resolved with holds.
        let target = if keeping.is_none() && params.sees_whole() {
            self.insert_target_kept(st, stmt)?
        } else {
            self.insert_target(st, stmt, (binding.noted(), binding.horizon()))?
        };
        let query = match &stmt.source {
            InsertSource::Query(query) => Some(self.insert_query(st, &target, query, binding)?),
            InsertSource::DefaultValues => None,
        };
        let Some(k) = keeping else { return Ok((target, query, false)) };
        k.made_clocked((!binding.held()).then(|| KeptPlan::Insert(Arc::new(target.clone()), query.clone())), binding.clocked())?;
        Ok((target, query, binding.clocked()))
    }

    /// [X3.135-1986 §8.7 SR1-SR4] the table `stmt` inserts into, which is
    /// not read-only, and its object columns: through an updatable view,
    /// its base table's [X3.135-1986 §6.5 GR2]. A view's read of the
    /// statement's instant is noted in `noted`, if given (`Clocked`).
    pub(super) fn insert_target(
        &self,
        st: &State,
        stmt: &InsertStatement,
        (noted, horizon): (Option<&Cell<bool>>, &Horizon),
    ) -> Result<Target> {
        let into = InsertInto {
            table: &stmt.table,
            columns: stmt.columns.as_deref(),
            given: (Inserted::Source(&stmt.source), stmt.overriding),
            rules: &INSERT_RULES,
            horizon,
        };
        self.insert_target_of(st, &into, noted)
    }

    /// The table `into` inserts into, which is not read-only, and its object
    /// columns, as `insert_target` gives them, of an INSERT or of a
    /// MERGE's WHEN NOT MATCHED clause, each citing its own rules.
    pub(super) fn insert_target_of(&self, st: &State, into: &InsertInto<'_>, noted: Option<&Cell<bool>>) -> Result<Target> {
        let authorization = self.authorization();
        insert_target_in(st, into, (authorization.of(st.catalog.roles()), &self.default_schema(), self.now.get()), noted)
    }

    /// [X3.135-1986 §8.7 SR5, SR6] The `<query expression>` `query` of an
    /// INSERT into `target`, bound: its columns those of `target`'s object
    /// columns, NULL and DEFAULT among its values theirs.
    pub(super) fn bound_insert(&self, st: &State, target: &Target, query: &QueryExpression, params: &Params) -> Result<binder::Bound> {
        let objects = target.objects();
        self.bound(st, params, |bound| binder::bind_insert_query(bound, query, &objects))
    }

    /// The plan of the rows of `query` inserted into `target`, its degree
    /// and its columns' types checked [X3.135-1986 §8.7 SR5, SR6].
    fn insert_query(&self, st: &State, target: &Target, query: &QueryExpression, params: &Params) -> Result<LogicalPlan> {
        let bound = self.bound_insert(st, target, query, params)?;
        if bound.columns.len() != target.object.len() {
            let (n, m) = (bound.columns.len(), target.object.len());
            let failed = rule(format!("[X3.135-1986 §8.7 SR5; ISO/IEC 9075:1992 §13.8 SR5] a query of degree {n} for {m} columns"));
            return Err(degree_at(query, m).map_or(failed.clone(), |at| failed.or_at(at)));
        }
        // [X3.135-1986 §8.7 SR6] placed at its select list item, if written;
        // a <table value constructor>'s values are checked as bound.
        let written: Vec<usize> = match query.body() {
            QueryExpression::Spec(spec) => match &spec.select_list {
                SelectList::Values(items) => items
                    .iter()
                    .map(|i| if let SelectSublist::Derived(d) = i { Some(d.at) } else { None })
                    .collect::<Option<_>>()
                    .unwrap_or_default(),
                SelectList::Star => Vec::new(),
            },
            QueryExpression::Values(_) | QueryExpression::SetOp(_) | QueryExpression::With(_) | QueryExpression::Ordered(_) => Vec::new(),
        };
        if !matches!(query.body(), QueryExpression::Values(_)) {
            for (n, (out, col)) in bound.columns.iter().zip(target.columns()).enumerate() {
                // [ISO/IEC 9075-2:2023 §9.2 SR3] or by an appropriate cast.
                if binder::assignment_cast(&*st.catalog, &out.data_type, &col.data_type).is_some() {
                    continue;
                }
                let checked = binder::check_assignment(col, &out.data_type.clone(), INSERT_RULE);
                match written.get(n) {
                    Some(&at) => checked.or_at(at)?,
                    None => checked?,
                }
            }
        }
        Ok(bound.plan)
    }
}

/// [X3.135-1986 §8.7 GR3, GR4; ISO/IEC 9075:1992 §13.8 GR3, GR4] the
/// candidate rows of a `<query expression>`, bound as `logical`, one for
/// each row of its result, which is evaluated in full before any row is
/// inserted.
///
/// Of VALUES that invoke a function that possibly modifies SQL-data, each
/// row is evaluated in turn by the session (`routines::hosted_rows`).
fn query_rows(
    (session, cx): (&Session, StatementContext<'_>),
    target: &Target,
    (logical, slots, kept): (LogicalPlan, Option<Slots>, Option<(&Keeping<'_>, &Params)>),
) -> Result<HeldRows<Row>> {
    let StatementContext { st, tx } = cx;
    let db = slots.as_ref().map_or_else(|| Db::of(st), |slots| Db::slotted(st, Arc::clone(slots)));
    let hosted = Hosted::in_db(Some(session), cx, &db);
    // Held up to MEMORY.WORK_BYTES, the rest spilled (RULES.md D38).
    let mut rows = HeldRows::new(st.list_space());
    if let Some(found) = hosted_rows(&logical, &hosted) {
        for values in found? {
            rows.push(target.candidate(st, values, &hosted)?)?;
        }
        return Ok(rows);
    }
    // A query other than a row of values, by its plan kept (RULES.md
    // D793); else planned with its values, as its paths follow them.
    if let Some(held) = inserted::kept_rows((session, cx), target, kept)? {
        return Ok(held);
    }
    let logical = match &slots {
        Some(slots) => filled(logical, slots)?,
        None => logical,
    };
    let (optimized, _, physical) = plan(st, logical);
    for t in optimized.tables() {
        st.read_lock(tx, &t)?;
    }
    for values in execute(super::explain::analyzing(tx, st, (physical, Some(&optimized))), st, tx.id) {
        rows.push(target.candidate(st, values?, &hosted)?)?;
    }
    Ok(rows)
}

/// The object rows of a searched UPDATE or DELETE, found before any is
/// changed [X3.135-1986 §8.5 GR1, §8.12 GR1], held up to MEMORY.WORK_BYTES, the
/// rest spilled (RULES.md D38).
pub(super) fn object_rows(st: &Arc<State>, tx: &Txn, w: Written) -> Result<(Written, HeldRows<(RowId, Row)>)> {
    let db = w.db(st);
    let Written { bound, rows, slots } = w;
    // T, to be written, and the tables its subqueries read.
    st.update_lock(tx, &bound.table)?;
    let (logical, kept) = match rows {
        ObjectRows::Bound => (Some(bound.rows.clone()), None),
        ObjectRows::Filled(logical) => (Some(logical), None),
        ObjectRows::Kept(plan, reads) => (None, Some((plan, reads))),
        ObjectRows::Point(kept, reads) => {
            let entries = pointed::object_row(st, tx, (&bound, &kept, &reads), (&db, slots.as_deref().unwrap_or_default()))?;
            return Ok((Written { bound, rows: ObjectRows::Bound, slots }, entries));
        }
    };
    let reads = match (&logical, &kept) {
        (_, Some((_, reads))) => reads.to_vec(),
        (Some(logical), None) => logical.tables(),
        (None, None) => Vec::new(),
    };
    // The tables its condition's and its SET values' subqueries read, the
    // object table among them perhaps [ISO/IEC 9075:1992 §13.7, §13.10,
    // Leveling Rules]. A SET value's subqueries read the same tables
    // filled or not.
    for t in reads.into_iter().chain(bound.set_tables()) {
        st.read_lock(tx, &t)?;
    }
    let (physical, optimized) = match (kept, logical) {
        (Some((physical, _)), _) => (physical, None),
        (None, logical) => {
            let (optimized, _, physical) = plan_written(st, logical.unwrap_or_else(|| bound.rows.clone()));
            (physical, Some(optimized))
        }
    };
    let entries = row_entries(super::explain::analyzing(tx, st, (physical, optimized.as_ref())), &db, tx.id)?;
    Ok((Written { bound, rows: ObjectRows::Bound, slots }, entries))
}

/// [X3.135-1986 §8.12] the searched UPDATE bound as `w`, the number of
/// rows it updated [ISO/IEC 9075:1992 §18.1 GR1e], none for no data
/// [X3.135-1986 §7.3 GR3a iv], what it did noted in `effects`, the
/// statement not yet ended. None if it found no row and no trigger may
/// fire, so nothing is to be checked.
pub(super) fn update_rows(cx: StatementContext<'_>, w: Written, effects: &mut Effects<'_>) -> Result<Option<usize>> {
    let StatementContext { st, tx } = cx;
    let (w, objects) = object_rows(st, tx, w)?;
    if objects.is_empty() && effects.triggered.is_none() {
        return Ok(None);
    }
    let columns = set_columns(&w);
    // [DESIGN] RULES.md D452: increments reserved, the rows left unwritten.
    let escrow = super::escrow::escrow_of(st, tx, &w, effects);
    let hosted = Hosted::in_db(effects.host, cx, &w.db(st));
    let runner = hosted.runner();
    let mut leftovers = portion::Leftovers::of(st, &w)?;
    let changes = super::assignment::changes_of(st, &w, objects, (effects, &hosted))?;
    // [ISO/IEC 9075-2:2023 §15.14 GR5] its BEFORE triggers first.
    let mut changes = effects.before_update(cx, (&w.bound.table, &columns), changes)?;
    // A row another transaction changed is written in the version it
    // committed, if it is still one to change, its values computed from it.
    let mut again: HashMap<RowId, Option<Row>> = HashMap::new();
    changes.visit(&mut |(id, seen, new)| {
        if let Some(guarded) = escrow
            && let Some(deltas) = super::escrow::deltas(&w, (seen, new), &hosted)?
            && st.reserve_row(tx, (&w.bound.table, *id), (seen, &deltas, guarded))?
        {
            return Ok(());
        }
        // [EXTENSION] a row its key moves to another partition (RULES.md D155).
        if st.moves((&w.bound.named, &w.bound.table), new) {
            return super::partitions::move_row(cx, (&w, runner), (*id, new), (effects, &mut again));
        }
        let mut recomputed = None;
        let mut recheck = |newest: &Row| {
            if !w.holds_of(newest, runner)? {
                return Ok(Again::Skip);
            }
            let row = candidate(&w, newest, ("ISO/IEC 9075:1992 §13.10 GR8", &hosted))?;
            recomputed = Some(row.clone());
            Ok(Again::Update(row))
        };
        match st.rewrite_row(tx, (&w.bound.table, *id), Some(new), &mut recheck)? {
            Some(old) => {
                leftovers.keep(st, &w, &old, &hosted)?;
                effects.updated(&w.bound.table, *id, (old, recomputed.as_ref().unwrap_or(new)), &columns)?;
                if let Some(row) = recomputed {
                    again.insert(*id, Some(row));
                }
            }
            None => {
                again.insert(*id, None);
            }
        }
        Ok(())
    })?;
    checked_again(&w, &mut changes, &again, runner)?;
    leftovers.insert(cx, &w, effects).map(|()| Some(changes.len().saturating_sub(again.values().filter(|r| r.is_none()).count())))
}

/// [X3.135-1986 §8.12 GR4] Of the UPDATE `w` through a view WITH CHECK
/// OPTION, that each row of `changes` it wrote, as `again` holds it if
/// written over another's commit, is one of the view's.
fn checked_again(
    w: &Written,
    changes: &mut HeldRows<(RowId, Row, Row)>,
    again: &HashMap<RowId, Option<Row>>,
    runner: &Runner,
) -> Result<()> {
    if w.bound.checks.is_empty() {
        return Ok(());
    }
    let checked = |row: &Row| check_option(&w.bound.checks, [row], ("X3.135-1986 §8.12 GR4", runner));
    changes.visit(&mut |(id, _, new)| match again.get(id) {
        Some(None) => Ok(()),
        Some(Some(row)) => checked(row),
        None => checked(new),
    })
}

/// [X3.135-1986 §8.5] the searched DELETE bound as `w`: the rows it
/// deleted [ISO/IEC 9075:1992 §18.1 GR1e], none for no data [X3.135-1986
/// §7.3 GR3a iv].
pub(super) fn delete_searched(cx: StatementContext<'_>, w: Written, mut effects: Effects<'_>) -> Result<usize> {
    match delete_rows(cx, w, &mut effects)? {
        Some(n) => {
            cx.st.end_statement(cx.tx, effects)?;
            Ok(n)
        }
        None => Ok(0),
    }
}

/// `delete_searched`'s rows deleted, as `update_rows` updates them, the
/// statement not yet ended.
pub(super) fn delete_rows(cx: StatementContext<'_>, w: Written, effects: &mut Effects<'_>) -> Result<Option<usize>> {
    let StatementContext { st, tx } = cx;
    let (w, mut objects) = object_rows(st, tx, w)?;
    if objects.is_empty() && effects.triggered.is_none() {
        return Ok(None);
    }
    // [ISO/IEC 9075-2:2023 §15.8 GR2] its BEFORE triggers first.
    effects.before_delete(cx, &w.bound.table, &mut objects)?;
    let runner = Runner::new(&w.db(st), tx.id);
    let mut leftovers = portion::Leftovers::of(st, &w)?;
    let mut count = 0_usize;
    for object in objects {
        let (id, _) = object?;
        // A row another transaction changed is deleted in the version it
        // committed, if it is still one to delete.
        let mut recheck = |newest: &Row| Ok(if w.holds_of(newest, &runner)? { Again::Delete } else { Again::Skip });
        if let Some(old) = st.rewrite_row(tx, (&w.bound.table, id), None, &mut recheck)? {
            leftovers.keep(st, &w, &old, &runner)?;
            effects.deleted(&w.bound.table, id, old)?;
            count = count.saturating_add(1);
        }
    }
    leftovers.insert(cx, &w, effects)?;
    Ok(Some(count))
}
