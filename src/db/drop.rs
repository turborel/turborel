//! \[EXTENSION\] DROP SCHEMA [ISO/IEC 9075:1992 §11.2], DROP TABLE [§11.18]
//! and DROP VIEW [§11.20], in a transaction (RULES.md D33): each staged as
//! one change (`Change::Drop`), made again on the definitions published
//! when it commits, and by recovery. Split from db/ddl.rs (RULES.md
//! R-SIZE-2).
//!
//! A drop takes the lock of each name it drops, or changes the
//! definitions of, exclusive: the object's; with CASCADE, those of the
//! views and of the tables whose constraints go with it; and a schema's
//! own (`QualifiedName::of_schema`), which a table or view defined in it
//! takes shared, so none is defined in a schema as it is dropped. It is
//! made on the definitions as they are once it holds them all.

use std::sync::Arc;

use super::ddl::{Changed, Dropped};
use super::{Session, State, rule};
use crate::ast::{DropBehavior, DropObject, TableName};
use crate::binder::qualify;
use crate::catalog::{ConstraintKind, QualifiedName, TemporaryKind, owner_of};
use crate::db::Completion;
use crate::error::{At, Code, Result, SqlError, SqlState, fail};
use crate::log::record::{Change, DropTarget};
use crate::settings::CATALOG_NAME;
use crate::txn::Txn;

impl State {
    /// Drops `object`, with what depends on it if `cascade`: what it took
    /// away.
    ///
    /// # Errors
    ///
    /// As `drop_table`, `drop_view` and `drop_schema` say.
    pub(super) fn drop_object(&mut self, object: &DropTarget, cascade: bool) -> Result<Dropped> {
        match object {
            DropTarget::Table(qn) => self.drop_table(qn, cascade),
            DropTarget::View(qn) => self.drop_view(qn, cascade),
            DropTarget::Schema(auth) => self.drop_schema(auth, cascade),
            DropTarget::Domain(qn) => self.drop_domain(qn, cascade),
            DropTarget::Assertion(qn) => self.drop_assertion(qn),
        }
    }

