//! [ISO/IEC 9075-2:2023 §11.60 SR16e, §9.18 SR9, GR3b ii] The default
//! values of a procedure's input parameters (Features T522 and T523).
//!
//! A `<parameter default>` is bound as the procedure is defined, as its
//! body is, under the owner of its schema, its names of the schema the
//! procedure's names are of: its value one its parameter's type takes by
//! store assignment [SR16e i, iii], what it names and invokes kept with
//! the routine's descriptor, for what is dropped with it [§11.23 SR7d,
//! §11.31 SR7f, §11.62 SR4c ii]. The descriptor keeps its text [GR3c vi].
//!
//! A CALL that takes it, by DEFAULT, by leaving its argument out past
//! those written, or by naming none of it [§9.18 SR9h i 1 B, 2 B], binds
//! that text again so, and evaluates it as the CALL's arguments are
//! evaluated [GR3b ii 2]. Of a parameter of no default, DEFAULT is the
//! null value [GR3b ii 1].
//!
//! The default sees none of the procedure's parameters, as their scope is
//! its body [§4.35.4], nor a host or dynamic parameter [§11.60 SR19].

use std::sync::Arc;

use super::super::State;
use super::super::rule;
use crate::ast::{ParameterDefault, RoutineDefinition, SqlArgument, ValueExpression};
use crate::binder::{Context, assigned_argument, bind_argument};
use crate::catalog::{Catalog, SqlParameter, SqlRoutineDesc, owner_of, routine_path};
use crate::error::Result;
use crate::logical::Expr;
use crate::params::{Params, RoutineScope};
use crate::parser::Parser;
use crate::value::Value;

/// The dynamic values a parameter default of `desc` is bound with: a
/// scope of the routine's name, path and horizon, of no parameter.
fn default_params(catalog: &dyn Catalog, desc: &SqlRoutineDesc) -> Params {
    let scope = RoutineScope { parameters: Vec::new(), ..RoutineScope::of(desc, routine_path(catalog, desc)) };
    Params::of_routine(Vec::new(), Arc::new(scope))
}

/// [ISO/IEC 9075-2:2023 §11.60 SR16e i, iii, §9.2 SR2] The default `value`
/// of the parameter `p`, bound in `cx`: the null value, or a value
/// expression of a type the parameter's takes by store assignment.
///
/// # Errors
///
/// SQLCODE -103 if its type is not one the parameter's takes; the value
/// expression's, bound.
fn bind_default(cx: Context<'_>, p: &SqlParameter, value: &SqlArgument) -> Result<Expr> {
    let SqlArgument::Value(v) = value else { return Ok(Expr::Const(Value::Null)) };
    let (bound, t) = bind_argument(cx, v, &p.data_type)?;
    if let Some(assigned) = assigned_argument(cx, (bound, &t), &p.data_type)? {
        return Ok(assigned);
    }
    Err(rule(format!(
        "[ISO/IEC 9075-2:2023 §11.60 SR16e i, §9.2 SR2] the <parameter default> of parameter {} ({}) is a {t}, which cannot be stored \
         in it",
        p.name, p.data_type
    )))
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3b ii] The value of the parameter `p` of
/// `desc` given DEFAULT by a CALL bound in `cx`: its `<parameter
/// default>`, bound as the procedure was defined; else the null value.
///
/// # Errors
///
/// The default's, bound as `bind_default` binds it.
pub(super) fn default_input(cx: Context<'_>, desc: &SqlRoutineDesc, p: &SqlParameter) -> Result<Expr> {
    let Some(text) = &p.default else { return Ok(Expr::Const(Value::Null)) };
    let value = Parser::new(text).and_then(|mut parser| parser.kept_default())?;
    let owner = owner_of(&*cx.db.catalog, &desc.name.auth).to_owned();
    let params = default_params(&*cx.db.catalog, desc);
    let inner = Context { schema: &desc.schema, horizon: &desc.horizon, now: cx.now, own: cx.own, ..Context::new(cx.db, &owner, &params) };
    bind_default(inner, p, &value)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.60 SR16e] Each `<parameter default>` of
    /// `definition`, of the routine `desc`, bound in `cx`, the context its
    /// body is bound in, so that what it names and invokes is noted where
    /// the body's is.
    ///
    /// # Errors
    ///
    /// As `bind_default`'s.
    pub(super) fn bind_defaults(&self, cx: Context<'_>, desc: &SqlRoutineDesc, definition: &RoutineDefinition) -> Result<()> {
        let params = default_params(&*self.catalog, desc);
        let cx = Context { params: &params, ..cx };
        for (declared, p) in definition.parameters.iter().zip(&desc.parameters) {
            if let Some(d) = &declared.default {
                bind_default(cx, p, &d.value)?;
            }
        }
        Ok(())
    }
}

/// The value expressions of the `<parameter default>`s of `definition`.
pub(super) fn default_values(definition: &RoutineDefinition) -> impl Iterator<Item = &ValueExpression> {
    definition
        .parameters
        .iter()
        .filter_map(|p| if let Some(ParameterDefault { value: SqlArgument::Value(v), .. }) = &p.default { Some(v) } else { None })
}
