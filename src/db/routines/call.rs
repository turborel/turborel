//! [ISO/IEC 9075-2:2023 §16.1, §9.18, §10.4] CALL of an SQL-invoked
//! procedure (Feature T321).
//!
//! The subject routine is the procedure of the name with as many
//! parameters as there are arguments, on which the current authorization
//! identifier holds EXECUTE [§9.18 SR6, SR9]. Each argument is as its
//! parameter's mode allows [SR9h iii, iv]: a value expression of an input
//! parameter; a target specification of an output one, or of one both.
//! NULL or DEFAULT of an input parameter, and DEFAULT of one both, are of
//! Features T522 and T523, not supported [§10.4 CR6, CR7]. A target is a
//! host parameter, an SQL parameter of the procedure whose body the CALL
//! is, or a column of a BEFORE trigger's new transition variable [§6.4];
//! or, of a prepared CALL, a `?`, an output dynamic parameter, or one
//! both input and output, of the parameter's type [§20.7 GR7a, SR5a].
//!
//! The arguments are evaluated, each value of an input parameter assigned
//! to its type, an output parameter's null [§9.18 GR3, GR4c]. The body's
//! statement then runs in the context of the procedure (`db/nested.rs`),
//! as its owner, its parameters its dynamic values (`params/routine.rs`)
//! [GR6, GR8]. Each output parameter's value is last assigned to its
//! target [GR10b], a `?`'s its dynamic value. The procedure runs within the CALL statement, so its
//! statements share its instant [GR6n] (US).

use std::sync::Arc;

use super::super::assignment::{assign_targets, check_targets};
use super::super::compiled::plan;
use super::super::savepoints::Level;
use super::super::{Completion, Session, State, StatementContext, rule};
use super::program::{Hosted, hosted_rows};
use crate::ast::{
    CallStatement, Composite, ElementPosition, ParameterSpecification, SqlArgument, TargetKind, ValueExpression, ValueSpecification,
};
use crate::binder::named::{all_or_none, arranged};
use crate::binder::{Context, OutColumn, bind_argument, qualify, resolve_procedure};
use crate::catalog::{DataAccess, ParameterMode, RoutineKind, SqlParameter, SqlRoutineDesc, owner_of, routine_path};
use crate::error::{At, Code, Result, SqlError, Subject};
use crate::exec::execute;
use crate::logical::{Expr, LogicalPlan};
use crate::params::{ParameterType, Params, RoutineScope};
use crate::storage::Row;
use crate::value::{Value, assign};

/// [ISO/IEC 9075-2:2023 §16.1] A CALL, bound.
pub(crate) struct BoundCall {
    /// The subject routine, as the catalog holds it.
    pub(crate) desc: Arc<SqlRoutineDesc>,
    /// The value each parameter is given, in order: its argument's, else
    /// the null value.
    pub(crate) inputs: Vec<Expr>,
    /// Each output parameter's ordinal position, from 0, and the target
    /// its value is assigned to.
    pub(crate) targets: Vec<(usize, ParameterSpecification)>,
    /// [ISO/IEC 9075-2:2023 §20.7 GR7a] Each output parameter's ordinal
    /// position, from 0, whose argument is a `?`, and that `?`'s place
    /// among the statement's.
    pub(crate) dynamic: Vec<(usize, usize)>,
}

/// The target of an output parameter's argument.
enum Target {
    /// A `<target specification>` other than a `?`.
    Spec(ParameterSpecification),
    /// [ISO/IEC 9075-2:2023 §6.4, §20.7 GR7a] A `<dynamic parameter
    /// specification>` of a prepared CALL: its place among the
    /// statement's.
    Dynamic(usize),
}

/// [ISO/IEC 9075-2:2023 §6.4, §20.7 GR7a] The place of the `?` the
/// argument `v` is, possibly in parentheses, if it is one.
fn dynamic_of(v: &ValueExpression) -> Option<usize> {
    if let ValueExpression::Nested(inner) = v {
        return dynamic_of(inner);
    }
    if let ValueExpression::Value(ValueSpecification::Dynamic { index, .. }) = v { Some(*index) } else { None }
}

/// The output parameters' targets of a CALL being bound.
#[derive(Default)]
struct Outputs {
    targets: Vec<(usize, ParameterSpecification)>,
    dynamic: Vec<(usize, usize)>,
}

