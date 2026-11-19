//! \[EXTENSION\] SQL-schema statements outside a `<schema definition>`
//! [ISO/IEC 9075:1992 §20.1], in a transaction (RULES.md D33, D47): CREATE
//! TABLE [§11.3], CREATE VIEW [§11.19], CREATE and ALTER DOMAIN [§11.21,
//! §11.22] (`db/domains.rs`), CREATE ASSERTION [§11.34]
//! (`db/assertions.rs`), GRANT [§11.36] and REVOKE [§11.37]
//! (`db/revoke.rs`), of ISO/IEC 9075-2:2023, CREATE COLLATION
//! (`db/collations.rs`) and CREATE and ALTER SEQUENCE (`db/sequences.rs`),
//! CREATE XML SCHEMA (`definitions/xml_schemas.rs`) and CREATE and ALTER
//! XQUERY MODULE (`definitions/xquery_modules.rs`).
//!
//! An object is defined by the owner of its schema, its own name and the
//! names in it written without a schema qualified by the session's default
//! schema [§5.4 SR8b], which its stored texts keep, so they are read again
//! with the names they were written with. Each statement takes the lock of
//! the name it defines, or grants or revokes on, exclusive, and shared
//! ones on the tables it reads and, defining, on its schema; it is made on
//! the definitions as they are once it holds them, so one that waited sees
//! what the other did. Its change is logged as its text, with the
//! CURRENT_TIMESTAMP it ran at, and made again from it at commit
//! (`Change::Defined`), what it makes stamped with that instant, not the
//! clock's then [ISO/IEC 9075-2:2023 §11.60 GR5].

pub(super) mod xml_schemas;
pub(super) mod xquery_modules;

use std::mem;
use std::sync::Arc;

use super::alter::Made;
use super::collations::is_made;
use super::constraints::Within;
use super::contents::contents_read;
use super::ddl::{Changed, Dropped, validate_rows};
use super::sequences::Epochs;
use super::{Completion, Session, State, api, long_texts, rule};
use crate::ast::{
    DropBehavior, PrivilegeDefinition, PrivilegeObject, SchemaStatement, SqlStatement, TableDefinition, TableName, ViewDefinition,
};
use crate::binder::{making_typed, making_typed_now, qualify};
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc, merged, owner_of, types_in};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::log::record::Change;
use crate::parser::Parser;
use crate::settings::{FUNCTION_CALLS, XQUERY_STATIC_TYPING};
use crate::txn::{LockMode, Txn};
use crate::value::datetime::Now;

/// What a schema statement did to the definitions.
#[derive(Default)]
pub(super) struct Defined {
    /// How it completed: with a warning, if it granted or revoked less
    /// than it named.
    pub(super) completion: Completion,
    /// The storage and indexes of a table it created.
    pub(super) made: Option<Made>,
    /// What it took away.
    pub(super) dropped: Dropped,
    /// Of a table it created, its engine and its storage's generation.
    pub(super) placed: Option<(String, u32)>,
    /// The locks it needs beside its object's: the names it reads, or
    /// changes the definitions of.
    pub(super) locks: Vec<(QualifiedName, LockMode)>,
    /// The tables whose rows must satisfy a constraint it added to them
    /// (`db/domains.rs`).
    pub(super) validate: Vec<QualifiedName>,
    /// The assertion it created, to be checked as it ends
    /// (`db/assertions.rs`).
    pub(super) asserted: Option<QualifiedName>,
}

impl Session {
    /// `statement`, written `text`, in transaction `tx`: its change staged
    /// with the transaction's.
    pub(super) fn schema_statement(&self, st: &Arc<State>, tx: &Txn, statement: &SchemaStatement, text: &str) -> Result<Completion> {
        // Its own texts read again as the session read it (RULES.md D74).
        let _calls = Parser::refusing_calls(text, st.settings.is_on(FUNCTION_CALLS));
        // [XQuery §5.2.3] Its definitions typed statically as the session
        // asks, logged with it (RULES.md D106).
        let _typed = making_typed(st.settings.is_on(XQUERY_STATIC_TYPING));
        let schema = self.default_schema();
        let object = self.object_of(st, statement, &schema)?;
        // [EXTENSION] A model's text logged with the shape it is of.
        let shaped = Self::shaped_model(st, statement, &schema)?;
        let text = shaped.as_deref().unwrap_or(text);
        self.refuse_declared(st, statement, (&object, &schema))?;
        st.acquire_name(tx, &object, LockMode::Exclusive)?;
        // The generated columns a REVOKE with CASCADE loses dropped first
        // (`db/generated/named.rs`).
        let mut lost = Vec::new();
        if let SchemaStatement::Revoke(r) = statement
            && r.behavior == DropBehavior::Cascade
        {
            let fresh = self.definitions(self.db.snapshot()?)?;
            let a = self.grantor(&fresh, r.grantor)?;
            lost = fresh.generations_revoked((&a, &schema), r).or_at(r.table.at)?;
        }
        if let SchemaStatement::Role(r) = statement {
            lost = self.role_generations(r)?;
        }
        self.dropping_generations(tx, &lost, || {
            self.revert_impacted(tx, statement)?;
            self.revert_role_impacted(tx, statement)?;
            self.staged_statement(tx, statement, (text, object))
        })
    }

