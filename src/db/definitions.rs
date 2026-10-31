//! \[EXTENSION\] SQL-schema statements outside a `<schema definition>`
//! [ISO/IEC 9075:1992 §20.1], in a transaction (RULES.md D33, D47): CREATE
//! TABLE [§11.3], CREATE VIEW [§11.19], CREATE and ALTER DOMAIN [§11.21,
//! §11.22] (`db/domains.rs`), CREATE ASSERTION [§11.34]
//! (`db/assertions.rs`), GRANT [§11.36] and REVOKE [§11.37]
//! (`db/revoke.rs`).
//!
//! An object is defined by the owner of its schema, its own name and the
//! names in it written without a schema qualified by the session's default
//! schema [§5.4 SR8b], which its stored texts keep, so they are read again
//! with the names they were written with. Each statement takes the lock of
//! the name it defines, or grants or revokes on, exclusive, and shared
//! ones on the tables it reads and, defining, on its schema; it is made on
//! the definitions as they are once it holds them, so one that waited sees
//! what the other did. Its change is logged as its text, and made again
//! from it at commit (`Change::Defined`).

use std::mem;
use std::sync::Arc;

use super::alter::Made;
use super::constraints::Within;
use super::ddl::{Changed, Dropped, validate_rows};
use super::{Completion, Session, State, api, long_texts, rule};
use crate::ast::{PrivilegeDefinition, SchemaStatement, SqlStatement, TableDefinition, TableName, ViewDefinition};
use crate::binder::qualify;
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc, merged, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::log::record::Change;
use crate::parser::Parser;
use crate::txn::{LockMode, Txn};

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
        let schema = self.default_schema();
        // [ISO/IEC 9075:1992 §5.3 SR18] the displacement of a datetime it
        // writes without one, logged with it.
        let zone = self.now.get().zone;
        let object = self.object_of(st, statement, &schema)?;
        self.refuse_declared(st, statement, (&object, &schema))?;
        st.acquire_name(tx, &object, LockMode::Exclusive)?;
        let mut held = vec![(object, LockMode::Exclusive)];
        loop {
            let fresh = self.definitions(self.db.snapshot()?)?;
            let mut next = State::clone(&fresh);
            let defined = next.define((&self.auth(), &schema, zone), statement, (None, None))?;
            let wanted: Vec<(QualifiedName, LockMode)> = defined.locks.iter().filter(|l| !held.contains(l)).cloned().collect();
            if wanted.is_empty() {
                let next = Arc::new(next);
                for table in &defined.validate {
                    validate_rows(&next, tx, table, self.effects(&next))?;
                }
                if let Some(a) = &defined.asserted {
                    next.check_assertion(tx, a)?;
                }
                // [ISO/IEC 9075:1992 §11.5 GR1b v] a default placed padded.
                let padding = held.first().and_then(|(named, _)| next.padding_of_statement(statement, named));
                let change = Change::Defined { auth: self.auth().clone(), schema, zone, sql: text.to_owned(), placed: defined.placed };
                self.stage(&fresh, tx, Changed { next, change, made: defined.made })?;
                return Ok(long_texts::warned(defined.completion, text, || long_texts::of_statement(statement)).warned(padding));
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
            SchemaStatement::Table(t) => self.own_object(st, &t.name, (schema, "§11.3 AR1")),
            SchemaStatement::View(v) => self.own_object(st, &v.name, (schema, "§11.19 AR1")),
            SchemaStatement::Grant(p) => Ok(qualify(&p.table, schema)),
            SchemaStatement::Revoke(r) => Ok(qualify(&r.table, schema)),
            SchemaStatement::Domain(d) => self.own_object(st, &d.name, (schema, "§11.21 AR1")),
            SchemaStatement::Assertion(a) => self.own_object(st, &a.name, (schema, "§11.34 AR1")),
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
    /// §5.4 SR8b], in a schema the session owns, as the Access Rule `rule`
    /// says. A schema not there is refused as the object is defined.
    fn own_object(&self, st: &State, name: &TableName, (schema, rule): (&str, &str)) -> Result<QualifiedName> {
        let qn = qualify(name, schema);
        if st.catalog.schema_exists(&qn.auth) && !self.owns_schema(st, &qn.auth) {
            let msg = format!("[ISO/IEC 9075:1992 {rule}] {} is in schema {}, which {} does not own", qn.ident, qn.auth, self.auth());
            return Err(SqlError::new(Code::Privilege, msg).or_at(name.at));
        }
        Ok(qn)
    }
}

impl Session {
    /// Whether the session's authorization identifier owns the schema
    /// `schema` [ISO/IEC 9075:1992 §11.1 GR2] (`owner_of`): the Access Rule
    /// of every statement that defines or changes what is in it.
    pub(super) fn owns_schema(&self, st: &State, schema: &str) -> bool {
        owner_of(&*st.catalog, schema) == *self.auth()
    }
}

impl State {
    /// Makes `statement` of `auth`, its names in schema `schema`; a table
    /// it creates given `placed`'s engine and generation and `made`'s
    /// storage, if given.
    pub(super) fn define(
        &mut self,
        (auth, schema, zone): (&str, &str, i16),
        statement: &SchemaStatement,
        (placed, made): (Option<&(String, u32)>, Option<&Made>),
    ) -> Result<Defined> {
        match statement {
            SchemaStatement::Table(t) => self.create_table((schema, zone), t, (placed, made)),
            SchemaStatement::View(v) => self.create_view(schema, v),
            SchemaStatement::Grant(p) => self.grant(auth, schema, p),
            SchemaStatement::Revoke(r) => self.revoke((auth, schema), r),
            SchemaStatement::Domain(d) => self.create_domain((schema, zone), d),
            SchemaStatement::Assertion(a) => self.create_assertion(schema, a),
            SchemaStatement::AlterDomain(a) => self.alter_domain((schema, zone), a),
        }
    }

    /// Makes the definition `sql`, a schema statement logged
    /// (`Change::Defined`), again: what it took away.
    ///
    /// # Errors
    ///
    /// The statement's own; SQLCODE -910 if it is not a schema statement.
    pub(super) fn redefine(
        &mut self,
        (auth, schema, zone): (&str, &str, i16),
        sql: &str,
        placed: Option<&(String, u32)>,
        made: Option<&Made>,
    ) -> Result<Dropped> {
        let SqlStatement::Schema { statement, .. } = Parser::new(sql)?.of_definition().sql_statement()? else {
            return Err(api(format!("[DESIGN] {sql} is not a schema statement")));
        };
        Ok(self.define((auth, schema, zone), &statement, (placed, made))?.dropped)
    }

    /// [ISO/IEC 9075:1992 §11.3] Creates base table `t` in schema `schema`.
    fn create_table(
        &mut self,
        (schema, zone): (&str, i16),
        t: &TableDefinition,
        (placed, made): (Option<&(String, u32)>, Option<&Made>),
    ) -> Result<Defined> {
        let qn = qualify(&t.name, schema);
        if let Some((engine, _)) = placed {
            self.placement.insert(qn.clone(), engine.clone());
        }
        let desc = self.table_described((schema, zone), t, placed)?;
        let mut catalog = self.catalog.duplicate();
        catalog.add_table(desc.clone());
        let built = self.place_table(&mut *catalog, &desc, made)?;
        self.catalog = Arc::from(catalog);
        let locks = in_schema(&qn, read_by(&desc));
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
        let (mut desc, pending) = self.describe_table((&qn.auth, schema, zone, Within::Session), t, &earlier, &keys).or_at(t.name.at)?;
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
        if p.domain {
            return self.grant_usage(auth, schema, p).or_at(p.table.at);
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
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Table(t), .. }) =
        Parser::new(sql).and_then(|p| p.of_definition().sql_statement())
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
/// read.
fn read_by(desc: &TableDesc) -> Vec<QualifiedName> {
    let mut read: Vec<QualifiedName> = Vec::new();
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