impl Outputs {
    /// The output parameter `p`, the `k`-th, from 0, assigned to `t` in
    /// `cx`, a `?` typed as `p` is [§20.7 SR5a].
    fn push(&mut self, cx: Context<'_>, (k, p): (usize, &SqlParameter), t: Target) -> Result<()> {
        match t {
            Target::Spec(spec) => self.targets.push((k, spec)),
            Target::Dynamic(index) => {
                drop(cx.params.dynamic_typed(index, Some(ParameterType::Of(p.data_type.clone())))?);
                self.dynamic.push((k, index));
            }
        }
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR9h iii] The `<target specification>` the
/// argument `v` is, in `cx`: a host parameter, an SQL parameter of the
/// procedure whose body it is in, a column of a BEFORE trigger's new
/// transition variable, or an element of the array either of the last two
/// holds [§6.4]. None if it is none of them.
fn target_of(cx: Context<'_>, v: &ValueExpression) -> Option<ParameterSpecification> {
    if let ValueExpression::Value(ValueSpecification::Parameter(p)) = v {
        return Some(p.clone());
    }
    if let ValueExpression::Nested(inner) = v {
        return target_of(cx, inner);
    }
    if let ValueExpression::Composite(c, _) = v {
        let Composite::Element(array, index) = &**c else { return None };
        let mut spec = target_of(cx, array).filter(|s| s.kind != TargetKind::Parameter && s.element.is_none())?;
        spec.element = Some(Box::new(position_of(cx, index)?));
        return Some(spec);
    }
    let ValueExpression::Column(cs) = v else { return None };
    if cs.catalog.is_some() {
        return None;
    }
    let qualifier = cs.qualifier.as_ref().filter(|q| q.auth.is_none()).map(|q| q.ident.as_str());
    if cs.qualifier.as_ref().is_some_and(|q| q.auth.is_some()) {
        return None;
    }
    let spec = |kind| Some(ParameterSpecification::of(cs.column.clone(), kind, cs.at));
    if cx.params.routine_scope().is_some_and(|s| s.parameter(qualifier, &cs.column).is_some()) {
        return spec(TargetKind::Routine);
    }
    // [SR9h iii 3] Not of an AFTER trigger, whose new row is not assigned.
    let transitions = cx.params.transitions().filter(|t| t.assigns)?;
    (qualifier.is_some() && transitions.new_row.as_deref() == qualifier).then(|| spec(TargetKind::Transition)).flatten()
}

/// [ISO/IEC 9075-2:2023 §6.4] The `<simple value specification>` `v`, the
/// position of an element of a target array, in `cx`: a `<literal>`, its
/// sign written against its digits, or an `<SQL parameter reference>` of
/// the procedure whose body it is in. None if it is neither.
fn position_of(cx: Context<'_>, v: &ValueExpression) -> Option<ElementPosition> {
    if let ValueExpression::Value(ValueSpecification::Literal(l)) = v {
        return Some(ElementPosition::Literal(l.clone()));
    }
    let ValueExpression::Column(cs) = v else { return None };
    if cs.catalog.is_some() || cs.qualifier.as_ref().is_some_and(|q| q.auth.is_some()) {
        return None;
    }
    let qualifier = cs.qualifier.as_ref().map(|q| q.ident.as_str());
    cx.params.routine_scope()?.parameter(qualifier, &cs.column).map(|_| ElementPosition::Parameter(cs.column.clone()))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR9h iii 1, 2] An argument of an output
/// parameter that is no target specification.
fn no_target<T>(desc: &SqlRoutineDesc, (k, both): (usize, bool), a: &SqlArgument) -> Result<T> {
    let (rule_of, may) = if both { ("SR9h iii 2", "a target specification or DEFAULT") } else { ("SR9h iii 1", "a target specification") };
    let name = desc.parameters.get(k).map_or("", |p| p.name.as_str());
    let refused = Err(rule(format!(
        "[ISO/IEC 9075-2:2023 §9.18 {rule_of}] the argument of parameter {name} of procedure {} shall be {may}",
        desc.name
    )));
    match a {
        SqlArgument::Value(_) => refused,
        SqlArgument::Default(at) | SqlArgument::Null(at) => refused.or_at(*at),
    }
}

/// [ISO/IEC 9075-2:2023 §10.4 CR6, CR7] A `<contextually typed value
/// specification>` of an input parameter, or DEFAULT of one both input
/// and output: the null value, as no parameter has a default [§9.18
/// GR3b ii 1], but of Feature `feature`, not supported.
pub(super) fn defaulted<T>(feature: &str, rule_of: u8, at: usize) -> Result<T> {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §10.4 CR{rule_of}] NULL or DEFAULT as an argument of an input parameter of CALL is of Feature {feature}, not supported"
    );
    Err(SqlError::new(Code::Unsupported, msg)).or_at(at)
}