    /// `statement`, written `text`, staged with transaction `tx`'s changes
    /// once the locks of all it defines or changes are held, the lock of
    /// `object` held already: made again, on the definitions then, each
    /// time it needs one more.
    fn staged_statement(&self, tx: &Txn, statement: &SchemaStatement, (text, object): (&str, QualifiedName)) -> Result<Completion> {
        let schema = self.default_schema();
        // [ISO/IEC 9075:1992 §5.3 SR18] the displacement of a datetime it
        // writes without one, and [ISO/IEC 9075-2:2023 §11.60 GR5] the
        // timestamps of what it makes, logged with it.
        let now = self.now.get();
        let mut held = vec![(object, LockMode::Exclusive)];
        loop {
            let fresh = self.definitions(self.db.snapshot()?)?;
            let mut next = State::clone(&fresh);
            let mut epochs = Epochs::fresh();
            let (auth, role) = (self.auth(), self.role.borrow().clone());
            let defined = next.define(((&auth, role.as_deref()), &schema, now), statement, (None, None, &mut epochs))?;
            let wanted: Vec<(QualifiedName, LockMode)> = defined.locks.iter().filter(|l| !held.contains(l)).cloned().collect();
            if wanted.is_empty() {
                let next = Arc::new(next);
                // [ISO/IEC 9075-2:2023 §11.3 GR10] the rows of its query, of
                // a table created WITH DATA, inserted before it is staged.
                let mut filled = Ok(None);
                if let SchemaStatement::Table(t) = statement {
                    filled = self.inserted_data(&next, tx, t);
                }
                if let SchemaStatement::Vector(v) = statement {
                    filled = self.pipeline_filled(&next, tx, v).map(|()| None);
                }
                let filled = match filled {
                    Ok(filled) => filled,
                    Err(e) => {
                        defined.made.iter().for_each(Made::discard);
                        return Err(e);
                    }
                };
                for table in &defined.validate {
                    validate_rows(&next, tx, table, self.effects(&next))?;
                }
                if let Some(a) = &defined.asserted {
                    next.check_assertion(tx, a)?;
                    next.start_totals(tx, a)?;
                    // Checked whole, so not again of rows written before.
                    self.deferral.borrow_mut().forget(a, next.list_space())?;
                }
                let (sql, placed, epochs) = (text.to_owned(), defined.placed, epochs.given());
                let typed = making_typed_now();
                let change = Change::Defined { auth: auth.clone(), role: role.clone(), schema, now, sql, placed, epochs, typed };
                self.stage(&fresh, tx, Changed { next, change, made: defined.made })?;
                // [§14.11 GR3] no row inserted is no data.
                let completion = if filled == Some(0) { Completion::NoData } else { defined.completion };
                return Ok(long_texts::warned(completion, text, || long_texts::of_statement(statement)));
            }
            // Made again once the locks are held, on the definitions then.
            defined.made.iter().for_each(Made::discard);
            for (name, mode) in wanted {
                fresh.acquire_name(tx, &name, mode)?;
                held.push((name, mode));
            }
        }
    }

