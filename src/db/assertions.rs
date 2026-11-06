//! \[EXTENSION\] Assertions [ISO/IEC 9075:1992 §4.10.4, §11.34, §11.35]: a
//! constraint of a schema, on no table, not satisfied if and only if its
//! search condition is false.
//!
//! An assertion is bound as the query `SELECT 1 FROM (VALUES (1)) AS
//! ASSERTED (ONE) WHERE NOT (condition)`, as its schema's owner, as a constraint
//! (`Purpose::Constraint`): REFERENCES on what it reads [AR2], no
//! authorization identifier [SR6], nothing possibly non-deterministic
//! [SR8]. A row of it is the assertion broken. It is checked at the end
//! of each statement that changes a table it reads, as the constraints of
//! a table are (`db/integrity.rs`), or at commit if deferred; and as it is
//! defined. Checked at a statement's end, it is checked from the rows the
//! statement changed where its form allows (`assertions/changes.rs`).

mod changes;
mod delta;
mod fold;
mod forms;
mod kept;
mod queried;
mod totals;

pub(super) use changes::Asserted;
pub(super) use fold::folded;

use std::sync::Arc;

use super::alter::Made;
use super::ddl::Dropped;
use super::definitions::Defined;
use super::{State, plan, rule};
use crate::ast::{AssertionDefinition, Schema};
use crate::binder::{self, Context, Purpose, Referencing, Uses, bind_cursor, qualify};
use crate::catalog::{ConstraintDesc, ConstraintKind, Horizon, QualifiedName, TableDesc, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState, Subject};
use crate::exec::execute;
use crate::logical::LogicalPlan;
use crate::params::Params;
use crate::parser::Parser;
use crate::txn::{LockMode, Txn};

/// [ISO/IEC 9075:1992 §11.34 GR2] Assertion `name` is false (SQLCODE -201,
/// SQLSTATE 23000), about it.
fn broken(name: &QualifiedName) -> SqlError {
    SqlError::new(Code::Constraint, format!("[ISO/IEC 9075:1992 §11.34 GR2] assertion {name} is false"))
        .about(Subject::assertion(name.parts()))
}