/// [ISO/IEC 9075-2:2023 §9.18 SR9h iv, v, §9.2 SR2] The value expression
/// `v` of the input parameter `p`, bound in `cx`: its type one the
/// parameter's may take by store assignment, a `?` of the parameter's.
fn input(cx: Context<'_>, p: &SqlParameter, v: &ValueExpression) -> Result<Expr> {
    let (bound, t) = bind_argument(cx, v, &p.data_type)?;
    if let Some(assigned) = crate::binder::assigned_argument(cx, (bound, &t), &p.data_type)? {
        return Ok(assigned);
    }
    Err(rule(format!(
        "[ISO/IEC 9075-2:2023 §9.18 SR9h v, §9.2 SR2] a {t} argument cannot be stored in parameter {} ({})",
        p.name, p.data_type
    )))
}

impl State {
    /// [ISO/IEC 9075-2:2023 §16.1, §9.18 SR9] `call` bound in `cx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no procedure of the name takes its arguments, or an
    /// argument is not as its parameter's mode allows; -104 without
    /// EXECUTE on it; an argument's, bound.
    pub(crate) fn bind_call(cx: Context<'_>, call: &CallStatement) -> Result<BoundCall> {
        all_or_none(&call.names)?;
        let desc = resolve_procedure(cx, &call.name, call.arguments.len())?;
        let parameters: Vec<&str> = desc.parameters.iter().map(|p| p.name.as_str()).collect();
        let Some(arguments) = arranged(&parameters, &call.arguments, &call.names) else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR5d] the arguments of the CALL do not name each parameter of procedure {}",
                desc.name
            )));
        };
        let mut outputs = Outputs::default();
        let mut inputs = Vec::new();
        for (k, (p, a)) in desc.parameters.iter().zip(arguments).enumerate() {
            inputs.push(argument(cx, (&desc, k, p), a, &mut outputs)?);
        }
        let Outputs { targets, dynamic } = outputs;
        let columns: Vec<OutColumn> = targets.iter().filter_map(|(k, _)| desc.parameters.get(*k)).map(out_column).collect();
        let specs: Vec<ParameterSpecification> = targets.iter().map(|(_, t)| t.clone()).collect();
        // [SR9h iii 4, 5] Each output parameter's type assignable to its target.
        check_targets(&columns, &specs, cx.params, "§9.18 SR9h iii", "§9.18 SR9h iii 4")?;
        Ok(BoundCall { desc, inputs, targets, dynamic })
    }

    /// Whether `call`, its names in schema `schema`, invokes a procedure
    /// that possibly modifies SQL-data: the one of the name of as many
    /// parameters as it has arguments [§9.18 SR9, §11.60 SR20a].
    pub(in crate::db) fn call_modifies(&self, call: &CallStatement, schema: &str) -> bool {
        let name = qualify(&call.name, schema);
        let invoked = |r: &&SqlRoutineDesc| r.kind == RoutineKind::Procedure && r.parameters.len() == call.arguments.len();
        // One written in Rust possibly modifies SQL-data.
        Session::rust_procedure(self, call).is_some()
            || self.catalog.routines_named(&name).into_iter().filter(invoked).any(|r| r.access == DataAccess::Modifies)
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR9h iii, iv] The value the `k`-th
/// parameter `p` of `desc`, from 0, is given by its argument `given`, bound in
/// `cx`, as its mode allows; the target of an output one kept in
/// `outputs`.
fn argument(
    cx: Context<'_>,
    (desc, k, p): (&SqlRoutineDesc, usize, &SqlParameter),
    given: &SqlArgument,
    outputs: &mut Outputs,
) -> Result<Expr> {
    let target = match given {
        SqlArgument::Value(v) => target_of(cx, v).map(Target::Spec).or_else(|| dynamic_of(v).map(Target::Dynamic)),
        SqlArgument::Default(_) | SqlArgument::Null(_) => None,
    };
    match (p.mode, given, target) {
        (ParameterMode::In, SqlArgument::Value(v), _) => input(cx, p, v),
        (ParameterMode::In, SqlArgument::Default(at) | SqlArgument::Null(at), _) => defaulted("T522", 6, *at),
        (ParameterMode::InOut, SqlArgument::Default(at), _) => defaulted("T523", 7, *at),
        (ParameterMode::Out, _, Some(to)) => {
            outputs.push(cx, (k, p), to)?;
            Ok(Expr::Const(Value::Null))
        }
        (ParameterMode::InOut, SqlArgument::Value(v), Some(to)) => {
            let value = input(cx, p, v)?;
            outputs.push(cx, (k, p), to)?;
            Ok(value)
        }
        (ParameterMode::Out, _, None)
        | (ParameterMode::InOut, SqlArgument::Value(_), None)
        | (ParameterMode::InOut, SqlArgument::Null(_), _) => no_target(desc, (k, p.mode == ParameterMode::InOut), given),
    }
}

