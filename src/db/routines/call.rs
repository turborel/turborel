//! [ISO/IEC 9075-2:2023 §16.1, §9.18, §10.4] CALL of an SQL-invoked
//! procedure (Feature T321).
//!
//! The subject routine is the procedure of the name that takes the
//! arguments, the one of fewest parameters, on which the current
//! authorization identifier holds EXECUTE [§9.18 SR6, SR9]. Each argument
//! is as its parameter's mode allows [SR9h iii, iv]: a value expression,
//! NULL or DEFAULT of an input parameter; a target specification of an
//! output one; a target specification or DEFAULT of one both. A parameter
//! past the arguments written, or named by none, takes DEFAULT, its
//! default value (`defaults.rs`) (Features T522 and T523). A target is a
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
use super::super::prepared::{Keeping, KeptPlan};
use super::super::savepoints::Level;
use super::super::{Completion, Session, State, StatementContext, rule};
use super::defaults::default_input;
use super::program::{Hosted, hosted_rows};
use crate::ast::{
    CallStatement, Composite, ElementPosition, ParameterSpecification, SqlArgument, TargetKind, ValueExpression, ValueSpecification,
};
use crate::binder::named::{all_or_none, placed};
use crate::binder::{Context, OutColumn, bind_argument, qualify, resolve_procedure, takes};
use crate::catalog::{DataAccess, ParameterMode, RoutineKind, SqlParameter, SqlRoutineDesc, owner_of, routine_path};
use crate::error::{At, Result, Subject};
use crate::exec::execute;
use crate::logical::{Expr, LogicalPlan, filled};
use crate::params::{ParameterType, Params, RoutineScope, conformed};
use crate::storage::Row;
use crate::value::{DataType, Value, assign};

/// [ISO/IEC 9075-2:2023 §9.18 GR4c i] The rule that assigns an argument's
/// value to its parameter.
const ASSIGNS: &str = "ISO/IEC 9075-2:2023 §9.18 GR4c i";

/// [ISO/IEC 9075-2:2023 §16.1] A CALL, bound.
#[derive(Debug)]
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
    /// [ISO/IEC 9075-2:2023 §20.10 GR7e ii, iii] Each `?` an argument simply
    /// contains, by its number, and the ordinal position, from 0, of the
    /// parameter that argument is of, in the order of the parameters.
    pub(crate) questions: Vec<(usize, usize)>,
    /// The scope of the procedure's body, made once as the CALL is bound
    /// and shared by its executions (RULES.md D940).
    pub(super) scope: Arc<RoutineScope>,
    /// How each parameter takes its value with no plan run, in order, if
    /// each argument is a `?` or a constant (RULES.md D940).
    pub(super) given: Option<Vec<Given>>,
}

/// \[DESIGN\] How a parameter of a CALL takes its argument's value with no
/// plan run (RULES.md D940).
#[derive(Debug)]
pub(super) enum Given {
    /// The value of the `?` at this place among the statement's, cast to
    /// the parameter's type [ISO/IEC 9075:1992 §17.9 GR5f] and assigned
    /// to it [ISO/IEC 9075-2:2023 §9.18 GR4c i]. `settled` if a value
    /// cast to that type is assigned to it unchanged, so it is not
    /// assigned again.
    Slot {
        /// The place of the `?`.
        index: usize,
        /// Whether the cast value is the assigned one.
        settled: bool,
    },
    /// A constant, assigned to the parameter's type as the CALL was bound.
    Constant(Value),
}

impl Given {
    /// How the parameter of type `to` takes the value of its argument
    /// `input`. None if the argument is neither a `?` of that type nor a
    /// constant the type takes.
    pub(super) fn of(input: &Expr, to: &DataType) -> Option<Given> {
        if let Expr::Dynamic(slot) = input {
            return (slot.data_type == *to).then(|| Given::Slot { index: slot.index, settled: settled(to) });
        }
        let Expr::Const(v) = input else { return None };
        assign(v.clone(), to, ASSIGNS).ok().map(Given::Constant)
    }

    /// The value the parameter of type `to` takes in an execution whose
    /// `?`s hold `slots`. None if it is not given that type cleanly, with
    /// neither a warning nor an exception.
    pub(super) fn value(&self, to: &DataType, slots: &[Value]) -> Option<Value> {
        match self {
            Given::Constant(v) => Some(v.clone()),
            Given::Slot { index, settled } => {
                let (value, warning) = conformed(slots.get(*index)?.clone(), to).ok()?;
                if warning.is_some() {
                    return None;
                }
                if *settled { Some(value) } else { assign(value, to, ASSIGNS).ok() }
            }
        }
    }
}

/// \[DESIGN\] Whether a value cast to `t` with no warning is assigned to
/// `t` unchanged (RULES.md D940). It holds of an exact number, which the
/// cast gives the type's scale within its precision, of a character
/// string, which the cast gives the type's length or a length within it,
/// of a truth value and a date, and of an array of one of these, which
/// the cast keeps within the maximum cardinality.
pub(super) fn settled(t: &DataType) -> bool {
    if let DataType::Array(a) = t { settled_scalar(&a.element) } else { settled_scalar(t) }
}

