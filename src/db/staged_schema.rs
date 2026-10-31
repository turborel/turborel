//! A `<schema definition>` a session runs [ISO/IEC 9075:1992 §11.1], made
//! in its transaction [§20.1 GR6b ii, iii]: staged as the transaction's
//! other definitions are (`db/ddl.rs`), published as it commits, undone if
//! it rolls back. The schema's name is held exclusively until then, so no
//! other transaction makes a schema of the name, or anything in it.

use std::sync::Arc;

use super::alter::Made;
use super::ddl::{Changed, Dropped};
use super::{Completion, Database, Session, State, long_texts};
use crate::catalog::QualifiedName;
use crate::error::{Result, SqlState};
use crate::log::record::Change;
use crate::parser::Parser;
use crate::txn::{LockMode, Txn};

impl Session {
    /// The session's own `<schema definition>` `sql`, of the schema `name`,
    /// made in its transaction, which it initiates if none is active; a
    /// statement of it, undone whole if it fails.
    pub(super) fn schema_in_transaction(&self, sql: &str, name: &str) -> Result<Completion> {
        let call = self.enter()?;
        let st = call.state();
        let tx = self.ensure_transaction(st);
        call.runs(&tx);
        tx.begin_statement(|| !self.cursors_open())?;
        let staged = self.stage_schema(st, &tx, sql, name);
        self.db.noted(staged.map_err(|e| st.fail_statement(&tx, &e).err().unwrap_or(e)))
    }

    /// Stages `sql`, of the schema `name`, in `tx`: made on the definitions
    /// published once its name is held, with those `tx` staged before; its
    /// assertions checked [§11.34 GR2].
    fn stage_schema(&self, st: &Arc<State>, tx: &Txn, sql: &str, name: &str) -> Result<Completion> {
        st.acquire_name(tx, &QualifiedName::of_schema(name), LockMode::Exclusive)?;
        let fresh = self.definitions(self.db.snapshot()?)?;
        let mut next = State::clone(&fresh);
        let zone = self.now.get().zone;
        let (owner, engines, warned, asserted, made) = next.schema(sql, (Some(&self.auth()), zone), None)?;
        let next = Arc::new(next);
        if let Err(e) = asserted.iter().try_for_each(|a| next.check_assertion(tx, a)) {
            made.discard();
            return Err(e);
        }
        let padding = padding_of(&next, sql);
        let change = Change::Schema { sql: sql.to_owned(), owner, engines, zone };
        self.stage(&fresh, tx, Changed { next, change, made: Some(made) })?;
        let done = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED) } else { Completion::Success };
        let schema = || Parser::new(sql).and_then(|p| p.of_definition().schema()).ok().as_ref().and_then(long_texts::of_schema);
        Ok(long_texts::warned(done, sql, schema).warned(padding))
    }
}

impl Database {
    /// `schema`, the `<schema definition>` `sql` owned by `implicit` if it
    /// names no AUTHORIZATION [ISO/IEC 9075:1992 §11.1 SR2b]; a datetime it
    /// writes without a displacement of `zone`, a session's, or else the
    /// database's TIME_ZONE [§5.3 SR18].
    pub(super) fn schema_of(&self, sql: &str, (implicit, zone): (Option<&str>, Option<i16>)) -> Result<Completion> {
        let current = self.snapshot()?;
        // Its name held, as a session's is (`db/staged_schema.rs`), taken
        // before the gate is entered, so a wait for it never holds the gate.
        let name = Parser::new(sql)?.in_catalog(&current.catalog_name()).schema()?.name;
        let holder = current.begin();
        current.acquire_name(&holder, &QualifiedName::of_schema(&name), LockMode::Exclusive)?;
        // Published before a checkpoint can see its record or its tables.
        let _gate = current.tx.gate.enter();
        self.noted(self.change(|st| {
            let zone = zone.unwrap_or_else(|| st.now(&crate::settings::NONE).zone);
            let (owner, engines, warned, asserted, _) = st.schema(sql, (implicit, zone), None)?;
            st.check_new_assertions(&asserted)?;
            // Logged before it is published (RULES.md D25).
            st.log_definition(None, Change::Schema { sql: sql.into(), owner, engines, zone })?;
            let done = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED) } else { Completion::Success };
            let schema = || Parser::new(sql).and_then(|p| p.of_definition().schema()).ok().as_ref().and_then(long_texts::of_schema);
            Ok(long_texts::warned(done, sql, schema).warned(padding_of(st, sql)))
        }))
    }
}

impl State {
    /// Makes the `<schema definition>` `sql` again, as `owner` made it, its
    /// datetimes of `zone`: each table placed in the engine `engines`
    /// gives it, and given what `made` holds for it, if given, so a
    /// transaction's rows written in it are kept.
    pub(super) fn reschema(
        &mut self,
        (sql, owner, zone): (&str, &str, i16),
        engines: &[(String, String)],
        made: Option<&Made>,
    ) -> Result<Dropped> {
        let schema = Parser::new(sql)?.of_definition().schema()?.name;
        for (ident, engine) in engines {
            self.placement.insert(QualifiedName { auth: schema.clone(), ident: ident.clone() }, engine.clone());
        }
        self.schema(sql, (Some(owner), zone), made)?;
        Ok(Dropped::default())
    }
}

/// The tables of the `<schema definition>` `sql`, of which `engines` names
/// each.
pub(super) fn schema_tables(sql: &str, engines: &[(String, String)]) -> Vec<QualifiedName> {
    let Ok(schema) = Parser::new(sql).and_then(|p| p.of_definition().schema()) else { return Vec::new() };
    engines.iter().map(|(ident, _)| QualifiedName { auth: schema.name.clone(), ident: ident.clone() }).collect()
}

/// [ISO/IEC 9075:1992 §11.5 GR1b v] The warning of the `<schema
/// definition>` `sql`, as `st` holds it, if a default it placed pads.
fn padding_of(st: &State, sql: &str) -> Option<SqlState> {
    Parser::new(sql).and_then(|p| p.of_definition().schema()).ok().and_then(|s| st.padding_of_schema(&s))
}
