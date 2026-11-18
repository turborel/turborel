//! [ISO/IEC 9075-2:2023 §9.18, §10.4] A `<routine invocation>` of a
//! function of a schema, bound: its subject routine among the functions of
//! its name (`invocation/candidates.rs`, Feature T341), then, of an
//! SQL-invoked function of an SQL routine body (Feature T321), its
//! arguments, each assigned to its parameter's declared type, and its body
//! (`invocation/body.rs`); of a distinct type's, its cast
//! (`binder/routines.rs`). Of none, the call is of a function registered
//! (RULES.md D53, D74).
//!
//! What contains the invocation decides which functions it may invoke.
//! One that possibly modifies SQL-data is in no query expression [§7.17
//! SR25], no search condition of a searched UPDATE or DELETE, a MERGE or
//! a trigger's WHEN [§14.9 SR6, §14.14 SR6, §14.12 SR13, §11.49 SR11],
//! no constraint [§11.9 SR6, §11.47 SR8] and no CASE abbreviation [§6.12
//! SR1a]. \[DESIGN\] The binder refuses it in every value expression,
//! the source of a SET clause and a row of an INSERT's VALUES too, as its
//! body is a `<return statement>`, which modifies nothing.

use std::sync::Arc;

use super::expression::nondeterministic_source;
use super::{Binder, Purpose, Scope, rule};
use crate::ast::ValueExpression;
use crate::catalog::{DataAccess, QualifiedName, SqlRoutineDesc};
use crate::error::{Result, SqlError};
use crate::logical::{Expr, Invocation};
use crate::params::ParameterType;
use crate::value::DataType;

mod body;
mod candidates;
mod resolve;
#[cfg(test)]
mod tests;

pub(crate) use body::bind_procedure_program;
pub use body::{FunctionBody, bind_function_body, definition_of, routine_definition, sql_statement};
pub use candidates::{Candidate, alike, functions_named, grantably_held, held};
pub(super) use resolve::note as note_specific;
pub use resolve::resolve_procedure;

use candidates::Chosen;
pub(super) use candidates::shown_types;

/// The arguments of an invocation, bound, and the name and declared type
/// of each parameter.
type Arguments = (Vec<Expr>, Vec<(String, DataType)>);

/// An argument bound, and its type: none of a `<dynamic parameter
/// specification>` until its function is chosen [§20.7 SR xxx].
pub(super) type Prebound = Vec<Option<(Expr, DataType)>>;

/// What an invocation of a name is, of the functions of its schema.
pub(super) enum Called {
    /// One of them, bound, and its result type.
    Done((Expr, DataType)),
    /// None: a function registered may be, of the arguments bound if they
    /// were (`Passed`).
    Pass(Passed),
}

