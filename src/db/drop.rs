//! \[EXTENSION\] DROP SCHEMA [ISO/IEC 9075:1992 §11.2], DROP TABLE [§11.18],
//! DROP VIEW [§11.20], DROP DOMAIN [§11.27] (`db/domains.rs`), DROP
//! ASSERTION [§11.35] (`db/assertions.rs`), and DROP COLLATION, DROP
//! SEQUENCE and DROP TYPE [ISO/IEC 9075-2:2023 §11.44, §11.74, §11.59]
//! (`db/user_types.rs`), and DROP XML SCHEMA (`definitions/xml_schemas.rs`),
//! in a transaction (RULES.md D33):
//! each staged as one change (`Change::Drop`), made again on the definitions
//! published when it commits, and by recovery.
//!
//! A drop takes the lock of each name it drops, or changes the
//! definitions of, exclusive: the object's; with CASCADE, those of the
//! views and of the tables whose constraints go with it; and a schema's
//! own (`QualifiedName::of_schema`), which a table or view defined in it
//! takes shared, so none is defined in a schema as it is dropped. It is
//! made on the definitions as they are once it holds them all.
//!
//! \[EXTENSION\] A schema goes with the views, routines and triggers that
//! map it by XMLMAP (`drop/mapped.rs`).

mod mapped;

use std::sync::Arc;