/// Whether `t` is an exact number, a character string, a truth value or a
/// date (`settled`).
fn settled_scalar(t: &DataType) -> bool {
    matches!(t, DataType::Exact { .. } | DataType::Character { .. } | DataType::Varying { .. } | DataType::Boolean | DataType::Date)
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
        let desc = resolve_procedure(cx, &call.name, &call.names)?;
        let shape: Vec<(&str, bool)> = desc.parameters.iter().map(|p| (p.name.as_str(), p.default.is_some())).collect();
        let places = placed(&shape, &call.names).unwrap_or_default();
        let mut outputs = Outputs::default();
        let (mut inputs, mut questions) = (Vec::new(), Vec::new());
        for (k, (p, place)) in desc.parameters.iter().zip(places).enumerate() {
            // [§9.18 SR9h i 1 B, 2 B] DEFAULT, of a parameter of no argument.
            let Some(j) = place else {
                inputs.push(default_input(cx, &desc, p)?);
                continue;
            };
            let Some(a) = call.arguments.get(j) else { continue };
            inputs.push(argument(cx, (&desc, k, p), a, &mut outputs)?);
            questions.extend(call.dynamic.get(j).into_iter().flatten().map(|number| (*number, k)));
        }
        let Outputs { targets, dynamic } = outputs;
        let columns: Vec<OutColumn> = targets.iter().filter_map(|(k, _)| desc.parameters.get(*k)).map(out_column).collect();
        let specs: Vec<ParameterSpecification> = targets.iter().map(|(_, t)| t.clone()).collect();
        // [SR9h iii 4, 5] Each output parameter's type assignable to its target.
        check_targets(&columns, &specs, cx.params, "§9.18 SR9h iii", "§9.18 SR9h iii 4")?;
        let scope = Arc::new(RoutineScope::of(&desc, routine_path(&*cx.db.catalog, &desc)));
        // An output parameter's target that is no `?` is assigned as any CALL assigns it.
        let each = || inputs.iter().zip(&desc.parameters).map(|(input, p)| Given::of(input, &p.data_type)).collect();
        let given = if targets.is_empty() { each() } else { None };
        Ok(BoundCall { desc, inputs, targets, dynamic, questions, scope, given })
    }

    /// Whether `call`, its names in schema `schema`, invokes a procedure
    /// that possibly modifies SQL-data: one of the name that takes its
    /// arguments [§9.18 SR9b, SR9d].
    pub(in crate::db) fn call_modifies(&self, call: &CallStatement, schema: &str) -> bool {
        let name = qualify(&call.name, schema);
        let invoked = |r: &&SqlRoutineDesc| r.kind == RoutineKind::Procedure && takes(r, &call.names);
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
        // [§9.18 GR3b ii] Its default value, its value out assigned to no
        // target [SR9h iii 2].
        (ParameterMode::In | ParameterMode::InOut, SqlArgument::Default(_), _) => default_input(cx, desc, p),
        (ParameterMode::In, SqlArgument::Null(_), _) => Ok(Expr::Const(Value::Null)),
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
    pub(in crate::db) fn call_statement(
        &self,
        cx: StatementContext<'_>,
        call: &CallStatement,
        params: &mut Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        // [DESIGN] RULES.md D940: a prepared CALL found kept runs straight.
        // Its binding is of a procedure of an SQL routine body, kept with
        // the functions registered as they are now, so none written in
        // Rust is of its name.
        let found = keeping.map(Keeping::plan).transpose()?.flatten();
        let found = if let Some(KeptPlan::Call(bound)) = found { Some(bound) } else { None };
        if let Some(bound) = &found
            && self.call_direct(cx, bound, params)?.is_some()
        {
            return Ok(Completion::Success);
        }
        if let (None, Some(p)) = (&found, Session::rust_procedure(cx.st, call)) {
            return self.rust_call(cx, &*p, call, params);
        }
        let (bound, kept) = match found {
            Some(bound) => (bound, true),
            None => self.call_bound(cx.st, call, params, keeping)?,
        };
        let done = self.call_general(cx, &bound, params, kept)?;
        if let Some(k) = keeping.filter(|_| kept) {
            self.keep_call(cx, &bound, k);
        }
        Ok(done)
    }

    /// `call_statement`, of the CALL `bound` binds, its arguments' slots
    /// filled if `kept`: the arguments evaluated as a row of values, and
    /// the body run over a frame of its own.
    fn call_general(&self, cx: StatementContext<'_>, bound: &BoundCall, params: &mut Params, kept: bool) -> Result<Completion> {
        let values = call_values((self, cx), bound, kept.then(|| params.dynamic_values()))?;
        let desc = &bound.desc;
        let mut inner = Params::of_routine(values, Arc::clone(&bound.scope));
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

impl Session {
    /// `call`, which its statement keeps no binding of, bound with
    /// `params`; of a prepared statement, kept, bound with its `?`s slots
    /// unless the binding holds a value (RULES.md D793), and then whether
    /// its arguments' slots are to be filled.
    ///
    /// # Errors
    ///
    /// As `bind_call`; SQLCODE -910 if a lock is poisoned.
    fn call_bound(
        &self,
        st: &State,
        call: &CallStatement,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<(Arc<BoundCall>, bool)> {
        let Some(k) = keeping else { return Ok((Arc::new(self.bound(st, params, |b| State::bind_call(b, call))?), false)) };
        let binding = params.keeping();
        let bound = Arc::new(self.bound(st, &binding, |b| State::bind_call(b, call))?);
        k.made((!binding.held()).then(|| KeptPlan::Call(Arc::clone(&bound))))?;
        Ok((bound, true))
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3, GR4] The values of `bound`'s
/// parameters: each argument's, evaluated as a row of no table is, so its
/// subqueries run, and assigned to its parameter's type.
///
/// # Errors
///
/// An argument's; its store assignment's, of the parameter [§23.1 GR4q].
fn call_values((session, cx): (&Session, StatementContext<'_>), bound: &BoundCall, slots: Option<&[Value]>) -> Result<Row> {
    let desc = &bound.desc;
    let types = desc.parameter_types();
    let logical = LogicalPlan::Values { rows: vec![bound.inputs.clone()], types: types.clone() };
    let logical = match slots {
        Some(values) => filled(logical, values)?,
        None => logical,
    };
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
