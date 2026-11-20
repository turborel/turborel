//! \[DESIGN\] SQL-invoked functions inlined (RULES.md D316). An invocation
//! of a function whose body returns one value is that value's expression,
//! each parameter its argument stored in the parameter's type, the result
//! stored in the result type [ISO/IEC 9075-2:2023 §9.18 GR4a ii, GR8b ix
//! 2, GR10a iii; §16.2 GR1]. A correlated subquery of the body, which ran
//! for each invocation (RULES.md D73), is then a subquery of the invoking
//! plan, which `UnnestSubqueries` unnests.
//!
//! The body is a `<return statement>` of one value, or an SQL control
//! statement (ISO/IEC 9075-4:2006 draft §11.2) that is that statement
//! alone, in a compound statement of no variable, cursor or handler, or
//! not. The function is deterministic, possibly modifies no SQL-data, is
//! no null-call function and no method, so evaluating its value where it
//! is invoked has the effect running its body has. Its arguments are
//! deterministic, as each is evaluated where its parameter is read, and
//! every parameter is read where nothing may leave it unread, so each
//! argument's error is raised as the invocation raised it before its body
//! ran [§9.18 GR4]. A body that runs where an SQL-session runs, or any
//! other, is invoked as before.

use std::sync::Arc;

use super::unnesting::{Assigned, expr_pure};
use super::{Rule, RuleContext};
use crate::error::Subject;
use crate::functions::ScalarRef;
use crate::logical::{Cond, Expr, Invocation, LogicalPlan, Program, Step, filled};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const INLINE_FUNCTIONS: &str = "inline_functions";

/// [ISO/IEC 9075-2:2023 §9.18 GR4a ii] The rule an argument's store
/// assignment to its parameter cites, as `exec/invoked.rs` does.
const PARAMETER_RULE: &str = "ISO/IEC 9075-2:2023 §9.18 GR4a ii";
/// [ISO/IEC 9075-2:2023 §9.18 GR10a iii] The rule the result's store
/// assignment cites.
const RESULT_RULE: &str = "ISO/IEC 9075-2:2023 §9.18 GR10a iii";
/// [ISO/IEC 9075-2:2023 §9.18 GR4a] The mode of a function's parameter.
const IN: &str = "IN";

/// Inlines the invocations of a filter's condition or a projection's
/// expressions that can be (see the module's documentation).
pub struct InlineFunctions;

impl Rule for InlineFunctions {
    fn name(&self) -> &'static str {
        INLINE_FUNCTIONS
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let mut changed = false;
        let node = match plan {
            LogicalPlan::Filter { cond, input } => {
                LogicalPlan::Filter { cond: cond.clone().map_exprs(&mut |e| inlined_in(e, &mut changed)), input: input.clone() }
            }
            LogicalPlan::Project { exprs, input } => {
                let exprs = exprs.iter().map(|e| inlined_in(e.clone(), &mut changed)).collect();
                LogicalPlan::Project { exprs, input: input.clone() }
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(_)
            | LogicalPlan::Window(_)
            | LogicalPlan::Recognize(_)
            | LogicalPlan::Polymorphic(_) => return None,
        };
        changed.then_some(node)
    }
}

/// `e`, each invocation in it that can be inlined replaced by its value;
/// `changed` set if one was.
fn inlined_in(e: Expr, changed: &mut bool) -> Expr {
    if let Expr::Invoked(i) = &e
        && let Some(value) = inlined(i)
    {
        *changed = true;
        return value;
    }
    e.map_children(&mut |child| inlined_in(child, changed))
}

/// The value of the invocation `i`, as an expression, if it can be
/// inlined.
fn inlined(i: &Invocation) -> Option<Expr> {
    let plain = i.rust.is_none() && i.overrides.is_none() && !i.modifies && i.deterministic && !i.null_call;
    if !plain || !i.args.iter().all(expr_pure) {
        return None;
    }
    let value = body_value(i)?;
    let mut read = Vec::new();
    columns_read(&value, false, &mut read);
    if (0..i.params.len()).any(|p| !read.contains(&p)) {
        return None;
    }
    let stored: Vec<Expr> = i.args.iter().zip(&i.params).enumerate().map(|(n, (arg, (name, t)))| parameter(i, (n, name, t), arg)).collect();
    let assigned = Assigned { to: i.result.clone(), rule: RESULT_RULE, about: Subject::default() };
    Some(Expr::Call(ScalarRef(Arc::new(assigned)), vec![columns_replaced(value, &stored)]))
}

/// Argument `arg` of `i`, stored in its parameter `name` of type `t`,
/// the `n`th from 0.
fn parameter(i: &Invocation, (n, name, t): (usize, &String, &crate::value::DataType), arg: &Expr) -> Expr {
    let about = Subject::parameter(i.name.parts(), i.specific.parts(), (n.saturating_add(1), IN, name.clone()));
    let assigned = Assigned { to: t.clone(), rule: PARAMETER_RULE, about };
    Expr::Call(ScalarRef(Arc::new(assigned)), vec![arg.clone()])
}

