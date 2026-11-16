//! [ISO/IEC 9075-2:2023 §11.60, §11.62] CREATE FUNCTION, CREATE PROCEDURE
//! and DROP ROUTINE.
//!
//! A routine's body is bound as the routine is defined (`body.rs`), so its
//! Syntax and Access Rules are checked then, and what it names and the
//! routines it invokes are kept with its descriptor, for what is dropped
//! with what it names (`dependents.rs`). Its subject routines are fixed
//! then [§4.35.2]: a routine defined later, or dropped and defined again,
//! is none of them.
//!
//! \[DESIGN\] A routine's implicit specific name is its name, an
//! underscore, and its timestamp of creation [SR9s] (UV102), another
//! underscore and a count if a routine has that specific name already.

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::{Completion, Session, State, rule};
use crate::ast::{
    ParameterDeclaration, PrivilegeDefinition, PrivilegeObject, RoutineDefinition, RoutineDesignator, Schema, TableName, TypeName,
};
use crate::binder::{qualify, routine_definition};
use crate::catalog::{
    Horizon, ParameterMode, QualifiedName, Routine, SYSTEM_GRANTOR, SqlParameter, SqlRoutineDesc, UsageDesc, UserTypeDesc, owner_of,
};
use crate::error::{At, Code, Result, SqlError};
use crate::log::record::DropTarget;
use crate::txn::LockMode;
use crate::value::DataType;