    /// The object `statement` defines or alters, or grants or revokes
    /// privileges on, its name qualified by `schema`.
    ///
    /// # Errors
    ///
    /// For an object defined or a domain altered: SQLCODE -104 if its
    /// schema is not the session's own.
    fn object_of(&self, st: &State, statement: &SchemaStatement, schema: &str) -> Result<QualifiedName> {
        match statement {
            SchemaStatement::Table(t) => self.own_object(st, &t.name, (schema, "ISO/IEC 9075:1992 §11.3 AR1")),
            SchemaStatement::View(v) => self.own_object(st, &v.name, (schema, "ISO/IEC 9075:1992 §11.19 AR1")),
            SchemaStatement::Grant(p) => Ok(qualify(&p.table, schema)),
            SchemaStatement::Revoke(r) => Ok(qualify(&r.table, schema)),
            SchemaStatement::Role(r) => self.role_access(st, r),
            SchemaStatement::Domain(d) => self.own_object(st, &d.name, (schema, "ISO/IEC 9075:1992 §11.21 AR1")),
            SchemaStatement::Assertion(a) => self.own_object(st, &a.name, (schema, "ISO/IEC 9075:1992 §11.34 AR1")),
            SchemaStatement::Collation(c) => self.own_object(st, &c.name, (schema, "ISO/IEC 9075-2:2023 §11.43 SR1")),
            SchemaStatement::Sequence(d) => self.own_object(st, &d.name, (schema, "ISO/IEC 9075-2:2023 §11.72 SR4")),
            SchemaStatement::Type(d) => self.own_object(st, &d.name, (schema, "ISO/IEC 9075-2:2023 §11.51 AR1")),
            SchemaStatement::Trigger(t) => self.own_object(st, &t.name, (schema, "ISO/IEC 9075-2:2023 §11.49 AR1")),
            SchemaStatement::Routine(r) => self.own_object(st, &r.name, (schema, "ISO/IEC 9075-2:2023 §11.60 AR1")),
            SchemaStatement::Module(m) => self.own_object(st, &m.name, (schema, "ISO/IEC 9075-4:2006 draft §9.21 AR1")),
            SchemaStatement::Vector(v) => {
                Session::plugin_registered(st, statement, schema)?;
                self.own_object(st, v.name(), (schema, "EXTENSION"))
            }
            // [EXTENSION] as CREATE COLLATION [ISO/IEC 9075-2:2023 §11.43 SR1].
            SchemaStatement::XmlSchema(x) => self.own_object(st, &x.name, (schema, "EXTENSION")),
            SchemaStatement::XQueryModule(m) | SchemaStatement::AlterXQueryModule(m) => self.own_object(st, &m.name, (schema, "EXTENSION")),
            SchemaStatement::AlterType(_)
            | SchemaStatement::Method(_)
            | SchemaStatement::Cast(_)
            | SchemaStatement::Ordering(_)
            | SchemaStatement::Transform(_)
            | SchemaStatement::AlterTransform(_) => self.type_object(st, statement, schema),
            SchemaStatement::AlterSequence(a) => {
                let qn = qualify(&a.name, schema);
                // [ISO/IEC 9075-2:2023 §11.73 AR1] only the owner of its
                // schema alters a sequence generator.
                if !self.owns_schema(st, &qn.auth) {
                    let msg =
                        format!("[ISO/IEC 9075-2:2023 §11.73 AR1] sequence generator {qn} is in a schema {} does not own", self.auth());
                    return Err(SqlError::new(Code::Privilege, msg).or_at(a.name.at));
                }
                Ok(qn)
            }
            SchemaStatement::AlterDomain(a) => {
                let qn = qualify(&a.name, schema);
                // [§11.22 AR1] only the owner of its schema alters a domain.
                if !self.owns_schema(st, &qn.auth) {
                    let msg = format!("[ISO/IEC 9075:1992 §11.22 AR1] domain {qn} is in a schema {} does not own", self.auth());
                    return Err(SqlError::new(Code::Privilege, msg).or_at(a.name.at));
                }
                Ok(qn)
            }
        }
    }

    /// The name `name` of an object to define, qualified by the session's
    /// default schema `schema` if written without one [ISO/IEC 9075:1992
    /// §5.4 SR8b], in a schema the session owns, as the rule `cited`, its
    /// standard named, says. A schema not there is refused as the object
    /// is defined.
    fn own_object(&self, st: &State, name: &TableName, (schema, cited): (&str, &str)) -> Result<QualifiedName> {
        let qn = qualify(name, schema);
        if st.catalog.schema_exists(&qn.auth) && !self.owns_schema(st, &qn.auth) {
            let msg = format!("[{cited}] {} is in schema {}, which {} does not own", qn.ident, qn.auth, self.auth());
            return Err(SqlError::new(Code::Privilege, msg).or_at(name.at));
        }
        Ok(qn)
    }
}