/// The arguments of an invocation no function of its schema is, as bound
/// to choose among them, and why none of them is, if one is of the name.
#[derive(Default)]
pub(super) struct Passed {
    /// The arguments, bound if they were.
    pub(super) bound: Option<Prebound>,
    /// Why no function of the schema of the name is invoked, if one is of
    /// it: the error if no function registered is either.
    pub(super) refusal: Option<SqlError>,
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.4, §9.18 SR10] The invocation of `name`, of
    /// schema `schema` if written, with `args`, in `scope`, of a function of
    /// the schema, else of the statement's default schema, the SQL-path
    /// [SR10b] (ID047): an SQL-invoked function of an SQL routine body, or
    /// one of a distinct type's, the subject routine among those of the
    /// name (`candidates.rs`), bound with its result type. If none is,
    /// what a function registered of the name is given.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if more than one function of the name is alike [§9.6],
    /// an argument cannot be stored in its parameter [SR10l iii], or what
    /// contains the invocation may not invoke the function; -104 if the
    /// statement's authorization identifier holds no EXECUTE on the
    /// subject routine of a definition \[SR6\]; an argument's.
    pub(super) fn invocation(&self, (schema, name): (Option<&str>, &str), args: &[ValueExpression], scope: Scope<'_>) -> Result<Called> {
        let path = self.routine_schemas(schema);
        // [§9.18 SR5e] A polymorphic table function of a generic table or
        // descriptor parameter is invoked with a table or descriptor
        // argument, which no value expression's invocation has.
        let named: Vec<Candidate<'_>> = path
            .iter()
            .flat_map(|s| functions_named(self.db, &QualifiedName { auth: s.clone(), ident: name.to_owned() }))
            .filter(|c| !matches!(c, Candidate::Sql(r) if r.generic()))
            .collect();
        if named.is_empty() {
            return Ok(Called::Pass(Passed::default()));
        }
        let bound = self.prebound(args, scope)?;
        let types: Vec<Option<DataType>> = bound.iter().map(|b| b.as_ref().map(|(_, t)| t.clone())).collect();
        let shown = schema.map_or_else(|| name.to_owned(), |s| format!("{s}.{name}"));
        match self.subject_function(named, (&types, &path), &shown)? {
            Chosen::One(Candidate::Sql(desc)) => self.sql_invocation(desc, args, bound, scope).map(Called::Done),
            Chosen::One(Candidate::Cast(t, which)) => self.cast_invocation((t, which), args, bound, scope).map(Called::Done),
            Chosen::One(Candidate::Constructor(t)) => self.constructor_invocation(t).map(Called::Done),
            Chosen::Refused(e) => Ok(Called::Pass(Passed { bound: Some(bound), refusal: Some(e) })),
            Chosen::Nothing => Ok(Called::Pass(Passed { bound: Some(bound), refusal: None })),
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR10b; ISO/IEC 9075-4:2006 draft §8.1
    /// SR4] The schemas a function named here is of: `schema`, if written;
    /// else those of the SQL-path of the module of the routine whose body
    /// this is, if of one; else the statement's default schema's alone.
    fn routine_schemas(&self, schema: Option<&str>) -> Vec<String> {
        match schema {
            Some(s) => vec![s.to_owned()],
            None if !self.params.routine_path().is_empty() => self.params.routine_path().to_vec(),
            None => vec![self.schema.to_owned()],
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.18, §10.4] The invocation of `desc`, an
    /// SQL-invoked function of an SQL routine body, with `args`, those
    /// but a `?` bound as `bound`: its arguments, its body and its result
    /// type (Feature T321).
    fn sql_invocation(
        &self,
        desc: &SqlRoutineDesc,
        args: &[ValueExpression],
        bound: Prebound,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        self.permitted(desc)?;
        let (args, _) = self.routine_arguments(desc, args, bound, scope)?;
        let invocation = self.bound_invocation(desc, args)?;
        let result = invocation.result.clone();
        Ok((Expr::Invoked(Arc::new(invocation)), result))
    }

    /// [ISO/IEC 9075-2:2023 §9.18] The invocation of `desc`, an SQL-invoked
    /// function or method of an SQL routine body, of the arguments `args`,
    /// bound and stored in its parameters: its body bound, as the subject
    /// routine of what invokes it [§4.35.2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no function; its body's.
    pub(super) fn bound_invocation(&self, desc: &SqlRoutineDesc, args: Vec<Expr>) -> Result<Invocation> {
        let qn = &desc.name;
        let result = desc.returns.clone().ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10e] {qn} is no function")))?;
        let definition = definition_of(desc)?;
        let cx = super::Context { uses: None, selected: None, referencing: None, draws: false, invoked: self.invoked, ..self.context() };
        let (bound_body, _) = body::bind_body(cx, (desc, &definition), body::chain_of(self.invoked))?;
        resolve::note(self.invoked, &desc.specific);
        Ok(Invocation {
            name: desc.name.clone(),
            specific: desc.specific.clone(),
            args,
            params: desc.parameters.iter().map(|p| (p.name.clone(), p.data_type.clone())).collect(),
            body: bound_body.body,
            program: bound_body.program,
            modifies: desc.access == DataAccess::Modifies,
            result,
            null_call: desc.null_call,
            deterministic: desc.deterministic,
            overrides: matches!(desc.method, Some((_, crate::ast::MethodKind::Instance))).then(Vec::new),
            rust: None,
        })
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR10l iii, §20.7 SR xxx] The arguments
    /// `args` of an invocation of `desc`, bound as `bound`, each but a `?`,
    /// which takes its parameter's declared type; and the name and type of
    /// each parameter.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an argument cannot be stored in its parameter.
    fn routine_arguments(&self, desc: &SqlRoutineDesc, args: &[ValueExpression], bound: Prebound, scope: Scope<'_>) -> Result<Arguments> {
        let mut out = Vec::with_capacity(args.len());
        for ((arg, slot), p) in args.iter().zip(bound).zip(&desc.parameters) {
            let (e, t) = match slot {
                Some(given) => given,
                None => self.typed(arg, ParameterType::Of(p.data_type.clone()), scope)?,
            };
            let Some(e) = self.assigned((e, &t), &p.data_type)? else {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §9.18 SR10l iii, §9.2 SR2] parameter {} of function {} is of {}, which an argument of {t} cannot be stored in",
                    p.name, desc.name, p.data_type
                )));
            };
            out.push(e);
        }
        let params = desc.parameters.iter().map(|p| (p.name.clone(), p.data_type.clone())).collect();
        Ok((out, params))
    }

    /// That what the invocation is bound for may invoke `desc`: one that
    /// possibly modifies SQL-data only where the binder is hosting, at the
    /// top level of a value Part 2 lets invoke one; in a
    /// constraint, none that possibly reads SQL-data or is possibly
    /// non-deterministic; in a generation expression, neither.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it may not.
    pub(super) fn permitted(&self, desc: &SqlRoutineDesc) -> Result<()> {
        let name = &desc.name;
        if desc.access == DataAccess::Modifies && !(self.purpose == Purpose::Query && self.hosting.get()) {
            let rules = match self.purpose {
                Purpose::Query => "§7.17 SR25, §11.49 SR11",
                Purpose::Constraint | Purpose::UnderConstraint => "§11.9 SR6, §11.47 SR8",
                Purpose::Generation | Purpose::Key => "§11.4 SR10e, §4.35.2",
            };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {rules}] function {name} possibly modifies SQL-data, so it is not invoked here"
            )));
        }
        match self.purpose {
            Purpose::Query => Ok(()),
            Purpose::Constraint | Purpose::UnderConstraint => constraint_permits(desc),
            Purpose::Generation | Purpose::Key => generation_permits(desc),
        }
    }
}

