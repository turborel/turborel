//! [ISO/IEC 9075-2:2023 §9.18, §10.4] A `<routine invocation>` of an
//! SQL-invoked function of an SQL routine body, bound (Feature T321): its
//! subject routine, its arguments, each assigned to its parameter's
//! declared type, and its body (`invocation/body.rs`).
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
use crate::catalog::{DataAccess, QualifiedName, RoutineKind, SqlRoutineDesc};
use crate::error::Result;
use crate::logical::{Expr, Invocation};
use crate::params::ParameterType;
use crate::value::DataType;
use crate::value::precedence::in_precedence_list;

mod body;
mod resolve;
#[cfg(test)]
mod tests;

pub use body::{FunctionBody, bind_function_body, routine_definition};
pub use resolve::resolve_routine;

/// The arguments of an invocation, bound, and the name and declared type
/// of each parameter.
type Arguments = (Vec<Expr>, Vec<(String, DataType)>);

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.4] The invocation of `name`, of schema
    /// `schema` if written, with `args`, in `scope`, if it invokes an
    /// SQL-invoked function of an SQL routine body: bound, and its result
    /// type. None if no such function is of the name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the function takes another number of arguments
    /// [§9.18 SR10a], an argument is not of a type its parameter takes
    /// [SR10d, SR10h v], or what contains the invocation may not invoke
    /// the function; -104 if the statement's authorization identifier
    /// holds no EXECUTE on it \[SR6\].
    pub(super) fn invocation(
        &self,
        (schema, name): (Option<&str>, &str),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<Option<(Expr, DataType)>> {
        // Every function call is first tried here: of a database of no
        // routine, none is invoked.
        if !self.db.catalog.any_routines() {
            return Ok(None);
        }
        // [§9.18 SR10b] of the schema written, else of the SQL-path, the
        // schema of the statement's names (ID047).
        let qn = QualifiedName { auth: schema.unwrap_or(self.schema).to_owned(), ident: name.to_owned() };
        let named = resolve::possibly_candidates(self.db, &qn, RoutineKind::Function);
        if named.is_empty() {
            return Ok(None);
        }
        let desc = resolve::subject((self.db, self.auth), &named, (&qn, RoutineKind::Function), args.len())?;
        self.permitted(desc)?;
        let (args, params) = self.routine_arguments(desc, args, scope)?;
        let result = desc.returns.clone().ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10e] {qn} is no function")))?;
        let definition = routine_definition(&desc.text)?;
        let cx = super::Context { uses: None, selected: None, referencing: None, draws: false, invoked: self.invoked, ..self.context() };
        let (bound, _) = body::bind_body(cx, (desc, &definition), body::chain_of(self.invoked))?;
        resolve::note(self.invoked, desc);
        let invocation = Invocation {
            name: desc.name.clone(),
            specific: desc.specific.clone(),
            args,
            params,
            body: bound.body,
            result: result.clone(),
            null_call: desc.null_call,
            deterministic: desc.deterministic,
        };
        Ok(Some((Expr::Invoked(Arc::new(invocation)), result)))
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR10d, SR10h v] The arguments `args` of
    /// an invocation of `desc`, bound in `scope`, a `?` of its parameter's
    /// declared type; and the name and type of each parameter.
    fn routine_arguments(&self, desc: &SqlRoutineDesc, args: &[ValueExpression], scope: Scope<'_>) -> Result<Arguments> {
        let mut bound = Vec::with_capacity(args.len());
        for (arg, p) in args.iter().zip(&desc.parameters) {
            let (e, t) = self.typed(arg, ParameterType::Of(p.data_type.clone()), scope)?;
            // [SR10d iv] its designator in the argument's type precedence
            // list, [SR10h v] and the argument assignable to it.
            if !in_precedence_list(&p.data_type, &t) || !super::assignable(&p.data_type, &t) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §9.18 SR10d] parameter {} of function {} is of {}, which an argument of {t} is not assignable to",
                    p.name, desc.name, p.data_type
                )));
            }
            bound.push(e);
        }
        let params = desc.parameters.iter().map(|p| (p.name.clone(), p.data_type.clone())).collect();
        Ok((bound, params))
    }

    /// That what the invocation is bound for may invoke `desc`: one that
    /// possibly modifies SQL-data nowhere (`invocation.rs`); in a
    /// constraint, none that possibly reads SQL-data or is possibly
    /// non-deterministic; in a generation expression, neither.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it may not.
    fn permitted(&self, desc: &SqlRoutineDesc) -> Result<()> {
        let name = &desc.name;
        if desc.access == DataAccess::Modifies {
            let rules = match self.purpose {
                Purpose::Query => "§7.17 SR25, §11.49 SR11",
                Purpose::Constraint | Purpose::UnderConstraint => "§11.9 SR6, §11.47 SR8",
                Purpose::Generation => "§11.4 SR10e, §4.35.2",
            };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {rules}] function {name} possibly modifies SQL-data, so it is not invoked here"
            )));
        }
        match self.purpose {
            Purpose::Query => Ok(()),
            Purpose::Constraint | Purpose::UnderConstraint => constraint_permits(desc),
            Purpose::Generation => generation_permits(desc),
        }
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
fn constraint_permits(desc: &SqlRoutineDesc) -> Result<()> {
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
fn generation_permits(desc: &SqlRoutineDesc) -> Result<()> {
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