impl Session {
    /// Whether an enabled authorization identifier of the session owns the
    /// schema `schema` [ISO/IEC 9075:1992 §11.1 GR2; ISO/IEC 9075-2:2023
    /// §4.42.4] (`owner_of`): its user, its role, or a role applicable for
    /// that role. It is the Access Rule
    /// of every statement that defines or changes what is in it. None owns
    /// DEFINITION_SCHEMA, the database's own, whose tables of kept totals
    /// (`db/assertions/kept.rs`) no statement names [§21.1].
    pub(super) fn owns_schema(&self, st: &State, schema: &str) -> bool {
        let authorization = self.authorization();
        schema != crate::system::DEFINITION_SCHEMA && authorization.of(st.catalog.roles()).cell.includes(owner_of(&*st.catalog, schema))
    }
}

impl State {
    /// Makes `statement` of the cell `auth` and `role` [ISO/IEC 9075-2:2023
    /// §4.42.1.2], its names in schema `schema`, run as CURRENT_TIMESTAMP
    /// was `now`: its datetimes of `now`'s displacement, and what it makes
    /// stamped with `now` [§11.60 GR5]; a table
    /// it creates given `placed`'s engine and generation and `made`'s
    /// storage, if given; a sequence generator it defines or restarts its
    /// epoch from `epochs`.
    pub(super) fn define(
        &mut self,
        ((auth, role), schema, now): ((&str, Option<&str>), &str, Now),
        statement: &SchemaStatement,
        (placed, made, epochs): (Option<&(String, u32)>, Option<&Made>, &mut Epochs),
    ) -> Result<Defined> {
        self.stamp = now;
        let zone = now.zone;
        match statement {
            SchemaStatement::Table(t) => self.create_table((schema, zone), t, (placed, made, epochs)),
            SchemaStatement::View(v) => self.create_view(schema, v),
            SchemaStatement::Grant(p) => {
                let a = self.cell_grantor((auth, role), p.grantor).or_at(p.table.at)?;
                let mut defined = self.grant(&a, schema, p)?;
                self.module_grants_upgraded();
                // So no role is created of a grantee's name meanwhile.
                defined.locks.push((super::roles::roles_lock(), LockMode::Shared));
                Ok(defined)
            }
            SchemaStatement::Revoke(r) => {
                let a = self.cell_grantor((auth, role), r.grantor).or_at(r.table.at)?;
                self.revoke((&a, schema), r)
            }
            SchemaStatement::Role(r) => self.role_statement((auth, role), r),
            SchemaStatement::Domain(d) => self.create_domain((schema, zone), d),
            SchemaStatement::Assertion(a) => self.create_assertion(schema, a, made),
            SchemaStatement::Collation(c) => self.create_collation((auth, schema), c),
            SchemaStatement::AlterDomain(a) => self.alter_domain((schema, zone), a),
            SchemaStatement::Sequence(d) => self.create_sequence(schema, d, epochs),
            SchemaStatement::AlterSequence(a) => self.alter_sequence(schema, a, epochs),
            SchemaStatement::Type(d) => self.create_user_type(schema, d),
            SchemaStatement::Trigger(t) => self.create_trigger(schema, t),
            SchemaStatement::Routine(r) => self.create_routine(schema, r),
            SchemaStatement::Module(m) => self.create_module((schema, zone), m, (made, epochs)),
            SchemaStatement::AlterType(a) => self.alter_user_type(schema, a),
            SchemaStatement::Method(m) => self.create_method(schema, m),
            SchemaStatement::Cast(c) => self.create_cast(schema, c),
            SchemaStatement::Ordering(o) => self.create_ordering(schema, o),
            SchemaStatement::Transform(t) => self.create_transform(schema, t),
            SchemaStatement::AlterTransform(t) => self.alter_transform(schema, t),
            SchemaStatement::Vector(v) => self.vector_statement(schema, v),
            SchemaStatement::XmlSchema(x) => self.create_xml_schema(schema, x),
            SchemaStatement::XQueryModule(m) => self.create_xquery_module(schema, m),
            SchemaStatement::AlterXQueryModule(m) => self.alter_xquery_module(schema, m),
        }
    }