    /// [ISO/IEC 9075:1992 §11.18] Drops base table `qn`: with the views that
    /// read it, directly or through others, and the constraints of other
    /// tables that reference or read it, if `cascade`, and the privileges
    /// on all of them [GR1]; with its indexes [GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `qn` is not a base table [SR1, SR2], or, not
    /// `cascade`, a view or a constraint of another table refers to it
    /// [SR4].
    fn drop_table(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.table(qn).is_some_and(|t| t.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared)) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.18 SR3] {qn} is a declared local temporary table: it goes with the session")));
        }
        if self.catalog.table(qn).is_none() {
            let what = if self.catalog.view(qn).is_some() { "a view, not a base table [SR2]" } else { "not defined [SR1]" };
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.18] {qn} is {what}")));
        }
        let mut dropped = self.drop_dependents(qn, cascade, "§11.18 SR4")?;
        let mut catalog = self.catalog.duplicate();
        for ix in self.catalog.indexes(qn) {
            catalog.remove_index(&ix.name);
            dropped.indexes.extend(self.indexes.remove(&ix.name));
        }
        catalog.revoke_all(qn);
        catalog.remove_table(qn);
        let generation = self.generation_of(qn);
        self.retired.entry(qn.clone()).and_modify(|g| *g = (*g).max(generation)).or_insert(generation);
        self.catalog = Arc::from(catalog);
        dropped.storage.extend(self.storage.remove(qn));
        self.placement.remove(qn);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// [ISO/IEC 9075:1992 §11.20] Drops view `qn`, with the privileges on
    /// it [GR1, GR2]: with the views that read it, directly or through
    /// others, and the constraints that read it, if `cascade`, which the
    /// privileges revoked from its owner CASCADE take [GR1, §11.37].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `qn` is not a view [SR1, SR2], or, not `cascade`, a
    /// view or a constraint reads it [SR3].
    pub(super) fn drop_view(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.view(qn).is_none() {
            let what = if self.catalog.table(qn).is_some() { "a base table, not a view [SR2]" } else { "not defined [SR1]" };
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.20] {qn} is {what}")));
        }
        let mut dropped = self.drop_dependents(qn, cascade, "§11.20 SR3")?;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_view(qn);
        catalog.revoke_all(qn);
        self.catalog = Arc::from(catalog);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// [ISO/IEC 9075:1992 §11.2] Drops schema `auth`: each of its tables,
    /// then each of its views left, as DROP TABLE and DROP VIEW with
    /// CASCADE do [GR1, GR2], then its domains, as DROP DOMAIN with CASCADE
    /// does [GR3], then its assertions [GR4], then the schema [GR8].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such schema [SR2] (SQLSTATE 3F000), or,
    /// not `cascade`, it holds a table or view [SR3].
    fn drop_schema(&mut self, auth: &str, cascade: bool) -> Result<Dropped> {
        if !self.catalog.schema_exists(auth) {
            let msg = format!("[ISO/IEC 9075:1992 §11.2 SR2] there is no schema {auth}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let tables: Vec<QualifiedName> =
            self.catalog.tables().into_iter().filter(|t| t.name.auth == auth).map(|t| t.name.clone()).collect();
        let views: Vec<QualifiedName> = self.catalog.views().into_iter().filter(|v| v.name.auth == auth).map(|v| v.name.clone()).collect();
        let domains: Vec<QualifiedName> =
            self.catalog.domains().into_iter().filter(|d| d.name.auth == auth).map(|d| d.name.clone()).collect();
        let assertions: Vec<QualifiedName> =
            self.catalog.assertions().into_iter().filter(|a| a.name.auth == auth).map(|a| a.name.clone()).collect();
        let held = tables.first().or(views.first()).or(domains.first()).or(assertions.first());
        if let (Some(first), false) = (held, cascade) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.2 SR3] schema {auth} holds {first}, so it is dropped only with CASCADE")));
        }
        let mut dropped = Dropped::default();
        for table in &tables {
            dropped.absorb(self.drop_table(table, true)?);
        }
        // Those a table's drop took are gone already.
        for view in views.iter().filter(|v| self.catalog.view(v).is_some()).cloned().collect::<Vec<_>>() {
            dropped.absorb(self.drop_view(&view, true)?);
        }
        // [GR3] its domains.
        for domain in &domains {
            dropped.absorb(self.drop_domain(domain, true)?);
        }
        // [GR4] its assertions left.
        for assertion in assertions.iter().filter(|a| self.catalog.assertion(a).is_some()).cloned().collect::<Vec<_>>() {
            dropped.absorb(self.drop_assertion(&assertion)?);
        }
        let mut catalog = self.catalog.duplicate();
        catalog.remove_schema(auth);
        self.catalog = Arc::from(catalog);
        dropped.names.push(QualifiedName::of_schema(auth));
        Ok(dropped)
    }

    /// What depends on `qn`, a table or view dropped, dropped with it if
    /// `cascade`: the views that read it, directly or through others, and
    /// the constraints of other tables that reference or read it or them,
    /// with the privileges on the views. The names of the views and of
    /// the tables changed are among what it took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if anything depends on `qn`, as the
    /// Syntax Rule `cited` says.
    fn drop_dependents(&mut self, qn: &QualifiedName, cascade: bool, cited: &str) -> Result<Dropped> {
        let views = self.views_reading(qn);
        if let (Some(view), false) = (views.first(), cascade) {
            return Err(rule(format!("[ISO/IEC 9075:1992 {cited}] {qn} is read by view {view}, so it is dropped only with CASCADE")));
        }
        let gone: Vec<&QualifiedName> = views.iter().chain([qn]).collect();
        let refers = |kind: &ConstraintKind| match kind {
            ConstraintKind::ForeignKey(fk) => gone.contains(&&fk.table),
            ConstraintKind::Check { reads, .. } => reads.iter().any(|r| gone.contains(&r)),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } => false,
        };
        let mut catalog = self.catalog.duplicate();
        let mut dropped = Dropped::default();
        for other in self.catalog.tables().into_iter().filter(|t| t.name != *qn) {
            let Some(rc) = other.constraints.iter().find(|c| refers(&c.kind)) else { continue };
            if !cascade {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 {cited}] constraint {} of {} refers to {qn}, so it is dropped only with CASCADE",
                    rc.name, other.name
                )));
            }
            let mut kept = other.clone();
            kept.constraints.retain(|c| !refers(&c.kind));
            kept.refresh_not_null();
            catalog.replace_table(kept);
            dropped.names.push(other.name.clone());
        }
        for view in &views {
            catalog.remove_view(view);
            catalog.revoke_all(view);
        }
        self.catalog = Arc::from(catalog);
        dropped.names.extend(self.drop_asserting(&gone, (cascade, cited))?);
        dropped.names.extend(views);
        Ok(dropped)
    }

    /// The views that read `table`, directly or through other views.
    pub(super) fn views_reading(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        let mut gone = vec![table.clone()];
        let mut views = Vec::new();
        loop {
            let next: Vec<QualifiedName> = self
                .catalog
                .views()
                .into_iter()
                .filter(|v| !views.contains(&v.name) && v.reads.iter().any(|r| gone.contains(r)))
                .map(|v| v.name.clone())
                .collect();
            if next.is_empty() {
                return views;
            }
            gone.extend(next.iter().cloned());
            views.extend(next);
        }
    }
}