impl State {
    /// [ISO/IEC 9075:1992 §11.34] CREATE ASSERTION `a`, its names in schema
    /// `schema`: to be checked as its statement ends (`Defined::asserted`).
    /// If it keeps totals, their table is made, of what `made` holds for
    /// it, if given, and the tables it reads are held exclusive, so none
    /// changes uncounted before it is published (`assertions/kept.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks: no schema, a constraint of
    /// its name [SR2, SR7], a temporary table read [SR5], or a CHECK's
    /// rules [SR4, SR6, SR8]; -104 for REFERENCES its schema's owner lacks
    /// [AR2].
    pub(super) fn create_assertion(&mut self, schema: &str, a: &AssertionDefinition, made: Option<&Made>) -> Result<Defined> {
        let name = qualify(&a.name, schema);
        if !self.catalog.schema_exists(&name.auth) {
            let msg = format!("[ISO/IEC 9075:1992 §11.34 SR2] there is no schema {}", name.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(a.name.at));
        }
        if self.constraint_names(&name.auth).contains(&name) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.34 SR2, SR7] schema {} has a constraint {} already", name.auth, name.ident))
                .or_at(a.name.at));
        }
        let kind =
            ConstraintKind::Check { text: a.check.text.clone(), names: schema.to_owned(), reads: Vec::new(), horizon: Horizon::default() };
        let mut desc = ConstraintDesc { name: name.clone(), kind, attributes: a.attributes, domain: None };
        let plan = self.assertion_plan(&desc).or_at(a.name.at)?;
        let reads = plan.tables();
        // The columns it sees of each: those of now, as a CHECK's.
        let seen = Horizon::of(&*self.catalog, &reads);
        // [SR5] no temporary table.
        if let Some(t) = reads.iter().find(|t| self.catalog.table(t).is_some_and(|d| d.temporary.is_some())) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.34 SR5] assertion {name} reads {t}, a temporary table")));
        }
        if let ConstraintKind::Check { reads: kept, horizon, .. } = &mut desc.kind {
            kept.clone_from(&reads);
            *horizon = seen;
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_assertion(desc);
        let built = self.make_kept(&mut *catalog, (&name, &plan), made)?;
        self.catalog = Arc::from(catalog);
        let held = if built.is_some() { LockMode::Exclusive } else { LockMode::Shared };
        let locks = reads.into_iter().map(|t| (t, held)).collect();
        Ok(Defined { locks, asserted: Some(name), made: built, ..Defined::default() })
    }

    /// The assertions of `<schema>` `s`, made once its tables and views
    /// are, each of what `made` holds for its kept totals' table, if given,
    /// what they make added to `built`: their names.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is named in another schema [§11.34 SR1], or as
    /// `create_assertion` says.
    pub(super) fn schema_assertions(&mut self, s: &Schema, (made, built): (Option<&Made>, &mut Made)) -> Result<Vec<QualifiedName>> {
        let mut names = Vec::new();
        for a in &s.assertions {
            if let Some(other) = a.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!("[ISO/IEC 9075:1992 §11.34 SR1] assertion {} is in schema {other}, not {}", a.name.ident, s.name);
                return Err(rule(msg).or_at(a.name.at));
            }
            let name = qualify(&a.name, &s.name);
            let kept = kept::kept_name(&name);
            if let Some(table) = self.create_assertion(&s.name, a, made.and_then(|m| m.of_table(&kept)))?.made {
                built.tables.push((kept, table));
            }
            names.push(name);
        }
        Ok(names)
    }

    /// The query whose rows break assertion `a`, bound.
    ///
    /// # Errors
    ///
    /// Its condition does not parse, or breaks a rule of a CHECK's.
    pub(super) fn assertion_plan(&self, a: &ConstraintDesc) -> Result<LogicalPlan> {
        self.bind_assertion(a, None)
    }

    /// [ISO/IEC 9075:1992 §21.3.18] The columns assertion `a` references,
    /// once each.
    ///
    /// # Errors
    ///
    /// It does not bind, as `assertion_plan`.
    pub(crate) fn assertion_uses(&self, a: &ConstraintDesc) -> Result<Vec<(QualifiedName, String)>> {
        let uses = Uses::default();
        self.bind_assertion(a, Some(&uses))?;
        Ok(binder::once_each(uses.into_inner()))
    }

    /// Assertion `a`, bound as `assertion_plan` says, its column references
    /// noted in `uses`, if given.
    fn bind_assertion(&self, a: &ConstraintDesc, uses: Option<&Uses>) -> Result<LogicalPlan> {
        let ConstraintKind::Check { text, names, horizon, .. } = &a.kind else {
            return Err(rule(format!("[DESIGN] assertion {} is no CHECK", a.name)));
        };
        let sql = format!("SELECT 1 FROM (VALUES (1)) AS ASSERTED (ONE) WHERE NOT ({text})");
        let spec = Parser::new(&sql)?.of_definition().cursor_specification()?;
        let params = Params::new();
        let owner = owner_of(&*self.catalog, &a.name.auth);
        let referencing = Referencing::default();
        let cx = Context {
            purpose: Purpose::Constraint,
            horizon,
            uses,
            schema: names,
            referencing: Some(&referencing),
            ..Context::new(self, owner, &params)
        };
        // A position in the query made of its text is none of a statement's.
        let plan = bind_cursor(cx, &spec).map_err(|e| SqlError { pos: None, ..e })?.plan;
        referencing.check(owner)?;
        Ok(plan)
    }

    /// [ISO/IEC 9075:1992 §11.34 GR2] That assertion `name` is not false,
    /// as transaction `tx` sees what it reads.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is.
    pub(super) fn check_assertion(self: &Arc<Self>, tx: &Txn, name: &QualifiedName) -> Result<()> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(()) };
        let ConstraintKind::Check { reads, .. } = &a.kind else { return Ok(()) };
        for t in reads {
            self.read_lock(tx, t)?;
        }
        self.assertion_holds(tx, name, self.assertion_plan(a)?)
    }

    /// That assertion `name`, whose query is `bound`, is not false, its
    /// query run whole, its reads locked.
    ///
    /// # Errors
    ///
    /// As `check_assertion`.
    fn assertion_holds(self: &Arc<Self>, tx: &Txn, name: &QualifiedName, bound: LogicalPlan) -> Result<()> {
        let (_, _, physical) = plan(self, bound);
        if execute(physical, self, tx.id).next().transpose()?.is_some() {
            return Err(broken(name));
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.1, §11.34 GR2] That the assertions `names`
    /// of a `<schema definition>` just made are not false: outside any
    /// session's transaction, in one of their own that reads the newest
    /// committed rows, which writes the totals they keep (`kept.rs`): it,
    /// to be committed once the schema is logged, so recovery makes their
    /// tables before it reads their rows.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the first that is; the totals'
    /// reads' and writes'.
    pub(super) fn check_new_assertions(&self, names: &[QualifiedName]) -> Result<Option<(Arc<State>, Txn)>> {
        if names.is_empty() {
            return Ok(None);
        }
        let probe = Arc::new(self.clone());
        let tx = probe.begin();
        let checked = names.iter().try_for_each(|n| probe.check_assertion(&tx, n).and_then(|()| probe.start_totals(&tx, n)));
        checked.or_else(|e| probe.rollback(&tx).and(Err(e)))?;
        Ok(Some((probe, tx)))
    }

    /// [ISO/IEC 9075:1992 §11.35] Drops assertion `qn` [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such assertion [SR2].
    pub(super) fn drop_assertion(&mut self, qn: &QualifiedName) -> Result<Dropped> {
        if self.catalog.assertion(qn).is_none() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.35 SR2] there is no assertion {qn}")));
        }
        let mut catalog = self.catalog.duplicate();
        catalog.remove_assertion(qn);
        let mut dropped = Dropped { names: vec![qn.clone()], ..Dropped::default() };
        self.drop_kept(&mut *catalog, qn, &mut dropped);
        self.catalog = Arc::from(catalog);
        Ok(dropped)
    }

    /// The assertions that read any of `gone`, dropped with them if
    /// `cascade` [§11.37 GR5], with the tables of their kept totals: what
    /// was taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one does, as the Syntax Rule `cited`
    /// says [§11.18 SR4, §11.20 SR3].
    pub(super) fn drop_asserting(&mut self, gone: &[&QualifiedName], (cascade, cited): (bool, &str)) -> Result<Dropped> {
        let reading = |a: &ConstraintDesc| {
            let ConstraintKind::Check { reads, .. } = &a.kind else { return false };
            reads.iter().any(|r| gone.contains(&r))
        };
        let asserting: Vec<QualifiedName> = self.catalog.assertions().into_iter().filter(|a| reading(a)).map(|a| a.name.clone()).collect();
        if let (Some(a), false) = (asserting.first(), cascade) {
            return Err(rule(format!("[ISO/IEC 9075:1992 {cited}] assertion {a} reads it, so it is dropped only with CASCADE")));
        }
        let (mut catalog, mut dropped) = (self.catalog.duplicate(), Dropped::default());
        for a in &asserting {
            catalog.remove_assertion(a);
            self.drop_kept(&mut *catalog, a, &mut dropped);
        }
        self.catalog = Arc::from(catalog);
        dropped.names.extend(asserting);
        Ok(dropped)
    }

    /// [ISO/IEC 9075:1992 §11.15 SR4] The assertions that refer to the
    /// column `at` of `desc`, a column dropped: those that bind no more
    /// without it, dropped if `cascade` [§11.37 GR5], with the tables of
    /// their kept totals, what was taken away; the others that read the
    /// table seeing one column fewer of it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one refers to it.
    pub(super) fn drop_column_asserting(&mut self, desc: &TableDesc, at: usize, cascade: bool) -> Result<Dropped> {
        let mut next = desc.clone();
        next.columns.remove(at);
        let without = self.without_column(&next);
        let (mut kept, mut broken) = (Vec::new(), Vec::new());
        for a in self.catalog.assertions() {
            let ConstraintKind::Check { text, names, reads, horizon } = &a.kind else { continue };
            if !reads.contains(&desc.name) {
                continue;
            }
            let seen = horizon.without(&desc.name, at);
            let kind = ConstraintKind::Check { text: text.clone(), names: names.clone(), reads: reads.clone(), horizon: seen };
            let narrowed = ConstraintDesc { kind, ..a.clone() };
            if without.assertion_plan(&narrowed).is_ok() {
                kept.push(narrowed);
            } else if cascade {
                broken.push(a.name.clone());
            } else {
                let column = desc.columns.get(at).map_or("?", |c| c.name.as_str());
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §11.15 SR4] assertion {} refers to {}.{column}, so it is dropped only with CASCADE",
                    a.name, desc.name
                )));
            }
        }
        let (mut catalog, mut dropped) = (self.catalog.duplicate(), Dropped::default());
        for a in &broken {
            catalog.remove_assertion(a);
            self.drop_kept(&mut *catalog, a, &mut dropped);
        }
        for a in kept {
            catalog.put_assertion(a);
        }
        self.catalog = Arc::from(catalog);
        dropped.names.extend(broken);
        Ok(dropped)
    }
}
