//! [ISO/IEC 9075-2:2023 §11.26 SR4 to SR7, SR13, GR1, GR3, GR4] What
//! depends on a table constraint dropped. A query specification of a
//! grouped query depends on it if the constraint is needed to know that
//! the grouping columns determine a column its select list reads (SR4).
//! A key, or the NOT NULL that makes a UNIQUE's columns one, is such a
//! constraint (T301, §4.26.3). So do the views, table and domain
//! constraints and assertions that contain one (SR5, SR7).
//!
//! Each is found as the triggers' are (`db/triggers/dependents.rs`), by
//! binding it again once the constraint is gone. One that bound before
//! and binds no more depends on it, for nothing else changed. One that
//! bound neither way, as what a REVOKE abandons as it drops the
//! constraint, is left to what drops it. Under RESTRICT a dependent
//! refuses the drop (SR13a, SR13c, SR13e). Under CASCADE each is dropped
//! as the General Rules say, with what depends on it in turn.

use super::super::ddl::Dropped;
use super::rule;
use crate::ast::{AlterDomain, AlterDomainAction, DropBehavior, TableName};
use crate::binder::view_degree;
use crate::catalog::{ConstraintKind, QualifiedName, ViewDesc};
use crate::db::State;
use crate::error::Result;

/// The name `q` as written in full.
fn written(q: &QualifiedName) -> TableName {
    TableName { auth: Some(q.auth.clone()), ident: q.ident.clone(), at: 0 }
}

/// The refusal of a drop under RESTRICT, by the Syntax Rule `cited`, of
/// the constraint `what` depends on.
fn depends(cited: &str, what: &str) -> crate::error::SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §11.26 {cited}] {what} depends on the constraint, so it is dropped only with CASCADE"))
}

impl State {
    /// [§11.26 SR13a, SR13c, SR13e, GR1, GR3, GR4] What depends on a
    /// constraint of table `qn`, the constraint dropped already, the
    /// definitions `before` it was: the CHECKs, domain constraints, views
    /// and assertions that read `qn`, bound in `before` and bind no more,
    /// dropped if `cascade`. What was taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if one depends on it.
    pub(in crate::db) fn drop_constraint_dependents(&mut self, before: &State, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let mut dropped = self.drop_dependent_checks(before, qn, cascade)?;
        dropped.absorb(self.drop_dependent_domain_constraints(before, qn, cascade)?);
        dropped.absorb(self.drop_dependent_views(before, qn, cascade)?);
        dropped.absorb(self.drop_dependent_assertions(before, qn, cascade)?);
        Ok(dropped)
    }

    /// [SR7, SR13a, GR1] The CHECKs of tables, not copies of a domain's,
    /// that read `qn` and bind no more, each dropped if `cascade` as
    /// ALTER TABLE … DROP CONSTRAINT … CASCADE drops it.
    fn drop_dependent_checks(&mut self, before: &State, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let mut found = Vec::new();
        for t in self.catalog.tables() {
            for c in &t.constraints {
                let ConstraintKind::Check { text, names, reads, horizon } = &c.kind else { continue };
                let binds = |st: &State| st.check_condition(&t.name, (text, names, horizon)).is_ok();
                if c.domain.is_none() && reads.contains(qn) && binds(before) && !binds(self) {
                    found.push((t.name.clone(), c.name.clone()));
                }
            }
        }
        if let (Some((table, c)), false) = (found.first(), cascade) {
            return Err(depends("SR13a", &format!("constraint {c} of {table}")));
        }
        let mut dropped = Dropped::default();
        for (table, c) in &found {
            // One dropped already with another is not dropped again.
            if self.catalog.table(table).is_some_and(|t| t.constraints.iter().any(|k| k.name == *c)) {
                dropped.absorb(self.drop_constraint((table, &table.auth), &written(c), DropBehavior::Cascade)?);
                dropped.names.push(table.clone());
            }
        }
        Ok(dropped)
    }

    /// [SR7, SR13e] The constraints of domains that read `qn` and bind no
    /// more, each dropped if `cascade` as ALTER DOMAIN … DROP CONSTRAINT
    /// drops it, with its copies [§11.39], as NOTE 568 says.
    fn drop_dependent_domain_constraints(&mut self, before: &State, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let mut found = Vec::new();
        for d in self.catalog.domains() {
            for c in &d.constraints {
                let ConstraintKind::Check { text, names, reads, horizon } = &c.kind else { continue };
                let binds = |st: &State| st.bound_on_value(&d.name, d.data_type.clone(), (text, names, horizon)).is_ok();
                if reads.contains(qn) && binds(before) && !binds(self) {
                    found.push((d.name.clone(), c.name.clone()));
                }
            }
        }
        if let (Some((domain, c)), false) = (found.first(), cascade) {
            return Err(depends("SR13e", &format!("constraint {c} of domain {domain}")));
        }
        let mut dropped = Dropped::default();
        for (domain, c) in &found {
            let statement = AlterDomain { name: written(domain), action: AlterDomainAction::DropConstraint(written(c)) };
            dropped.names.extend(self.alter_domain((&domain.auth, 0), &statement)?.locks.into_iter().map(|(name, _)| name));
        }
        Ok(dropped)
    }

    /// [SR5, SR13c, GR3] The views that read `qn`, directly or through
    /// others, and bind no more, each dropped if `cascade` as DROP VIEW …
    /// CASCADE drops it.
    fn drop_dependent_views(&mut self, before: &State, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let binds = |st: &State, v: &ViewDesc| view_degree(st, v).is_ok();
        let broken: Vec<QualifiedName> = self
            .views_reading(qn)
            .into_iter()
            .filter(|v| self.catalog.view(v).is_some_and(|v| binds(before, v) && !binds(self, v)))
            .collect();
        if let (Some(view), false) = (broken.first(), cascade) {
            return Err(depends("SR13c", &format!("view {view}")));
        }
        let mut dropped = Dropped::default();
        for view in &broken {
            // One that read another dropped went with it.
            if self.catalog.view(view).is_some() {
                dropped.absorb(self.drop_view(view, true)?);
            }
        }
        Ok(dropped)
    }

    /// [SR7, SR13e, GR4] The assertions that read `qn` and bind no more,
    /// each dropped if `cascade` as DROP ASSERTION … CASCADE drops it.
    fn drop_dependent_assertions(&mut self, before: &State, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let reading = |kind: &ConstraintKind| if let ConstraintKind::Check { reads, .. } = kind { reads.contains(qn) } else { false };
        let broken: Vec<QualifiedName> = (self.catalog.assertions().into_iter())
            .filter(|a| reading(&a.kind) && before.assertion_plan(a).is_ok() && self.assertion_plan(a).is_err())
            .map(|a| a.name.clone())
            .collect();
        if let (Some(a), false) = (broken.first(), cascade) {
            return Err(depends("SR13e", &format!("assertion {a}")));
        }
        let mut dropped = Dropped::default();
        for a in &broken {
            dropped.absorb(self.drop_assertion(a)?);
        }
        Ok(dropped)
    }
}