/// An output parameter as a column whose value is assigned to its target.
pub(in crate::db) fn out_column(p: &SqlParameter) -> OutColumn {
    OutColumn { name: Some(p.name.clone()), collation: None, data_type: p.data_type.clone(), not_null: false, origin: None }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §16.1 GR1, §9.18 GR3 to GR10] CALL `call` in
    /// `cx` with `params`.
    ///
    /// # Errors
    ///
    /// As `bind_call`; an argument's, or its store assignment's [GR4c i];
    /// SQLCODE -900 past `MAX_NESTING` contexts; the body's; an output
    /// parameter's, assigned to its target [GR10b].
    pub(in crate::db) fn call_statement(&self, cx: StatementContext<'_>, call: &CallStatement, params: &mut Params) -> Result<Completion> {
        if let Some(p) = Session::rust_procedure(cx.st, call) {
            return self.rust_call(cx, &*p, call, params);
        }
        let bound = self.bound(cx.st, params, |b| State::bind_call(b, call))?;
        let values = call_values((self, cx), &bound)?;
        let desc = &bound.desc;
        let scope = RoutineScope::of(desc, routine_path(&*cx.st.catalog, desc));
        let mut inner = Params::of_routine(values, Arc::new(scope));
        // Its text read once while it stays the same (RULES.md D406).
        let routine = self.kept_routine(desc)?;
        let body = routine.body()?;
        let owner = owner_of(&*cx.st.catalog, &desc.name.auth).to_owned();
        {
            // [§9.18 GR7, §11.60 GR3k] at a savepoint level of its own if
            // NEW SAVEPOINT LEVEL, else at the caller's.
            let level = routine.new_savepoint_level().then_some(Level::Procedure);
            let _pushed = self.push(&desc.name, (owner, desc.schema.clone()), level)?;
            self.run_body(cx, body, &mut inner, &routine)?;
        }
        let out = inner.dynamic_values();
        // [ISO/IEC 9075-2:2023 §20.7 GR7a] a `?`'s value its output's.
        for (k, index) in &bound.dynamic {
            params.set_dynamic(*index, out.get(*k).cloned().unwrap_or(Value::Null))?;
        }
        let returned: Vec<Value> = bound.targets.iter().map(|(k, _)| out.get(*k).cloned().unwrap_or(Value::Null)).collect();
        let specs: Vec<ParameterSpecification> = bound.targets.iter().map(|(_, t)| t.clone()).collect();
        assign_targets(
            &returned,
            &specs,
            params,
            "ISO/IEC 9075-2:2023 §9.18 GR10b i, §9.1 GR3b",
            "ISO/IEC 9075-2:2023 §9.18 GR10b i; X3.135-1986 §8.6 GR10",
        )
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3, GR4] The values of `bound`'s
/// parameters: each argument's, evaluated as a row of no table is, so its
/// subqueries run, and assigned to its parameter's type.
///
/// # Errors
///
/// An argument's; its store assignment's, of the parameter [§23.1 GR4q].
fn call_values((session, cx): (&Session, StatementContext<'_>), bound: &BoundCall) -> Result<Row> {
    let desc = &bound.desc;
    let types = desc.parameter_types();
    let logical = LogicalPlan::Values { rows: vec![bound.inputs.clone()], types: types.clone() };
    // [ISO/IEC 9075-4:2006 draft §11.2] an argument that invokes a function
    // that possibly modifies SQL-data evaluated by the session.
    let row = if let Some(found) = hosted_rows(&logical, &Hosted::new(Some(session), cx)) {
        found?.into_iter().next().unwrap_or_default()
    } else {
        let (optimized, _, physical) = plan(cx.st, logical);
        for t in optimized.tables() {
            cx.st.read_lock(cx.tx, &t)?;
        }
        execute(physical, cx.st, cx.tx.id).next().transpose()?.unwrap_or_default()
    };
    let mut values = Vec::with_capacity(row.len());
    for (k, (v, p)) in row.into_iter().zip(&desc.parameters).enumerate() {
        let about = || Subject::parameter(desc.name.parts(), desc.specific.parts(), (k.saturating_add(1), p.mode.name(), p.name.clone()));
        values.push(assign(v, &p.data_type, "ISO/IEC 9075-2:2023 §9.18 GR4c i").map_err(|e| e.or_about(about()))?);
    }
    Ok(values)
}