/// The one value `i`'s body returns, each parameter the column of its
/// number. Of a `<return statement>`, its outer references become the
/// parameters' columns; of a program's, the frame's slots of them
/// (`exec/program/leaves.rs`), read only where they can be replaced.
fn body_value(i: &Invocation) -> Option<Expr> {
    let (body, framed) = match (&i.body, &i.program) {
        (Some(body), None) => (body, false),
        (None, Some(program)) => (&returned(program)?.query, true),
        (None, None) | (Some(_), Some(_)) => return None,
    };
    let LogicalPlan::Values { rows, .. } = &body.plan else { return None };
    let [row] = rows.as_slice() else { return None };
    let [value] = row.as_slice() else { return None };
    if body.outer.iter().any(Expr::can_raise) {
        return None;
    }
    let mut outer = body.outer.clone();
    let mut value = value.clone();
    if framed {
        let width = i.params.len();
        outer = outer.into_iter().map(|o| framed_columns(o, width)).collect::<Option<Vec<Expr>>>()?;
        value = framed_columns(value, width)?;
    }
    let value = slots_replaced(value, &outer);
    // A frame's slot in a plan of the value's subqueries is not replaced.
    let unframed = value.all_subqueries().iter().all(|s| filled(s.plan.clone(), &[]).is_ok());
    unframed.then_some(value)
}

/// `e`, each slot of the frame, of a parameter of the `width`, the column
/// of its number; none if it reads a variable's.
fn framed_columns(e: Expr, width: usize) -> Option<Expr> {
    let mut beyond = false;
    let e = framed(e, width, &mut beyond);
    (!beyond).then_some(e)
}

/// `framed_columns`, noting in `beyond` a slot past the parameters.
fn framed(e: Expr, width: usize, beyond: &mut bool) -> Expr {
    if let Expr::Dynamic(slot) = e {
        *beyond = *beyond || slot.index >= width;
        return Expr::Column(slot.index);
    }
    e.map_children(&mut |x| framed(x, width, beyond))
}

/// The leaf of `program`'s one RETURN, if it is that statement alone.
fn returned(program: &Program) -> Option<&crate::logical::Leaf> {
    if !program.variables.is_empty() || !program.cursors.is_empty() {
        return None;
    }
    let step = match &program.body {
        Step::Block(b) => {
            let bare = b.variables.is_empty() && b.cursors.is_empty() && b.handlers.is_empty();
            let [only] = b.statements.as_slice() else { return None };
            if !bare {
                return None;
            }
            only
        }
        other => other,
    };
    let Step::Return(Some(leaf)) = step else { return None };
    Some(leaf)
}

/// Adds to `out` each column, a parameter's, `e` reads where it is
/// evaluated whenever `e` is, so not if `conditional`, nor in a CASE's
/// result, ELSE or WHEN after the first [ISO/IEC 9075:1992 §6.9 GR2].
fn columns_read(e: &Expr, conditional: bool, out: &mut Vec<usize>) {
    match e {
        Expr::Column(column) => {
            if !conditional {
                out.push(*column);
            }
        }
        Expr::Case(whens, otherwise) => {
            for (n, (c, result)) in whens.iter().enumerate() {
                cond_columns_read(c, conditional || n > 0, out);
                columns_read(result, true, out);
            }
            columns_read(otherwise, true, out);
        }
        Expr::Truth(c) => cond_columns_read(c, conditional, out),
        Expr::Outer(..)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => e.for_each_child(&mut |child| columns_read(child, conditional, out)),
    }
}

/// `columns_read`, of a condition.
fn cond_columns_read(c: &Cond, conditional: bool, out: &mut Vec<usize>) {
    match c {
        Cond::And(a, b) | Cond::Or(a, b) => {
            cond_columns_read(a, conditional, out);
            cond_columns_read(b, conditional, out);
        }
        Cond::Not(a) | Cond::Is(a, ..) => cond_columns_read(a, conditional, out),
        Cond::Const(_)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::Value(_)
        | Cond::In(..) => c.for_each_expr(&mut |e| columns_read(e, conditional, out)),
    }
}

/// `e`, each column replaced by `values`' expression for it.
fn columns_replaced(e: Expr, values: &[Expr]) -> Expr {
    if let Expr::Column(c) = e {
        return values.get(c).cloned().unwrap_or(Expr::Column(c));
    }
    e.map_children(&mut |x| columns_replaced(x, values))
}

/// `e`, each outer reference slot replaced by `values`' expression for
/// it, those of the subqueries nested in it among them.
fn slots_replaced(e: Expr, values: &[Expr]) -> Expr {
    if let Expr::Outer(slot, name) = e {
        return values.get(slot).cloned().unwrap_or(Expr::Outer(slot, name));
    }
    e.map_children(&mut |x| slots_replaced(x, values))
}