impl Session {
    /// [ISO/IEC 9075:1992 §11.2, §11.18, §11.20] DROP SCHEMA, TABLE or
    /// VIEW, in transaction `tx`: made once the locks of all it drops or
    /// changes are held, on the definitions then.
    pub(super) fn drop_object(&self, st: &Arc<State>, tx: &Txn, object: &DropObject, behavior: DropBehavior) -> Result<Completion> {
        let target = self.drop_target(st, object)?;
        let cascade = behavior == DropBehavior::Cascade;
        let mut held: Vec<QualifiedName> = Vec::new();
        loop {
            let fresh = self.definitions(self.db.snapshot()?)?;
            let mut next = State::clone(&fresh);
            let dropped = next.drop_object(&target, cascade)?;
            let mut wanted: Vec<QualifiedName> = dropped.names.into_iter().filter(|n| !held.contains(n)).collect();
            wanted.sort();
            wanted.dedup();
            if wanted.is_empty() {
                let change = Change::Drop { object: target, cascade };
                self.stage(&fresh, tx, Changed { next: Arc::new(next), change, made: None })?;
                return Ok(Completion::Success);
            }
            for name in wanted {
                self.prepare_change(&fresh, tx, &name)?;
                held.push(name);
            }
        }
    }

    /// What `object` names, its names qualified by the default schema, if
    /// this session's authorization identifier owns its schema.
    ///
    /// # Errors
    ///
    /// SQLCODE -551 if it does not [§11.2 AR1, §11.18 AR1, §11.20 AR1];
    /// -103 if a schema is named in another catalog [§11.2 SR2].
    fn drop_target(&self, st: &State, object: &DropObject) -> Result<DropTarget> {
        match object {
            DropObject::Table(table) => Ok(DropTarget::Table(self.owned(st, table, "§11.18 AR1")?)),
            DropObject::View(view) => Ok(DropTarget::View(self.owned(st, view, "§11.20 AR1")?)),
            DropObject::Domain(domain) => Ok(DropTarget::Domain(self.owned(st, domain, "§11.27 AR1")?)),
            DropObject::Assertion(assertion) => Ok(DropTarget::Assertion(self.owned(st, assertion, "§11.35 AR1")?)),
            DropObject::Schema { catalog, name, at } => {
                let ours = st.settings.get(CATALOG_NAME).to_string();
                if let Some(other) = catalog.as_ref().filter(|c| **c != ours.trim_end()) {
                    let msg = format!("[ISO/IEC 9075:1992 §11.2 SR2] the one catalog is {}, not {other}", ours.trim_end());
                    return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(*at));
                }
                if !self.owns_schema(st, name) {
                    let msg = format!(
                        "[ISO/IEC 9075:1992 §11.2 AR1] schema {name} is owned by {}, not {}",
                        owner_of(&*st.catalog, name),
                        self.auth()
                    );
                    return Err(SqlError::new(Code::Privilege, msg).or_at(*at));
                }
                Ok(DropTarget::Schema(name.clone()))
            }
        }
    }

    /// `name`, qualified by the default schema, if this session's
    /// authorization identifier owns its schema, as the Access Rule
    /// `cited` says.
    fn owned(&self, st: &State, name: &TableName, cited: &str) -> Result<QualifiedName> {
        let qn = qualify(name, &self.default_schema());
        if !self.owns_schema(st, &qn.auth) {
            return fail(Code::Privilege, format!("[ISO/IEC 9075:1992 {cited}] {qn} is in a schema {} does not own", self.auth()))
                .or_at(name.at);
        }
        Ok(qn)
    }
}