use super::ddl::{Changed, Dropped};
use super::{Session, State, rule};
use crate::ast::{AlterDomain, AlterDomainAction, DropBehavior, DropObject, TableName};
use crate::binder::qualify;
use crate::catalog::{ConstraintDesc, ConstraintKind, QualifiedName, TemporaryKind, owner_of};
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
            DropTarget::Collation(qn) => self.drop_collation(qn, cascade),
            DropTarget::Sequence(qn) => self.drop_sequence(qn, cascade),
            DropTarget::Type(qn) => self.drop_user_type(qn, cascade),
            DropTarget::Trigger(qn) => self.drop_trigger(qn),
            DropTarget::Routine(specific) => self.drop_routine(specific, cascade),
            DropTarget::Module(qn) => self.drop_module(qn, cascade),
            DropTarget::Cast(types) => self.drop_cast(&types.0, &types.1, cascade),
            DropTarget::Ordering(qn) => self.drop_ordering(qn, cascade),
            DropTarget::Transform(qn, group) => self.drop_transform(qn, group.as_deref(), cascade),
            DropTarget::Model(qn) => self.drop_model(qn, cascade),
            DropTarget::Pipeline(qn) => self.drop_pipeline(qn),
            DropTarget::XmlSchema(qn) => self.drop_xml_schema(qn, cascade),
        }
    }

    /// [ISO/IEC 9075:1992 §11.18] Drops base table `qn`: with the views that
    /// read it, directly or through others, and the constraints of other
    /// tables that reference or read it, if `cascade`, and the privileges
    /// on all of them [GR1]; with its indexes [GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `qn` is not a base table [SR1, SR2] (a declared
    /// local temporary table is refused before [SR3], `drop_target`), or, not
    /// `cascade`, a view or a constraint of another table refers to it
    /// [SR4].
    pub(super) fn drop_table(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.table(qn).is_none() {
            let (what, clause) = if self.catalog.view(qn).is_some() { ("a view, not a base table", "SR2") } else { ("not defined", "SR1") };
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.18 {clause}] {qn} is {what}")));
        }
        // [ISO/IEC 9075-2:2023 §11.31 SR6, GR1] its subtables first.
        let mut dropped = self.drop_pipelines_of(qn, cascade)?;
        dropped.absorb(self.drop_subtables(qn, cascade)?);
        dropped.absorb(self.drop_dependents(qn, cascade, "§11.18 SR4")?);
        let mut catalog = self.catalog.duplicate();
        for ix in self.catalog.indexes(qn) {
            catalog.remove_index(&ix.name);
            dropped.indexes.extend(self.indexes.remove(&ix.name));
        }
        catalog.revoke_all(qn);
        catalog.remove_table(qn);
        self.drop_history(&mut *catalog, qn, &mut dropped);
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
            let (what, clause) =
                if self.catalog.table(qn).is_some() { ("a base table, not a view", "SR2") } else { ("not defined", "SR1") };
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.20 {clause}] {qn} is {what}")));
        }
        // [ISO/IEC 9075-2:2023 §11.33 SR4, GR1] its subviews first.
        let mut dropped = self.drop_subviews(qn, cascade)?;
        dropped.absorb(self.drop_dependents(qn, cascade, "§11.20 SR3")?);
        let mut catalog = self.catalog.duplicate();
        catalog.remove_view(qn);
        catalog.revoke_all(qn);
        self.catalog = Arc::from(catalog);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// [ISO/IEC 9075-4:2006 draft §9.2 GR1, GR3] The modules `modules` of a
    /// schema dropped, and those whose descriptors name it, each as DROP
    /// MODULE ... CASCADE drops it, but those a drop before took.
    ///
    /// # Errors
    ///
    /// As each drop's.
    fn drop_modules_naming(&mut self, modules: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for m in modules {
            if self.catalog.module(m).is_some() {
                dropped.absorb(self.drop_module(m, true)?);
            }
        }
        Ok(dropped)
    }

    /// The registered XML Schemas, collations and sequence generators of
    /// schema `auth`, which nothing of it left depends on, dropped.
    ///
    /// # Errors
    ///
    /// As each kind's drop.
    fn drop_schema_described(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = self.drop_schema_xml_schemas(auth)?;
        dropped.absorb(self.drop_schema_collations(auth)?);
        dropped.absorb(self.drop_schema_sequences(auth)?);
        Ok(dropped)
    }

    /// [ISO/IEC 9075:1992 §11.2] Drops schema `auth`: each of its tables,
    /// then each of its views left, as DROP TABLE and DROP VIEW with
    /// CASCADE do [GR1, GR2], then its domains, as DROP DOMAIN with CASCADE
    /// does [GR3], then its assertions [GR4], then its triggers [ISO/IEC
    /// 9075-2:2023 §11.2 GR9], its routines, \[EXTENSION\] its registered
    /// XML Schemas, as DROP XML SCHEMA with CASCADE does, and the rest of
    /// what it holds, then the schema [GR8].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such schema [SR2] (SQLSTATE 3F000), or,
    /// not `cascade`, it holds a table, view, domain, assertion, collation,
    /// sequence generator, user-defined type [SR3], trigger or SQL-invoked
    /// routine [ISO/IEC 9075-2:2023 §11.2 SR4j, SR4l], or \[EXTENSION\] a
    /// registered XML Schema.
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
        let collations = self.schema_collations(auth);
        let sequences = self.schema_sequences(auth);
        let types = self.schema_user_types(auth);
        let (triggers, routines) = (self.schema_triggers_of(auth), self.schema_routines_of(auth));
        let (modules, registered) = (self.modules_of_schema(auth), self.schema_xml_schemas(auth));
        // [ISO/IEC 9075-4:2006 draft §9.2 SR1] a module of it named first.
        let held =
            (modules.iter().find(|m| m.auth == auth).or(tables.first()).or(views.first()).or(domains.first()).or(assertions.first()))
                .or(collations.first().or(sequences.first()).or(types.first()))
                .or(triggers.first().or(routines.first()).or(registered.first()));
        if let (Some(first), false) = (held.cloned().or_else(|| self.schema_embeddings(auth).into_iter().next()), cascade) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.2 SR3] schema {auth} holds {first}, so it is dropped only with CASCADE")));
        }
        let mut dropped = self.drop_schema_embeddings(auth)?;
        dropped.absorb(self.drop_named_in(auth, cascade)?);
        dropped.absorb(self.drop_mappers(auth, cascade)?);
        dropped.absorb(self.drop_modules_naming(&modules)?);
        // A subtable goes with its supertable, maybe before its turn.
        for table in &tables {
            if self.catalog.table(table).is_some() {
                dropped.absorb(self.drop_table(table, true)?);
            }
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
        // [ISO/IEC 9075-2:2023 §11.2 GR9] its triggers left.
        dropped.absorb(self.drop_triggers(&triggers)?);
        // [ISO/IEC 9075-2:2023 §11.2 GR10] its routines left.
        dropped.absorb(self.drop_routines(&routines)?);
        dropped.absorb(self.drop_schema_described(auth)?);
        // Its types, once the tables of them are gone.
        dropped.absorb(self.drop_schema_user_types(auth)?);
        let mut catalog = self.catalog.duplicate();
        catalog.remove_schema(auth);
        self.catalog = Arc::from(catalog);
        dropped.names.push(QualifiedName::of_schema(auth));
        Ok(dropped)
    }

    /// \[EXTENSION\] (RULES.md D47) The constraints named in schema `auth`
    /// of tables and domains of other schemas, dropped with it if
    /// `cascade`, as ALTER TABLE and ALTER DOMAIN DROP CONSTRAINT drop them,
    /// a domain constraint's copies with it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if there is one.
    fn drop_named_in(&mut self, auth: &str, cascade: bool) -> Result<Dropped> {
        let named = |c: &&ConstraintDesc| c.name.auth == auth && c.domain.is_none();
        let of_tables: Vec<(QualifiedName, QualifiedName)> = (self.catalog.tables().into_iter())
            .filter(|t| t.name.auth != auth)
            .flat_map(|t| t.constraints.iter().filter(named).map(|c| (t.name.clone(), c.name.clone())))
            .collect();
        let of_domains: Vec<(QualifiedName, QualifiedName)> = (self.catalog.domains().into_iter())
            .filter(|d| d.name.auth != auth)
            .flat_map(|d| d.constraints.iter().filter(named).map(|c| (d.name.clone(), c.name.clone())))
            .collect();
        if let (Some((of, c)), false) = (of_tables.first().or(of_domains.first()), cascade) {
            return Err(rule(format!("[EXTENSION] schema {auth} names constraint {c} of {of}, so it is dropped only with CASCADE")));
        }
        let written = |q: &QualifiedName| TableName { auth: Some(q.auth.clone()), ident: q.ident.clone(), at: 0 };
        let mut dropped = Dropped::default();
        for (table, c) in &of_tables {
            dropped.absorb(self.drop_constraint((table, &table.auth), &written(c), DropBehavior::Cascade)?);
            dropped.names.push(table.clone());
        }
        for (domain, c) in &of_domains {
            let statement = AlterDomain { name: written(domain), action: AlterDomainAction::DropConstraint(written(c)) };
            dropped.names.extend(self.alter_domain((&domain.auth, 0), &statement)?.locks.into_iter().map(|(name, _)| name));
        }
        Ok(dropped)
    }

    /// What depends on `qn`, a table or view dropped, dropped with it if
    /// `cascade`: the views that read it, directly or through others, and
    /// the constraints of other tables that reference or read it or them,
    /// and the assertions that read it or them, with the privileges on the
    /// views; the triggers on them, and those whose actions name them. The
    /// names of the views, of the tables changed, of the assertions and of
    /// the triggers are among what it took away.
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
        dropped.absorb(self.drop_asserting(&gone, (cascade, cited))?);
        dropped.absorb(self.drop_triggering(&gone, (cascade, self.catalog.table(qn).is_some()))?);
        dropped.absorb(self.drop_routining(&gone, (cascade, self.catalog.table(qn).is_some()))?);
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
    /// [ISO/IEC 9075:1992 §11.2, §11.18, §11.20, §11.27, §11.35] DROP
    /// SCHEMA, TABLE, VIEW, DOMAIN, ASSERTION, COLLATION, SEQUENCE or TYPE,
    /// in transaction `tx`. With CASCADE, the generated columns that name a
    /// domain, collation or type it drops go first, and the columns of a
    /// type it drops.
    pub(super) fn drop_object(&self, st: &Arc<State>, tx: &Txn, object: &DropObject, behavior: DropBehavior) -> Result<Completion> {
        let target = self.drop_target(st, object)?;
        let cascade = behavior == DropBehavior::Cascade;
        // The generated columns that name what goes, with CASCADE, dropped
        // first (`db/generated/named.rs`).
        let lost = if cascade { st.generations_dropped(&target) } else { Vec::new() };
        self.dropping_generations(tx, &lost, || {
            let fresh = if lost.is_empty() { Arc::clone(st) } else { self.definitions(self.db.snapshot()?)? };
            self.drop_reverted(&fresh, tx, &target, cascade).or_at(object.at())
        })
    }

    /// DROP of `target`, with `cascade` or not, in transaction `tx`, on
    /// definitions `st`: the columns of a collation it drops reverted
    /// first, then made once the locks of all it drops or changes are
    /// held, on the definitions then.
    fn drop_reverted(&self, st: &Arc<State>, tx: &Txn, target: &DropTarget, cascade: bool) -> Result<Completion> {
        let reverting: Vec<(QualifiedName, Vec<QualifiedName>)> = match target {
            DropTarget::Collation(qn) => vec![(qn.clone(), st.tables_of_collation(qn, None))],
            // [§11.2 GR5] Its own tables go, those of other schemas revert.
            DropTarget::Schema(auth) => {
                st.schema_collations(auth).into_iter().map(|qn| (qn.clone(), st.tables_of_collation(&qn, Some(auth)))).collect()
            }
            DropTarget::Table(_)
            | DropTarget::View(_)
            | DropTarget::Domain(_)
            | DropTarget::Assertion(_)
            | DropTarget::Sequence(_)
            | DropTarget::Type(_)
            | DropTarget::Trigger(_)
            | DropTarget::Routine(_)
            | DropTarget::Module(_)
            | DropTarget::Cast(_)
            | DropTarget::Ordering(_)
            | DropTarget::XmlSchema(_)
            | DropTarget::Transform(..)
            | DropTarget::Model(_)
            | DropTarget::Pipeline(_) => Vec::new(),
        };
        if !reverting.is_empty() {
            // Refused before a row is copied: a statement undone takes
            // back its rows, not a table's new definition.
            State::clone(st).drop_object(target, cascade)?;
            for (qn, tables) in reverting {
                // No table of it defined meanwhile (`collations/revert.rs`).
                self.prepare_change(st, tx, &qn)?;
                self.revert_columns(tx, &qn, &tables)?;
            }
        }
        let mut held: Vec<QualifiedName> = Vec::new();
        loop {
            let fresh = self.definitions(self.db.snapshot()?)?;
            let mut next = State::clone(&fresh);
            let dropped = next.drop_object(target, cascade)?;
            let mut wanted: Vec<QualifiedName> = dropped.names.into_iter().filter(|n| !held.contains(n)).collect();
            wanted.sort();
            wanted.dedup();
            if wanted.is_empty() {
                let change = Change::Drop { object: target.clone(), cascade };
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
    /// SQLCODE -104 if it does not [§11.2 AR1, §11.18 AR1, §11.20 AR1,
    /// §11.27 AR1, §11.35 AR1]; -103 if a schema is named in another
    /// catalog [§11.2 SR2], or a table is a declared local temporary table
    /// [§11.18 SR3].
    fn drop_target(&self, st: &State, object: &DropObject) -> Result<DropTarget> {
        match object {
            DropObject::Table(table) => {
                let qn = self.owned(st, table, "ISO/IEC 9075:1992 §11.18 AR1")?;
                // [§11.18 SR3] of the session's definitions, which hold its
                // declared tables, as the published ones do not.
                if st.catalog.table(&qn).is_some_and(|t| t.temporary.is_some_and(|t| t.kind == TemporaryKind::Declared)) {
                    let msg = format!("[ISO/IEC 9075:1992 §11.18 SR3] {qn} is a declared local temporary table: it goes with the session");
                    return Err(rule(msg).or_at(table.at));
                }
                Ok(DropTarget::Table(qn))
            }
            DropObject::View(view) => Ok(DropTarget::View(self.owned(st, view, "ISO/IEC 9075:1992 §11.20 AR1")?)),
            DropObject::Domain(domain) => Ok(DropTarget::Domain(self.owned(st, domain, "ISO/IEC 9075:1992 §11.27 AR1")?)),
            DropObject::Assertion(assertion) => Ok(DropTarget::Assertion(self.owned(st, assertion, "ISO/IEC 9075:1992 §11.35 AR1")?)),
            DropObject::Collation(collation) => Ok(DropTarget::Collation(self.owned(st, collation, "ISO/IEC 9075-2:2023 §11.44 AR1")?)),
            DropObject::Sequence(sequence) => Ok(DropTarget::Sequence(self.owned(st, sequence, "ISO/IEC 9075-2:2023 §11.74 AR1")?)),
            DropObject::Type(name) => Ok(DropTarget::Type(self.owned(st, name, "ISO/IEC 9075-2:2023 §11.59 AR1")?)),
            DropObject::Trigger(name) => Ok(DropTarget::Trigger(self.owned(st, name, "ISO/IEC 9075-2:2023 §11.50 AR1")?)),
            DropObject::Routine(designator) => self.routine_target(st, designator),
            DropObject::Module(name) => Ok(DropTarget::Module(self.owned(st, name, "ISO/IEC 9075-4:2006 draft §9.22 AR1")?)),
            DropObject::Cast(..) | DropObject::Ordering(_) | DropObject::Transform(..) => self.type_drop_target(st, object),
            DropObject::Vector(kind, name) => Ok(super::embedding::drop_target(*kind, self.owned(st, name, "EXTENSION")?)),
            DropObject::XmlSchema(name) => self.xml_schema_target(st, name),
            DropObject::Schema { catalog, name, at } => {
                let ours = st.settings.get(CATALOG_NAME).to_string();
                if let Some(other) = catalog.as_ref().filter(|c| **c != ours.trim_end()) {
                    let msg = format!("[ISO/IEC 9075:1992 §11.2 SR2] the one catalog is {}, not {other}", ours.trim_end());
                    return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(*at));
                }
                // [SR2] the schema is there before its owner is asked.
                if st.catalog.schema_exists(name) && !self.owns_schema(st, name) {
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

    /// \[EXTENSION\] The registered XML Schema `name` names, qualified by
    /// the default schema, if this session's authorization identifier owns
    /// its schema, as of a collation [ISO/IEC 9075-2:2023 §11.44 AR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is permanently registered [ISO/IEC 9075-14:2006
    /// draft §4.2.6]; -104 if the session does not own its schema.
    fn xml_schema_target(&self, st: &State, name: &TableName) -> Result<DropTarget> {
        let qn = qualify(name, &self.default_schema());
        if crate::catalog::xml_schemas::permanent(&qn).is_some() {
            let msg = format!("[EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] {qn} is permanently registered: it is never dropped");
            return Err(rule(msg).or_at(name.at));
        }
        Ok(DropTarget::XmlSchema(self.owned(st, name, "EXTENSION")?))
    }

    /// `name`, qualified by the default schema, if this session's
    /// authorization identifier owns its schema, as the Access Rule
    /// `cited` says.
    fn owned(&self, st: &State, name: &TableName, cited: &str) -> Result<QualifiedName> {
        let qn = qualify(name, &self.default_schema());
        if !self.owns_schema(st, &qn.auth) {
            return fail(Code::Privilege, format!("[{cited}] {qn} is in a schema {} does not own", self.auth())).or_at(name.at);
        }
        Ok(qn)
    }
}