    /// Makes the definition `sql`, a schema statement logged
    /// (`Change::Defined`), again, with the epochs it logged: what it took
    /// away.
    ///
    /// # Errors
    ///
    /// The statement's own; SQLCODE -910 if it is not a schema statement.
    pub(super) fn redefine(
        &mut self,
        (cell, schema, now): ((&str, Option<&str>), &str, Now),
        sql: &str,
        (placed, made, epochs): (Option<&(String, u32)>, Option<&Made>, &[u64]),
    ) -> Result<Dropped> {
        let SqlStatement::Schema { statement, .. } = Parser::for_stored(sql)?.sql_statement()? else {
            return Err(api(format!("[DESIGN] {sql} is not a schema statement")));
        };
        Ok(self.define((cell, schema, now), &statement, (placed, made, &mut Epochs::logged(epochs)))?.dropped)
    }

    /// [ISO/IEC 9075:1992 §11.3] Creates base table `t` in schema `schema`,
    /// the generator of its identity column given its epoch from `epochs`.
    fn create_table(
        &mut self,
        (schema, zone): (&str, i16),
        t: &TableDefinition,
        (placed, made, epochs): (Option<&(String, u32)>, Option<&Made>, &mut Epochs),
    ) -> Result<Defined> {
        let qn = qualify(&t.name, schema);
        if let Some((engine, _)) = placed {
            self.placement.insert(qn.clone(), engine.clone());
        }
        let mut desc = self.table_described((schema, zone), t, placed)?;
        self.give_epochs(&mut desc, epochs)?;
        let mut catalog = self.catalog.duplicate();
        catalog.add_table(desc.clone());
        let mut built = self.place_table(&mut *catalog, &desc, made)?;
        // [ISO/IEC 9075-2:2023 §11.3 GR1c] A system-versioned table, its
        // history made (RULES.md D82).
        if t.versioning.is_some() {
            built.tables.push(self.versioned_table(&mut *catalog, &desc, made).or_at(t.name.at)?);
        }
        self.catalog = Arc::from(catalog);
        let mut reads = read_by(&desc);
        // [ISO/IEC 9075-2:2023 §11.3] what its columns were found from, so
        // they are found alike at commit (`db/contents.rs`).
        for name in contents_read(t, schema) {
            if name != qn && !reads.contains(&name) {
                reads.push(name);
            }
        }
        let locks = in_schema(&qn, reads);
        Ok(Defined { made: Some(built), placed: Some((desc.engine, desc.generation)), locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075:1992 §11.3] The descriptor of base table `t` in schema
    /// `schema`, its constraints bound, a generation after `placed`'s or
    /// the last of its name: its rules checked, nothing made.
    pub(super) fn table_described(
        &mut self,
        (schema, zone): (&str, i16),
        t: &TableDefinition,
        placed: Option<&(String, u32)>,
    ) -> Result<TableDesc> {
        let qn = qualify(&t.name, schema);
        self.new_object(&qn, "§11.3 SR2").or_at(t.name.at)?;
        let earlier: Vec<TableDesc> = self.catalog.tables().into_iter().filter(|d| d.name.auth == qn.auth).cloned().collect();
        let keys: Vec<Vec<_>> = earlier.iter().map(|_| Vec::new()).collect();
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let (mut desc, pending) =
            self.describe_table((&qn.auth, &owner, schema, zone, Within::Session), t, &earlier, &keys).or_at(t.name.at)?;
        self.constraint_schemas(&qn, (&desc.constraints, &pending)).or_at(t.name.at)?;
        // Another table of the name, dropped, had a generation of its own.
        desc.generation = placed.map_or_else(|| self.retired.get(&qn).map_or(0, |g| g.saturating_add(1)), |(_, g)| *g);
        // Its foreign keys and CHECKs, it among the tables they may name.
        let mut staged = self.catalog.duplicate();
        staged.add_table(desc.clone());
        let saved = mem::replace(&mut self.catalog, Arc::from(staged));
        let completed = self.complete_tables(schema, std::slice::from_mut(&mut desc), std::slice::from_ref(&pending));
        self.catalog = saved;
        // An error of a CHECK, bound from its own text, is placed at the table.
        completed.or_at(t.name.at)?;
        self.check_new_objects(std::slice::from_ref(&desc))?;
        Ok(desc)
    }

    /// [ISO/IEC 9075:1992 §11.19] Creates view `v` in schema `schema`.
    fn create_view(&mut self, schema: &str, v: &ViewDefinition) -> Result<Defined> {
        let qn = qualify(&v.name, schema);
        self.new_object(&qn, "§11.19 SR3").or_at(v.name.at)?;
        self.define_views((&qn.auth, schema), std::slice::from_ref(v))?;
        let reads = self.catalog.view(&qn).map(|view| view.reads.clone()).unwrap_or_default();
        Ok(Defined { locks: in_schema(&qn, reads), ..Defined::default() })
    }

    /// That `qn` may name a table or view defined now: its schema exists,
    /// and holds no table or view of the name, as the Syntax Rule
    /// `rule_cited` says.
    fn new_object(&self, qn: &QualifiedName, rule_cited: &str) -> Result<()> {
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[ISO/IEC 9075:1992 {rule_cited}] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        if self.catalog.table(qn).is_some() || self.catalog.view(qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075:1992 {rule_cited}] schema {} has a table or view {} already", qn.auth, qn.ident)));
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.36] GRANT `p` by `auth`, its names in schema
    /// `schema`: what `auth` holds with grant option of what it names
    /// granted, a warning for the rest [GR14, GR15].
    fn grant(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        if p.object == PrivilegeObject::Collation {
            return self.grant_collation_usage(auth, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::Domain {
            return self.grant_usage(auth, schema, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::Sequence {
            return self.grant_sequence_usage(auth, schema, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::Module {
            return self.grant_on_module(auth, schema, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::Model {
            return self.grant_model_execute(auth, schema, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::XmlSchema {
            return self.grant_xml_schema_usage(auth, schema, p).or_at(p.table.at);
        }
        if p.object == PrivilegeObject::XQueryModule {
            return self.grant_xquery_module_usage(auth, schema, p).or_at(p.table.at);
        }
        if matches!(p.object, PrivilegeObject::Type | PrivilegeObject::SpecificRoutine(_) | PrivilegeObject::Routine(_)) {
            return self.grant_on_type(auth, schema, p).or_at(p.table.at);
        }
        let (granted, warned) = self.describe_grant((auth, schema), p).or_at(p.table.at)?;
        let name = qualify(&p.table, schema);
        let mut catalog = self.catalog.duplicate();
        let mut all: Vec<_> = catalog.privileges(&name).into_iter().cloned().collect();
        all.extend(granted);
        catalog.set_privileges(&name, merged(all));
        self.catalog = Arc::from(catalog);
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED) } else { Completion::Success };
        Ok(Defined { completion, ..Defined::default() })
    }
}

/// The table the logged statement `sql` created, its names in schema
/// `schema`, if it created one.
pub(super) fn created(schema: &str, sql: &str) -> Option<QualifiedName> {
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Table(t), .. }) = Parser::for_stored(sql).and_then(|mut p| p.sql_statement())
    else {
        return None;
    };
    Some(qualify(&t.name, schema))
}

/// The locks a table or view `qn` defined takes beside its own, shared:
/// those of what it reads, `reads`, and of its schema, which is not
/// dropped meanwhile (`db/drop.rs`).
fn in_schema(qn: &QualifiedName, reads: Vec<QualifiedName>) -> Vec<(QualifiedName, LockMode)> {
    reads.into_iter().chain([QualifiedName::of_schema(&qn.auth)]).map(|name| (name, LockMode::Shared)).collect()
}

/// The tables other than `desc` its foreign keys reference and its CHECKs
/// read, the collations made its columns are of, which are not reverted
/// meanwhile (`db/collations/revert.rs`), and the user-defined types they
/// are based on, which are not dropped meanwhile (`db/user_types.rs`).
fn read_by(desc: &TableDesc) -> Vec<QualifiedName> {
    let mut read: Vec<QualifiedName> = Vec::new();
    desc.columns.iter().for_each(|c| types_in(&c.data_type, &mut read));
    for name in desc.columns.iter().filter_map(|c| c.collation.clone()).filter(is_made) {
        if !read.contains(&name) {
            read.push(name);
        }
    }
    for c in &desc.constraints {
        let names = match &c.kind {
            ConstraintKind::ForeignKey(fk) => vec![fk.table.clone()],
            ConstraintKind::Check { reads, .. } => reads.clone(),
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } => Vec::new(),
        };
        for name in names {
            if name != desc.name && !read.contains(&name) {
                read.push(name);
            }
        }
    }
    read
}