/// A routine of a name, of a feature not supported.
fn overloaded(name: &QualifiedName, other: &str) -> SqlError {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §11.60 CR2] schema {} has {other} {} already: overloading (Feature T341) is not supported",
        name.auth, name.ident
    );
    SqlError::new(Code::Unsupported, msg)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.60] CREATE FUNCTION or CREATE PROCEDURE
    /// `r`, its names without a schema in `schema`: its routine descriptor
    /// added [GR3], and EXECUTE on it granted its owner by `_SYSTEM`
    /// [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks [SR9q, SR9t, SR9u, SR16,
    /// SR23]; -900 if a routine of its name is in its schema [CR2]; a
    /// parameter's type's, or its body's, bound.
    pub(in crate::db) fn create_routine(&mut self, schema: &str, r: &RoutineDefinition) -> Result<Defined> {
        let name = qualify(&r.name, schema);
        if !self.catalog.schema_exists(&name.auth) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR9p] there is no schema {}", name.auth)).or_at(r.name.at));
        }
        self.unique_name(&name).or_at(r.name.at)?;
        let owner = owner_of(&*self.catalog, &name.auth).to_owned();
        let created = self.catalog.routines().iter().map(|d| d.created).max().unwrap_or(0).saturating_add(1);
        let mut desc = SqlRoutineDesc {
            specific: self.specific_name(r, &name, created)?,
            name,
            kind: r.kind,
            parameters: self.parameters(r, (schema, &owner))?,
            returns: r.returns.as_ref().map(|t| self.parameter_type(t, (schema, &owner))).transpose()?,
            deterministic: r.deterministic,
            access: r.access,
            null_call: r.null_call,
            text: r.text.clone(),
            body: r.body_text.clone(),
            schema: schema.to_owned(),
            reads: Vec::new(),
            horizon: Horizon::default(),
            invokes: Vec::new(),
            execute: Vec::new(),
            created,
        };
        let bound = self.bind_routine_body(&desc, r)?;
        desc.horizon = Horizon::of(&*self.catalog, &bound.reads);
        (desc.reads, desc.invokes) = (bound.reads, bound.invokes);
        desc.execute = vec![UsageDesc { grantor: SYSTEM_GRANTOR.to_owned(), grantee: Some(owner), grantable: bound.grantable }];
        let locks = desc.reads.iter().map(|t| (t.clone(), LockMode::Shared)).collect();
        self.put_routine(desc);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.60 CR2] That no routine of schema of
    /// `name` is of its `<qualified identifier>`: no SQL-invoked routine,
    /// nor a function a distinct type's definition made.
    fn unique_name(&self, name: &QualifiedName) -> Result<()> {
        if !self.catalog.routines_named(name).is_empty() {
            return Err(overloaded(name, "a routine"));
        }
        let of_type = |t: &UserTypeDesc| {
            t.name.auth == name.auth && [Routine::ToDistinct, Routine::ToSource].iter().any(|w| t.routine(*w).name == name.ident)
        };
        if self.catalog.user_types().into_iter().any(of_type) {
            return Err(overloaded(name, "a distinct type's function"));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR9s, SR9t, SR9u] The specific name of
    /// `r`, the routine `name`, created as `created`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its `<specific name>` is of another schema, or a
    /// routine's of the schema already.
    fn specific_name(&self, r: &RoutineDefinition, name: &QualifiedName, created: u64) -> Result<QualifiedName> {
        let Some(written) = &r.specific else {
            let mut specific = QualifiedName { auth: name.auth.clone(), ident: format!("{}_{created}", name.ident) };
            let mut n: u64 = 0;
            while self.specific_taken(&specific) {
                n = n.saturating_add(1);
                specific.ident = format!("{}_{created}_{n}", name.ident);
            }
            return Ok(specific);
        };
        let specific = qualify(written, &name.auth);
        if specific.auth != name.auth {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR9t] specific name {specific} is not of schema {}", name.auth);
            return Err(rule(msg).or_at(written.at));
        }
        if self.specific_taken(&specific) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR9u] schema {} has a routine of specific name {} already",
                specific.auth, specific.ident
            ))
            .or_at(written.at));
        }
        Ok(specific)
    }

    /// Whether a routine of its schema has the specific name `specific`:
    /// an SQL-invoked routine, or a distinct type's function.
    fn specific_taken(&self, specific: &QualifiedName) -> bool {
        let of_type = |t: &UserTypeDesc| {
            t.name.auth == specific.auth
                && [Routine::ToDistinct, Routine::ToSource].iter().any(|w| t.routine(*w).specific == specific.ident)
        };
        self.catalog.routine(specific).is_some() || self.catalog.user_types().into_iter().any(of_type)
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR16] The SQL parameters of `r`, each
    /// IN unless written otherwise [SR16d], of its declared type.
    fn parameters(&self, r: &RoutineDefinition, (schema, owner): (&str, &str)) -> Result<Vec<SqlParameter>> {
        let declared = |p: &ParameterDeclaration| {
            let data_type = self.parameter_type(&p.data_type, (schema, owner)).or_at(p.at)?;
            Ok(SqlParameter { name: p.name.clone(), data_type, mode: p.mode.unwrap_or(ParameterMode::In) })
        };
        r.parameters.iter().map(declared).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR16, §6.1 AR1] The `<parameter type>`
    /// or `<returns data type>` `t`, a `<data type>`, no domain.
    fn parameter_type(&self, t: &TypeName, (schema, owner): (&str, &str)) -> Result<DataType> {
        match self.resolve_type(t, (schema, owner))? {
            (_, Some(d)) => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR16] a parameter's or result's type is a <data type>, not domain {}",
                d.name
            ))),
            (data_type, None) => Ok(data_type),
        }
    }

    /// The routines of `<schema definition>` `s`, of owner `owner`, made
    /// once its tables, views, grants and assertions are, in the order
    /// written, before its triggers, whose actions may invoke them; then
    /// its grants of EXECUTE on them: whether one granted less than it
    /// named.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is named in another schema [SR9q]; as
    /// `create_routine` and GRANT say.
    pub(in crate::db) fn schema_routines(&mut self, s: &Schema, owner: &str) -> Result<bool> {
        for r in &s.routines {
            if let Some(other) = r.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR9q] routine {} is in schema {other}, not {}", r.name.ident, s.name);
                return Err(rule(msg).or_at(r.name.at));
            }
            self.create_routine(&s.name, r)?;
        }
        let mut warned = false;
        for p in s.privileges.iter().filter(|p| of_schema_routine(s, p)) {
            warned |= self.grant_on_type(owner, &s.name, p).or_at(p.table.at)?.completion != Completion::Success;
        }
        Ok(warned)
    }

    /// [ISO/IEC 9075-2:2023 §12.7 SR3b] That routine `r` may still be
    /// defined as it is: its body binds, under its owner, with the
    /// privileges it needs, and within its SQL-data access indication.
    ///
    /// # Errors
    ///
    /// As `create_routine`'s, of its body.
    pub(in crate::db) fn routine_binds(&self, r: &SqlRoutineDesc) -> Result<()> {
        let definition = routine_definition(&r.text)?;
        self.bind_routine_body(r, &definition).map(drop)
    }

    /// The routines of specific names `specifics`, and each routine that
    /// invokes one, at any remove [§11.62 GR1]: those DROP ... CASCADE of
    /// them drops.
    pub(in crate::db) fn invokers_of(&self, specifics: &[QualifiedName]) -> Vec<QualifiedName> {
        let mut all: Vec<QualifiedName> = specifics.to_vec();
        let mut at = 0;
        while let Some(s) = all.get(at).cloned() {
            for r in self.catalog.routines() {
                if r.invokes.contains(&s) && !all.contains(&r.specific) {
                    all.push(r.specific.clone());
                }
            }
            at = at.saturating_add(1);
        }
        all
    }

    /// [ISO/IEC 9075-2:2023 §11.62] DROP of the routine of specific name
    /// `specific`: with CASCADE, each routine that invokes it, and each
    /// view, constraint, generated column and trigger that does, dropped
    /// too [GR1]; its descriptor, with its privileges, destroyed [GR6].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such routine, or without CASCADE a
    /// routine, view, constraint, generated column or trigger invokes it
    /// [SR4c].
    pub(in crate::db) fn drop_routine(&mut self, specific: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let Some(desc) = self.catalog.routine(specific).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.62 SR1] there is no routine of specific name {specific}")));
        };
        let gone = self.invokers_of(std::slice::from_ref(specific));
        let mut abandoned = self.invoking(&gone);
        abandoned.routines = gone.iter().filter(|s| *s != specific).cloned().collect();
        if !cascade && !abandoned.is_empty() {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §11.62 SR4c] {} invokes routine {}: drop it with CASCADE", abandoned.first(), desc.name);
            return Err(rule(msg));
        }
        let after = self.without_routines(&gone);
        abandoned.routines.clear();
        let views: Vec<QualifiedName> =
            abandoned.views.iter().flat_map(|v| std::iter::once(v.clone()).chain(self.views_reading(v))).collect();
        let (mut dropped, locks) = self.destroy(Arc::clone(&after.catalog), &abandoned)?;
        // What reads a view so dropped goes with it.
        let read: Vec<&QualifiedName> = views.iter().collect();
        dropped.absorb(self.drop_triggering(&read, (true, false))?);
        dropped.absorb(self.drop_routining(&read, (true, false))?);
        dropped.names.extend(locks.into_iter().map(|(name, _)| name));
        dropped.names.extend(gone);
        Ok(dropped)
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §11.62 SR1 to SR3, AR1] The routine
    /// `designator` names, in a schema the session's authorization
    /// identifier owns: one of its `<routine type>` [SR2, SR3].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names not exactly one routine; -104 if the
    /// session's authorization identifier does not own its schema [AR1].
    pub(in crate::db) fn routine_target(&self, st: &State, designator: &RoutineDesignator) -> Result<DropTarget> {
        let schema = self.schema.borrow().clone();
        let object = if designator.specific {
            PrivilegeObject::SpecificRoutine(designator.routine_type)
        } else {
            PrivilegeObject::Routine(designator.routine_type)
        };
        let named = (&designator.name, object, designator.parameters.as_deref(), None);
        let auth = self.auth().clone();
        let Some(desc) = st.sql_routine_named((&auth, &schema), named)? else {
            return Err(no_routine(&designator.name, &schema));
        };
        if !self.owns_schema(st, &desc.name.auth) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.62 AR1] routine {} is in schema {}, which {auth} does not own",
                desc.name.ident, desc.name.auth
            );
            return Err(SqlError::new(Code::Privilege, msg).or_at(designator.name.at));
        }
        Ok(DropTarget::Routine(desc.specific))
    }
}

/// Whether the `<grant statement>` `p` of `<schema definition>` `s` is of
/// a routine `s` defines, by its name or its specific name.
pub(in crate::db) fn of_schema_routine(s: &Schema, p: &PrivilegeDefinition) -> bool {
    let named = qualify(&p.table, &s.name);
    let of = |r: &RoutineDefinition| match p.object {
        PrivilegeObject::SpecificRoutine(_) => r.specific.as_ref().is_some_and(|n| qualify(n, &s.name) == named),
        PrivilegeObject::Routine(_) => qualify(&r.name, &s.name) == named,
        PrivilegeObject::Table
        | PrivilegeObject::Domain
        | PrivilegeObject::Collation
        | PrivilegeObject::Sequence
        | PrivilegeObject::Type => false,
    };
    s.routines.iter().any(of)
}

/// [ISO/IEC 9075-2:2023 §11.62 SR1, §10.6 SR3, SR4] No SQL-invoked routine
/// is of the name `name`, of schema `schema` if written without one.
fn no_routine(name: &TableName, schema: &str) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §10.6 SR3c] there is no routine {}", qualify(name, schema));
    rule(msg).or_at(name.at)
}
