//! [ISO/IEC 9075-2:2023 §7.11 SR1i to SR1k] The plan tree of a `<JSON
//! table>`: its `<JSON table specific plan>`, checked, or the implicit one
//! its `<JSON table default plan>`, or none, makes.
//!
//! A path name is joined to the context item of its primitive by the
//! `<JSON table plan inner>` or `<JSON table plan outer>` that has it in
//! its second operand nearest to it, as its parent [SR1k iii 6 B II]; so
//! the first operand of that join names the clause it is nested in, of
//! whose rows it is chained. A plan that joins it to another is refused,
//! as is one that leaves a path name other than the outermost joined to
//! none, since its primitive would have no context item.
//!
//! The plan is no deeper than the statement's syntax tree, which the
//! parser bounds by `parser::MAX_NESTING_DEPTH`, so the recursion over it
//! is bounded (RULES.md R-CF-6).

use super::Clause;
use crate::ast::{JsonTablePlan, JsonTablePlanClause};
use crate::binder::rule;
use crate::error::{At, Result, SqlError};
use crate::json::PlanNode;

/// [§7.11 SR1j, SR1k] The nodes of the plan tree of the clauses `clauses`,
/// the outermost first, of the plan clause `clause`, and the node that is
/// the whole plan.
///
/// # Errors
///
/// SQLCODE -103 if a specific plan breaks SR1g i, SR1j i, SR1j ii or
/// SR1k iii 6 B II.
pub(super) fn plan(clauses: &[Clause<'_>], clause: Option<&JsonTablePlanClause>) -> Result<(Vec<PlanNode>, usize)> {
    let mut nodes = Vec::new();
    let top = match clause {
        Some(JsonTablePlanClause::Specific(plan)) => specific(clauses, plan, &mut nodes)?,
        // [SR1i i, ii] INNER or OUTER, CROSS or UNION, as written, else
        // OUTER and UNION.
        Some(JsonTablePlanClause::Default { inner, cross }) => implicit(clauses, 0, (*inner, *cross), &mut nodes),
        None => implicit(clauses, 0, (false, false), &mut nodes),
    };
    Ok((nodes, top))
}

/// `node` added to `nodes`, and its place.
fn push(nodes: &mut Vec<PlanNode>, node: PlanNode) -> usize {
    nodes.push(node);
    nodes.len().saturating_sub(1)
}

/// [§7.11 SR1i viii] CPLANc, of clause `c`: its primitive alone, of no
/// child; joined to its one child's plan; or joined to its children's
/// plans' union or product, as `(inner, cross)` say.
fn implicit(clauses: &[Clause<'_>], c: usize, (inner, cross): (bool, bool), nodes: &mut Vec<PlanNode>) -> usize {
    let children = clauses.get(c).map_or(&[][..], |clause| clause.children.as_slice());
    let child = match children {
        [] => return push(nodes, PlanNode::Primitive(c)),
        [only] => implicit(clauses, *only, (inner, cross), nodes),
        many => {
            let operands = many.iter().map(|&d| implicit(clauses, d, (inner, cross), nodes)).collect();
            push(nodes, PlanNode::Siblings { cross, operands })
        }
    };
    push(nodes, PlanNode::Parent { primitive: c, inner, child })
}

/// A plan compiled: its node, the clauses it joins to the context item of
/// what contains it (its heads), and every clause it names.
struct Compiled {
    node: usize,
    heads: Vec<usize>,
    named: Vec<usize>,
}

/// [§7.11 SR1j] The specific plan `plan` of `clauses`, compiled to
/// `nodes`.
fn specific(clauses: &[Clause<'_>], plan: &JsonTablePlan, nodes: &mut Vec<PlanNode>) -> Result<usize> {
    // [SR1g i] with a plan, every nested columns clause has a path name.
    if let Some((_, at)) = clauses.iter().skip(1).find(|c| c.name.is_none()).map(|c| c.path) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.11 SR1g i] with a PLAN, NESTED COLUMNS have a path name")).or_at(at);
    }
    let compiled = compile(clauses, plan, nodes)?;
    // [SR1j i] every path name exactly once, the outermost's among them.
    for (c, clause) in clauses.iter().enumerate() {
        let times = compiled.named.iter().filter(|&&n| n == c).count();
        if times != 1 {
            let name = clause.name.map_or("of the row pattern, which has none written,", |(n, _)| n);
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.11 SR1j i] the path name {name} appears in the PLAN {times} times, not once"
            )));
        }
    }
    // [SR1k iii 6 B II] the outermost primitive alone is of the context
    // item of the JSON table.
    if compiled.heads != [0] {
        return Err(chain_error(clauses, compiled.heads.iter().copied().find(|&h| h != 0).unwrap_or(0)));
    }
    Ok(compiled.node)
}

