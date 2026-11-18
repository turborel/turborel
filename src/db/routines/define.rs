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
    ParameterDeclaration, PrivilegeDefinition, PrivilegeObject, RoutineBody, RoutineDefinition, RoutineDesignator, RoutineType, Schema,
    SchemaStatement, TableName, TypeName,
};
use crate::binder::{alike, definition_of, qualify};
use crate::catalog::{
    Horizon, ParameterMode, QualifiedName, RoutineKind, SYSTEM_GRANTOR, SqlParameter, SqlRoutineDesc, UsageDesc, owner_of, sole,
};
use crate::error::{At, Code, Result, SqlError};
use crate::log::record::DropTarget;
use crate::txn::LockMode;
use crate::value::DataType;

/// The types `types`, as a message lists them.
fn listed(types: &[DataType]) -> String {
    types.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

/// Where a routine is defined: the schema its name is of if written
/// without one, `schema`; that of the other names its definition writes
/// without one, `names`; and the SQL-server module it is of, if one
/// [ISO/IEC 9075-4:2006 draft §9.21 SR7, §9.24 SR3].
#[derive(Clone, Copy)]
pub(in crate::db) struct Home<'a> {
    pub(in crate::db) schema: &'a str,
    pub(in crate::db) names: &'a str,
    pub(in crate::db) module: Option<&'a str>,
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
        self.routine_in(Home { schema, names: schema, module: None }, r).map(|(defined, _)| defined)
    }

    /// The routine `r` defined in `home`: as `create_routine` defines it,
    /// and of a module, of its module, with no privilege of its own [§11.60
    /// GR1]. Whether the privileges its body needs are held grantably.
    ///
    /// # Errors
    ///
    /// As `create_routine`'s.
    pub(in crate::db) fn routine_in(&mut self, home: Home<'_>, r: &RoutineDefinition) -> Result<(Defined, bool)> {
        let Home { schema, names, module } = home;
        let name = qualify(&r.name, schema);
        if !self.catalog.schema_exists(&name.auth) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR9p] there is no schema {}", name.auth)).or_at(r.name.at));
        }
        let owner = owner_of(&*self.catalog, &name.auth).to_owned();
        let parameters = self.parameters(r, (names, &owner))?;
        self.told_apart(&name, r.kind, &parameters).or_at(r.name.at)?;
        if r.kind == RoutineKind::Function {
            let types: Vec<DataType> = parameters.iter().map(|p| p.data_type.clone()).collect();
            self.told_apart_from_methods(&name, &types).or_at(r.name.at)?;
        }
        let created = self.catalog.routines().iter().map(|d| d.created).max().unwrap_or(0).saturating_add(1);
        let mut desc = SqlRoutineDesc {
            method: None,
            distance: r.distance.clone(),
            specific: self.specific_name(r, &name, created)?,
            name,
            kind: r.kind,
            parameters,
            returns: r.returns.as_ref().map(|t| self.parameter_type(t, (names, &owner))).transpose()?,
            deterministic: r.deterministic,
            access: r.access,
            null_call: r.null_call,
            text: r.text.clone(),
            body: r.body_text.clone(),
            schema: names.to_owned(),
            reads: Vec::new(),
            horizon: Horizon::default(),
            sequences: Vec::new(),
            execute: Vec::new(),
            created,
            module: module.map(str::to_owned),
            ptf: match &r.body {
                RoutineBody::Polymorphic(def) => Some(Box::new(self.ptf_desc(def, (names, &owner))?)),
                RoutineBody::Sql(_) => None,
            },
        };
        let bound = self.bind_routine_body(&desc, r)?;
        desc.horizon = Horizon::of(&*self.catalog, &bound.reads).invoking(bound.invokes);
        (desc.reads, desc.sequences) = (bound.reads, bound.sequences);
        super::super::vectors::distance_defined(self, &desc).or_at(r.name.at)?;
        // [ISO/IEC 9075-4:2006 draft §9.21 GR2] of a module's, the
        // privilege is on the module.
        if module.is_none() {
            desc.execute = vec![UsageDesc { grantor: SYSTEM_GRANTOR.to_owned(), grantee: Some(owner), grantable: bound.grantable }];
        }
        let locks = desc.reads.iter().map(|t| (t.clone(), LockMode::Shared)).collect();
        self.put_routine(desc);
        Ok((Defined { locks, ..Defined::default() }, bound.grantable))
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR20] That a routine `name` of kind
    /// `kind` and of the SQL parameters `parameters` is told apart from the
    /// other routines of its name in its schema (Feature T341): a procedure
    /// from each procedure of as many parameters [SR20a]; a function from
    /// each function, an SQL-invoked function's or a distinct type's, by
    /// the types of its parameters [SR20b].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not.
    fn told_apart(&self, name: &QualifiedName, kind: RoutineKind, parameters: &[SqlParameter]) -> Result<()> {
        match kind {
            RoutineKind::Procedure => {
                let arity = |d: &&SqlRoutineDesc| d.kind == RoutineKind::Procedure && d.parameters.len() == parameters.len();
                match self.catalog.routines_named(name).into_iter().find(arity) {
                    Some(other) => Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §11.60 SR20a] schema {} has procedure {} of {} parameters already, of specific name {}",
                        name.auth,
                        name.ident,
                        parameters.len(),
                        other.specific
                    ))),
                    None => Ok(()),
                }
            }
            RoutineKind::Function => {
                let types: Vec<DataType> = parameters.iter().map(|p| p.data_type.clone()).collect();
                match alike(self, name, &types) {
                    Some(other) => Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §11.60 SR20b] function {name} ({}) is not told apart from function {} ({}), as an invocation of arguments of its types would invoke either",
                        listed(&types),
                        other.specific(),
                        listed(&other.parameters())
                    ))),
                    None => Ok(()),
                }
            }
        }
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
        // And a structured type's routines and method specifications'
        // [§11.51 SR9b vii] (`db/structured.rs`).
        self.specific_in_use(specific)
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR16] The SQL parameters of `r`, each
    /// IN unless written otherwise [SR16d], of its declared type.
    fn parameters(&self, r: &RoutineDefinition, (schema, owner): (&str, &str)) -> Result<Vec<SqlParameter>> {
        let declared = |p: &ParameterDeclaration| {
            let (data_type, generic) = self.ptf_parameter(&p.data_type, (schema, owner)).or_at(p.at)?;
            Ok(SqlParameter { name: p.name.clone(), data_type, mode: p.mode.unwrap_or(ParameterMode::In), generic })
        };
        r.parameters.iter().map(declared).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR16, §6.1 AR1] The `<parameter type>`
    /// or `<returns data type>` `t`, a `<data type>`, no domain.
    pub(super) fn parameter_type(&self, t: &TypeName, (schema, owner): (&str, &str)) -> Result<DataType> {
        match self.resolve_type(t, (schema, sole(&*self.catalog, owner)))? {
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
        for (at, r) in s.routines.iter().enumerate() {
            self.schema_defined((s, owner), at)?;
            if let Some(other) = r.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR9q] routine {} is in schema {other}, not {}", r.name.ident, s.name);
                return Err(rule(msg).or_at(r.name.at));
            }
            self.create_routine(&s.name, r)?;
        }
        self.schema_defined((s, owner), s.routines.len())?;
        let mut warned = false;
        for p in s.privileges.iter().filter(|p| of_schema_routine(s, p)) {
            warned |= self.grant_on_type(owner, &s.name, p).or_at(p.table.at)?.completion != Completion::Success;
        }
        Ok(warned)
    }

    /// [ISO/IEC 9075-2:2023 §12.7 SR3b, GR12o] That routine `r` may still
    /// be defined as it is: its owner holds USAGE on its parameters' and
    /// result's types [§6.1 AR1], and its body binds, under its owner, with
    /// the privileges it needs, and within its SQL-data access indication.
    ///
    /// # Errors
    ///
    /// As `create_routine`'s, of its types and body.
    pub(in crate::db) fn routine_binds(&self, r: &SqlRoutineDesc) -> Result<()> {
        let definition = definition_of(r)?;
        let owner = owner_of(&*self.catalog, &r.name.auth).to_owned();
        self.parameters(&definition, (&r.schema, &owner))?;
        if let Some(t) = &definition.returns {
            self.parameter_type(t, (&r.schema, &owner))?;
        }
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
                if r.invokes().contains(&s) && !all.contains(&r.specific) {
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
    /// [SR4c], or it is the function of an ordering, a transform or a cast
    /// [SR4a, SR4d].
    pub(in crate::db) fn drop_routine(&mut self, specific: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let Some(desc) = self.catalog.routine(specific).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.62 SR1] there is no routine of specific name {specific}")));
        };
        // [ISO/IEC 9075-4:2006 draft §9.2 GR2a, §10.3 GR7] with its module.
        if let Some(module) = desc.module_name() {
            return self.drop_module(&module, cascade);
        }
        // [§11.62 SR4a, SR4d, GR2 to GR4] what it is the function of.
        let mut dropped = self.drop_function_uses(specific, cascade)?;
        let refused =
            |first: String| format!("[ISO/IEC 9075-2:2023 §11.62 SR4c] {first} invokes routine {}: drop it with CASCADE", desc.name);
        dropped.absorb(self.drop_invoked(std::slice::from_ref(specific), cascade, &refused)?);
        Ok(dropped)
    }

    /// The routines of specific names `own` dropped, with CASCADE each
    /// routine that invokes one and each view, constraint, generated
    /// column and trigger that does, and the modules of those routines, as
    /// `module_closure` says: what was taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if anything invokes one of them, as
    /// `refused` of the first says.
    pub(in crate::db) fn drop_invoked(
        &mut self,
        own: &[QualifiedName],
        cascade: bool,
        refused: &dyn Fn(String) -> String,
    ) -> Result<Dropped> {
        let (gone, modules) = self.module_closure(self.invokers_of(own));
        let mut abandoned = self.invoking(&gone);
        abandoned.routines = gone.iter().filter(|s| !own.contains(s)).cloned().collect();
        if !cascade && !abandoned.is_empty() {
            return Err(rule(refused(abandoned.first())));
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
        dropped.absorb(self.remove_modules(&modules)?);
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
        // [§10.6 SR3b] METHOD ... FOR a type, of the type's methods.
        let found = match &designator.udt {
            Some(udt) => st.method_designated((&auth, &schema), named, udt)?,
            None => st.sql_routine_named((&auth, &schema), named)?,
        };
        let Some(desc) = found else {
            return Err(no_routine(&designator.name, &schema));
        };
        if let Some(module) = desc.module_name() {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.62 SR2] routine {} is of module {module}, not a schema-level routine: it goes with DROP MODULE",
                desc.name
            );
            return Err(rule(msg).or_at(designator.name.at));
        }
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
        | PrivilegeObject::Module
        | PrivilegeObject::Model
        | PrivilegeObject::Type => false,
    };
    // [ISO/IEC 9075-2:2023 §11.1] of a method of the schema, once it has
    // its body.
    let method = p.udt.is_some()
        || matches!(p.object, PrivilegeObject::SpecificRoutine(RoutineType::Method(_)) | PrivilegeObject::Routine(RoutineType::Method(_)));
    let defines_methods = s.defined.iter().any(|(_, d)| matches!(d, SchemaStatement::Method(_)));
    s.routines.iter().any(of) || (method && defines_methods)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.1] The methods, casts, orderings and
    /// transforms of `<schema definition>` `s`, owned by `owner`, written
    /// after `at` of its routines and before the next: each made in the
    /// schema, as it would be alone.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a method is of a type of another schema [§11.60
    /// SR9q]; -104 if `owner` owns not the schema of the type of a cast,
    /// ordering or transform [§11.63 AR2, §11.65 AR1, §11.67 AR1]; as each
    /// one's.
    fn schema_defined(&mut self, (s, owner): (&Schema, &str), at: usize) -> Result<()> {
        for (_, d) in s.defined.iter().filter(|(before, _)| *before == at) {
            if let (Some(n), cited) = super::super::structured::type_named(d) {
                let qn = qualify(n, &s.name);
                if matches!(d, SchemaStatement::Method(_)) && qn.auth != s.name {
                    let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR9q] method of {qn} is in schema {}, not {}", qn.auth, s.name);
                    return Err(rule(msg).or_at(n.at));
                }
                if owner_of(&*self.catalog, &qn.auth) != owner {
                    let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {qn} is in schema {}, which {owner} does not own", qn.auth);
                    return Err(SqlError::new(Code::Privilege, msg).or_at(n.at));
                }
            }
            self.schema_element_for_type(&s.name, d)?;
        }
        Ok(())
    }

    /// The method, cast, ordering or transform `d` of schema `schema` made.
    fn schema_element_for_type(&mut self, schema: &str, d: &SchemaStatement) -> Result<()> {
        match d {
            SchemaStatement::Method(m) => self.create_method(schema, m).map(drop),
            SchemaStatement::Cast(c) => self.create_cast(schema, c).map(drop),
            SchemaStatement::Ordering(o) => self.create_ordering(schema, o).map(drop),
            SchemaStatement::Transform(t) => self.create_transform(schema, t).map(drop),
            SchemaStatement::Table(_)
            | SchemaStatement::View(_)
            | SchemaStatement::Grant(_)
            | SchemaStatement::Revoke(_)
            | SchemaStatement::Role(_)
            | SchemaStatement::Domain(_)
            | SchemaStatement::Assertion(_)
            | SchemaStatement::Collation(_)
            | SchemaStatement::Sequence(_)
            | SchemaStatement::AlterSequence(_)
            | SchemaStatement::AlterDomain(_)
            | SchemaStatement::Type(_)
            | SchemaStatement::Trigger(_)
            | SchemaStatement::Routine(_)
            | SchemaStatement::Module(_)
            | SchemaStatement::AlterType(_)
            | SchemaStatement::AlterTransform(_)
            | SchemaStatement::Vector(_) => {
                Err(rule("[DESIGN] a schema element defined for a type is a method, cast, ordering or transform"))
            }
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.62 SR1, §10.6 SR3, SR4] No SQL-invoked routine
/// is of the name `name`, of schema `schema` if written without one.
fn no_routine(name: &TableName, schema: &str) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §10.6 SR3c] there is no routine {}", qualify(name, schema));
    rule(msg).or_at(name.at)
}