impl Binder<'_> {
    /// \[EXTENSION\] That what the binder binds may call the registered
    /// function `name`, which `ScalarFunction::deterministic` says is
    /// `deterministic` or not (RULES.md D133). A CHECK, an assertion, a
    /// generation expression and a key part of an index call none that is
    /// not, as they invoke no such SQL-invoked function
    /// (`constraint_permits`, `generation_permits`) [ISO/IEC 9075-2:2023
    /// §11.9 SR5, CR2, §11.4 SR10d, §9.16 SR1am].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it may not.
    pub(super) fn registered_permitted(&self, name: &str, deterministic: bool) -> Result<()> {
        if deterministic {
            return Ok(());
        }
        let what = format!("a call of function {name}, registered, which is not deterministic");
        match self.purpose {
            Purpose::Query => Ok(()),
            Purpose::Constraint | Purpose::UnderConstraint => Err(nondeterministic_source("am", &what)),
            Purpose::Generation | Purpose::Key => {
                Err(rule(format!("[ISO/IEC 9075-2:2023 §11.4 SR10d, §9.16 SR1am] a generation expression shall not contain {what}")))
            }
        }
    }

    /// `bind` run with `hosting` saying whether what it binds may invoke a
    /// function that possibly modifies SQL-data; as it was after.
    pub(super) fn hosting<T>(&self, hosting: bool, bind: impl FnOnce() -> T) -> T {
        let before = self.hosting.replace(hosting);
        let bound = bind();
        self.hosting.set(before);
        bound
    }
}

/// [ISO/IEC 9075-2:2023 §11.9 CR2, CR3] That a CHECK or an assertion may
/// invoke `desc`: not possibly non-deterministic [§9.16 SR1am], as
/// Feature F672 is not claimed, nor possibly reading SQL-data, as Feature
/// F673 is not. \[DESIGN\] An assertion refuses such a function too: the
/// tables it reads are none whose changes evaluate the constraint.
///
/// # Errors
///
/// SQLCODE -103 if it may not.
pub(super) fn constraint_permits(desc: &SqlRoutineDesc) -> Result<()> {
    if !desc.deterministic {
        return Err(nondeterministic_source("am", &format!("an invocation of function {}, which is not deterministic", desc.name)));
    }
    if desc.access == DataAccess::Reads {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.9 CR3] a constraint's search condition shall not invoke function {}, which possibly reads SQL-data, as Feature F673 is not supported",
            desc.name
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.4 SR10d, SR10e] That a generation expression
/// may invoke `desc`: not possibly non-deterministic [§9.16 SR1am], nor
/// possibly reading SQL-data.
///
/// # Errors
///
/// SQLCODE -103 if it may not.
pub(super) fn generation_permits(desc: &SqlRoutineDesc) -> Result<()> {
    if !desc.deterministic {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.4 SR10d, §9.16 SR1am] a generation expression shall not invoke function {}, which is not deterministic",
            desc.name
        )));
    }
    if desc.access == DataAccess::Reads {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.4 SR10e] a generation expression shall not invoke function {}, which possibly reads SQL-data",
            desc.name
        )));
    }
    Ok(())
}
