//! SQL-server modules [ISO/IEC 9075-4:2006 draft §4.1, §9.21, §9.22]
//! (Feature P001): defined, of a schema statement or a `<schema
//! definition>`, and dropped.
//!
//! A module's routines are routines of its schema whose descriptors name
//! it, read in the module (`binder::definition_of`), which hold no
//! privilege of their own: EXECUTE on the module is what invokes them
//! [§8.1 SR1] (`binder::held`). Its declared local temporary tables are
//! the catalog's, in its own schema (`catalog::module_schema`), each
//! session with an instance of each (`db/temporary.rs`). A routine of it
//! that goes takes the module with it [§9.2 GR2a, §10.3 GR7]: DROP
//! MODULE, with CASCADE, is the one way any of it goes.

use std::slice;
use std::sync::Arc;

use super::alter::Made;
use super::constraints::Within;
use super::ddl::Dropped;
use super::definitions::Defined;
use super::routines::Home;
use super::sequences::Epochs;
use super::{State, rule};
use crate::ast::{ModuleDefinition, RoutineDefinition, Schema, TableDefinition, TableName};
use crate::binder::{qualify, routine_definition};
use crate::catalog::{ModuleDesc, QualifiedName, SYSTEM_GRANTOR, Stamps, UsageDesc, module_schema, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

mod privileges;

impl State {
    /// [ISO/IEC 9075-4:2006 draft §9.21] CREATE MODULE `m`, its name, if
    /// written without a schema, of schema `schema`: its descriptor, its
    /// declared tables, given `made`'s storage if given, the generators of
    /// their identity columns their epochs from `epochs`, and its routines,
    /// and EXECUTE on it granted the owner of its schema by `_SYSTEM`
    /// \[GR2\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks [SR2, SR5, §9.24 SR3;
    /// ISO/IEC 9075-2:2023 §10.3 SR1]; a table's or a routine's.
    pub(super) fn create_module(
        &mut self,
        (schema, zone): (&str, i16),
        m: &ModuleDefinition,
        (made, epochs): (Option<&Made>, &mut Epochs),
    ) -> Result<Defined> {
        let qn = qualify(&m.name, schema);
        self.new_module(&qn).or_at(m.name.at)?;
        // [SR7] its names without a schema are of its default schema.
        let names = m.schema.clone().unwrap_or_else(|| qn.auth.clone());
        if !self.catalog.schema_exists(&names) {
            let msg = format!("[ISO/IEC 9075-4:2006 draft §9.21 GR3c] there is no schema {names}, the default of module {qn}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(m.name.at));
        }
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let created = self.catalog.modules().iter().map(|d| d.created).max().unwrap_or(0).saturating_add(1);
        let mut desc = ModuleDesc {
            name: qn.clone(),
            schema: names.clone(),
            path: module_path(&qn, m.path.as_deref(), &self.catalog_name()).or_at(m.name.at)?,
            path_text: m.path_text.clone(),
            temporaries: Vec::new(),
            text: m.text.clone(),
            execute: vec![UsageDesc { grantor: SYSTEM_GRANTOR.to_owned(), grantee: Some(owner), grantable: false }],
            created,
            // [ISO/IEC 9075-4:2006 draft §19.4 Description 7, 8]
            stamps: Stamps::at(self.stamp),
        };
        // There first, so its routines are read with its SQL-path.
        self.put_module(desc.clone());
        let own = module_schema(&qn);
        let mut built = Made::default();
        for t in &m.temporaries {
            let (name, table) = self.module_table((&own, &names, zone), t, (made, epochs))?;
            built.tables.push((name.clone(), table));
            desc.temporaries.push(name);
        }
        let (mut locks, grantable) = self.module_routines(&desc, &m.routines)?;
        if let Some(system) = desc.execute.first_mut() {
            system.grantable = grantable;
        }
        self.put_module(desc);
        locks.push((QualifiedName::of_schema(&qn.auth), LockMode::AccessShare));
        Ok(Defined { made: Some(built), locks, ..Defined::default() })
    }

    /// That `qn` may name a module defined now: its schema is there, and
    /// holds no module of the name [§9.21 SR2].
    fn new_module(&self, qn: &QualifiedName) -> Result<()> {
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[ISO/IEC 9075-4:2006 draft §9.21 SR2] there is no schema {}", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        if self.catalog.module(qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075-4:2006 draft §9.21 SR2] schema {} has a module {} already", qn.auth, qn.ident)));
        }
        Ok(())
    }

    /// The routines `routines` of the module `desc`, each read again in it,
    /// so MODULE.T names its declared tables [§12.8 GR3b]: the locks they
    /// take, and whether the privileges their bodies need are all held
    /// grantably [§9.21 GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is named in another schema than the module's
    /// [§9.24 SR3]; a routine's, placed in the statement.
    fn module_routines(&mut self, desc: &ModuleDesc, routines: &[RoutineDefinition]) -> Result<(Vec<(QualifiedName, LockMode)>, bool)> {
        let (mut locks, mut grantable) = (Vec::new(), true);
        let home = Home { schema: &desc.name.auth, names: &desc.schema, module: Some(&desc.name.ident) };
        for written in routines {
            if let Some(other) = written.name.auth.as_ref().filter(|a| **a != desc.name.auth) {
                let msg = format!(
                    "[ISO/IEC 9075-4:2006 draft §9.24 SR3] routine {other}.{} is not of schema {}, the module's",
                    written.name.ident, desc.name.auth
                );
                return Err(rule(msg).or_at(written.name.at));
            }
            let r = routine_definition(&written.text, Some(desc.own_schema()))?;
            // Where the text read again begins in the statement.
            let by = written.name.at.saturating_sub(r.name.at);
            let (defined, held) = self.routine_in(home, &r).map_err(|e| e.shifted(by))?;
            locks.extend(defined.locks);
            grantable &= held;
        }
        Ok((locks, grantable))
    }

    /// [ISO/IEC 9075-4:2006 draft §12.8] The declared local temporary
    /// table `t` of a module, in its own schema `own`, its other names
    /// without a schema of schema `names`: described, in the catalog, given
    /// `made`'s storage if given; its name, and what it was given.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the module declares a table of its name already
    /// [ISO/IEC 9075:1992 §13.11 SR1], or for a rule its definition breaks.
    fn module_table(
        &mut self,
        (own, names, zone): (&str, &str, i16),
        t: &TableDefinition,
        (made, epochs): (Option<&Made>, &mut Epochs),
    ) -> Result<(QualifiedName, Made)> {
        let t = TableDefinition { name: TableName { auth: Some(own.to_owned()), ..t.name.clone() }, ..t.clone() };
        let qn = QualifiedName { auth: own.to_owned(), ident: t.name.ident.clone() };
        if self.catalog.table(&qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.11 SR1] the module declares MODULE.{} twice", qn.ident)).or_at(t.name.at));
        }
        let owner = owner_of(&*self.catalog, own).to_owned();
        let (mut desc, pending) = self.describe_table((own, &owner, names, zone, Within::Declared), &t, &[], &[]).or_at(t.name.at)?;
        self.give_epochs(&mut desc, epochs)?;
        desc.generation = self.retired.get(&qn).map_or(0, |g| g.saturating_add(1));
        let mut staged = self.catalog.duplicate();
        staged.add_table(desc.clone());
        let saved = std::mem::replace(&mut self.catalog, Arc::from(staged));
        let completed = self.complete_tables(names, slice::from_mut(&mut desc), slice::from_ref(&pending));
        self.catalog = saved;
        completed.or_at(t.name.at)?;
        self.check_new_objects(slice::from_ref(&desc))?;
        let mut catalog = self.catalog.duplicate();
        catalog.add_table(desc.clone());
        let table = self.place_table(&mut *catalog, &desc, made.and_then(|m| m.of_table(&qn)))?;
        self.catalog = Arc::from(catalog);
        Ok((qn, table))
    }

    /// The modules of `<schema definition>` `s`, made once its routines
    /// are, their declared tables' storage put in `built`: and whether a
    /// privilege on one it would grant was not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is named in another schema than `s` [§9.21
    /// SR1]; as `create_module`'s.
    pub(super) fn schema_modules(
        &mut self,
        s: &Schema,
        zone: i16,
        (made, epochs): (Option<&Made>, &mut Epochs),
        built: &mut Made,
    ) -> Result<bool> {
        for m in &s.modules {
            if let Some(other) = m.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!("[ISO/IEC 9075-4:2006 draft §9.21 SR1] module {} is in schema {other}, not {}", m.name.ident, s.name);
                return Err(rule(msg).or_at(m.name.at));
            }
            let defined = self.create_module((&s.name, zone), m, (made, epochs))?;
            built.tables.extend(defined.made.map(|d| d.tables).unwrap_or_default());
        }
        self.schema_module_grants(s)
    }

    /// [ISO/IEC 9075-4:2006 draft §9.22] DROP MODULE `qn`: its routines,
    /// with CASCADE each routine that invokes one and each view, constraint,
    /// generated column and trigger that does, and the modules of those
    /// routines, dropped too [GR1]; its descriptor, with its privileges,
    /// its routines and its declared tables, destroyed [GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such module [SR2], or, not `cascade`,
    /// another routine, a view, a constraint, a generated column or a
    /// trigger invokes one of its routines [SR3].
    pub(super) fn drop_module(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.module(qn).is_none() {
            return Err(rule(format!("[ISO/IEC 9075-4:2006 draft §9.22 SR2] there is no module {qn}")));
        }
        let own: Vec<QualifiedName> =
            self.catalog.routines().into_iter().filter(|r| r.module_name().as_ref() == Some(qn)).map(|r| r.specific.clone()).collect();
        let refused =
            |first: String| format!("[ISO/IEC 9075-4:2006 draft §9.22 SR3] {first} invokes a routine of module {qn}: drop it with CASCADE");
        let mut dropped = self.drop_invoked(&own, cascade, &refused)?;
        // A module whose routines are gone already.
        if self.catalog.module(qn).is_some() {
            dropped.absorb(self.remove_modules(slice::from_ref(qn))?);
        }
        Ok(dropped)
    }

    /// The routines of specific names `gone`, with every routine of a
    /// module one of them is of, and each routine that invokes one of
    /// those, at any remove: those a drop of them takes, as a routine of a
    /// module goes with its module [§9.2 GR2a, §10.3 GR7]; and those
    /// modules.
    pub(super) fn module_closure(&self, mut gone: Vec<QualifiedName>) -> (Vec<QualifiedName>, Vec<QualifiedName>) {
        let mut modules: Vec<QualifiedName> = Vec::new();
        loop {
            let found: Vec<QualifiedName> =
                gone.iter().filter_map(|s| self.catalog.routine(s)?.module_name()).filter(|m| !modules.contains(m)).collect();
            if found.is_empty() {
                return (gone, modules);
            }
            for m in found {
                let routines =
                    self.catalog.routines().into_iter().filter(|r| r.module_name().as_ref() == Some(&m)).map(|r| r.specific.clone());
                gone.extend(routines.filter(|s| !gone.contains(s)).collect::<Vec<_>>());
                if !modules.contains(&m) {
                    modules.push(m);
                }
            }
            gone = self.invokers_of(&gone);
        }
    }

    /// The descriptors of the modules `modules`, their routines gone
    /// already, destroyed, with their declared tables [§9.22 GR2]: what
    /// was taken away.
    ///
    /// # Errors
    ///
    /// As `drop_table`'s.
    pub(super) fn remove_modules(&mut self, modules: &[QualifiedName]) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for qn in modules {
            let Some(desc) = self.catalog.module(qn).cloned() else { continue };
            for t in &desc.temporaries {
                if self.catalog.table(t).is_some() {
                    dropped.absorb(self.drop_table(t, true)?);
                }
            }
            let mut catalog = self.catalog.duplicate();
            catalog.remove_module(qn);
            self.catalog = Arc::from(catalog);
            dropped.names.push(qn.clone());
        }
        Ok(dropped)
    }

    /// [ISO/IEC 9075-4:2006 draft §9.2 GR1, GR3] The modules DROP SCHEMA
    /// `auth` drops: those of the schema, and those whose default schema
    /// is it, or whose SQL-path names it.
    pub(super) fn modules_of_schema(&self, auth: &str) -> Vec<QualifiedName> {
        let naming = |m: &&ModuleDesc| m.name.auth == auth || m.schema == auth || m.path.iter().any(|s| s == auth);
        self.catalog.modules().into_iter().filter(naming).map(|m| m.name.clone()).collect()
    }

    /// Puts the module descriptor `desc` in the catalog.
    pub(super) fn put_module(&mut self, desc: ModuleDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_module(desc);
        self.catalog = Arc::from(catalog);
    }
}

/// [ISO/IEC 9075-4:2006 draft §9.21 SR4 to SR6] The SQL-path of the module
/// `qn` of the `<schema name list>` `written`, if one is, in the catalog
/// `ours`: the schemas it
/// names; else its own schema's alone, the implementation-defined path SR4
/// leaves.
///
/// # Errors
///
/// SQLCODE -103 if a schema of it is of another catalog [SR5], or it names
/// one twice [ISO/IEC 9075-2:2023 §10.3 SR1].
fn module_path(qn: &QualifiedName, written: Option<&[(Option<String>, String)]>, ours: &str) -> Result<Vec<String>> {
    let Some(list) = written else { return Ok(vec![qn.auth.clone()]) };
    let mut path: Vec<String> = Vec::new();
    for (catalog, schema) in list {
        if let Some(other) = catalog.as_ref().filter(|c| *c != ours) {
            let msg =
                format!("[ISO/IEC 9075-4:2006 draft §9.21 SR5] schema {other}.{schema} of the path is of another catalog than {ours}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_CATALOG_NAME));
        }
        if path.contains(schema) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.3 SR1] the path names schema {schema} twice")));
        }
        path.push(schema.clone());
    }
    Ok(path)
}