/// The plan `plan` compiled to `nodes`.
fn compile(clauses: &[Clause<'_>], plan: &JsonTablePlan, nodes: &mut Vec<PlanNode>) -> Result<Compiled> {
    match plan {
        JsonTablePlan::Name(name, at) => {
            let c = named(clauses, name).or_at(*at)?;
            Ok(Compiled { node: push(nodes, PlanNode::Primitive(c)), heads: vec![c], named: vec![c] })
        }
        JsonTablePlan::Parent { name, at, inner, child } => {
            let c = named(clauses, name).or_at(*at)?;
            let below = compile(clauses, child, nodes)?;
            // [SR1j ii] OTHERCOLS is contained in LEFTCOLS.
            if let Some(&other) = below.named.iter().find(|&&o| !within(clauses, o, c)) {
                let other = clauses.get(other).and_then(|o| o.name).map_or("", |(n, _)| n);
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1j ii] the columns of {other} are not nested in those of {name}")))
                    .or_at(*at);
            }
            if let Some(&head) = below.heads.iter().find(|&&h| clauses.get(h).and_then(|clause| clause.parent) != Some(c)) {
                return Err(chain_error(clauses, head)).or_at(*at);
            }
            let node = push(nodes, PlanNode::Parent { primitive: c, inner: *inner, child: below.node });
            let mut all = vec![c];
            all.extend(below.named);
            Ok(Compiled { node, heads: vec![c], named: all })
        }
        JsonTablePlan::Siblings { cross, operands } => {
            let (mut ops, mut heads, mut all) = (Vec::new(), Vec::new(), Vec::new());
            for operand in operands {
                let done = compile(clauses, operand, nodes)?;
                ops.push(done.node);
                heads.extend(done.heads);
                all.extend(done.named);
            }
            Ok(Compiled { node: push(nodes, PlanNode::Siblings { cross: *cross, operands: ops }), heads, named: all })
        }
    }
}

/// The clause whose path name is `name`.
///
/// # Errors
///
/// SQLCODE -103 if none is [SR1j i].
fn named(clauses: &[Clause<'_>], name: &str) -> Result<usize> {
    clauses
        .iter()
        .position(|c| c.name.is_some_and(|(n, _)| n == name))
        .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1j i] the PLAN names {name}, which is no path name of the JSON table")))
}

/// Whether clause `inner` is nested in clause `outer`, at any depth. The
/// walk up is bounded by the number of clauses.
fn within(clauses: &[Clause<'_>], inner: usize, outer: usize) -> bool {
    let mut at = clauses.get(inner).and_then(|c| c.parent);
    for _ in 0..clauses.len() {
        let Some(p) = at else { return false };
        if p == outer {
            return true;
        }
        at = clauses.get(p).and_then(|c| c.parent);
    }
    false
}

/// [§7.11 SR1k iii 6 B II] The error of a plan that joins clause `c` to
/// the rows of another than the clause it is nested in, or to none.
fn chain_error(clauses: &[Clause<'_>], c: usize) -> SqlError {
    let name = clauses.get(c).and_then(|clause| clause.name).map_or("", |(n, _)| n);
    rule(format!(
        "[ISO/IEC 9075-2:2023 §7.11 SR1k iii 6 B II] the PLAN joins {name} by OUTER or INNER to no path name of the columns it is nested in"
    ))
}
