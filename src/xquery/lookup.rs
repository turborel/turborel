//! \[EXTENSION\] RULES.md D86: a query reduced to the lookup of an XML
//! index (`index::xml::XmlLookup`) that selects every value of one node
//! the query's value is not empty of [ISO/IEC 9075-14:2006 draft §8.4 GR2],
//! so a planner may read only the rows the lookup gives, each checked by
//! the query itself.
//!
//! A query reduces if its body is one path [XQuery §3.2] from the context
//! item or `/`: steps of the child, descendant, `descendant-or-self::
//! node()` (`//`) and self axes over a name or `*`, its last of the
//! attribute axis if it selects attributes, and at most one predicate, on
//! the last step: a relative path of such steps, or a general or value
//! comparison of one with a literal [XQuery §3.5.1, §3.5.2], `.` standing
//! for the step's own nodes. `a[b op v]` selects an `a` with a `b` the
//! comparison holds of, as `a/b op v` does, so its lookup is `a/b` and the
//! comparison. Anything else reduces to none: a planner then reads every
//! row.
//!
//! A general comparison with a string literal, by the codepoint
//! collation, or with a number is the lookup's comparison, as an index
//! keys a node's value as a string and as an `xs:double` (RULES.md D86);
//! any other comparison is left to the check of each row, the lookup the
//! path alone.

use super::Query;
use super::ast::{Comparator, GeneralComp};
use super::collation::CODEPOINT;
use super::plan::{Axis, CompareOp, Namespace, Op, PathOp, PathStart, StepOp, StepTest};
use super::types::NodeKind;
use crate::index::xml::{Axis as Along, Comparison, Literal, MAX_STEPS, PathPattern, Step, Target, XmlLookup};
use crate::value::{CompOp, round_to_single};
use crate::xml::{Atomic, Primitive, QName, double};

/// \[EXTENSION\] RULES.md D86: a query reduced to an XML index's lookup.
#[derive(Debug, Clone)]
pub struct Reduction {
    /// The lookup: of a value of one node, a row it does not give is one
    /// the query's value is empty of.
    pub lookup: XmlLookup,
    /// Whether the query's evaluation of a value of one node raises no
    /// error: a path alone, or compared with a string. A comparison with a
    /// number casts each node's value \[err:FORG0001\]; a value comparison
    /// of more than one node fails \[err:XPTY0004\]. A row such a query
    /// may fail of, its own lookup gives (RULES.md D15).
    pub raise_free: bool,
}

impl Query {
    /// The query reduced to an XML index's lookup, if it is a path the
    /// module's documentation describes; none if not, or if its prolog
    /// declares a variable or a function.
    #[must_use]
    pub fn reduced(&self) -> Option<Reduction> {
        let plan = &self.0;
        if !plan.globals.is_empty() || !plan.functions.is_empty() {
            return None;
        }
        let Op::Path(path) = &plan.body.op else { return None };
        reduce(path)
    }
}

/// A path reduced, from the context item or `/`, which of a value's one
/// node is that node: its steps, and its last step's predicate's.
fn reduce(path: &PathOp) -> Option<Reduction> {
    let (steps, target, predicate) = steps_of(&path.steps, true)?;
    let mut reduction =
        Reduction { lookup: XmlLookup { pattern: PathPattern { steps, target }, comparisons: Vec::new(), row: None }, raise_free: true };
    if let Some(predicate) = predicate {
        predicated(predicate, &mut reduction)?;
    }
    (reduction.lookup.pattern.steps.len() <= MAX_STEPS).then_some(reduction)
}

/// What a step does to the path a reduction makes.
enum Taken {
    /// Nothing: `self::node()`.
    Nothing,
    /// Makes the next step a descendant step: `descendant-or-self::node()`.
    Descend,
    /// Adds a step of a name test.
    Named,
}

/// What a step of `axis` does, its node test `node()` if `any_node`; none
/// if it cannot be reduced.
fn taken(axis: Axis, any_node: bool) -> Option<Taken> {
    match axis {
        Axis::SelfNode => any_node.then_some(Taken::Nothing),
        Axis::DescendantOrSelf => any_node.then_some(Taken::Descend),
        Axis::Child | Axis::Descendant | Axis::Attribute => Some(Taken::Named),
        Axis::FollowingSibling
        | Axis::Following
        | Axis::Parent
        | Axis::Ancestor
        | Axis::PrecedingSibling
        | Axis::Preceding
        | Axis::AncestorOrSelf => None,
    }
}

/// Whether a step of `predicates` may be reduced: of none, or of one if
/// `allowed`, as of the last step of a path that may have one.
fn takes_predicates(predicates: &[Op], allowed: bool) -> bool {
    match predicates {
        [] => true,
        [_] => allowed,
        [_, _, ..] => false,
    }
}

/// The steps of a path `steps`, what its last selects, and its last
/// step's predicate, if `predicate` allows one; none if a step is of
/// another kind, or none selects nodes by a name test.
fn steps_of(steps: &[StepOp], predicate: bool) -> Option<(Vec<Step>, Target, Option<&Op>)> {
    let (mut out, mut descend, mut target, mut last_predicate) = (Vec::new(), false, Target::Element, None);
    for (at, step) in steps.iter().enumerate() {
        let last = at.saturating_add(1) == steps.len();
        let (axis, test, predicates) = match step {
            StepOp::Axis(axis, test, predicates) => (axis, test, predicates),
            // `.`, of no predicate, is the node it is of, as `self::node()`.
            StepOp::Filter(Op::ContextItem, predicates) => {
                if !predicates.is_empty() {
                    return None;
                }
                continue;
            }
            StepOp::Filter(..) => return None,
        };
        if target == Target::Attribute {
            return None;
        }
        if !takes_predicates(predicates, last && predicate) {
            return None;
        }
        let own = predicates.first();
        match taken(*axis, matches!(test, StepTest::Kind(NodeKind::Any)))? {
            Taken::Nothing => {
                if own.is_some() {
                    return None;
                }
                continue;
            }
            Taken::Descend => {
                if last {
                    return None;
                }
                descend = true;
                continue;
            }
            Taken::Named => {}
        }
        let along = if descend || *axis == Axis::Descendant { Along::Descendant } else { Along::Child };
        if *axis == Axis::Attribute {
            target = Target::Attribute;
        }
        out.push(step_of(along, test)?);
        (descend, last_predicate) = (false, own);
    }
    (!out.is_empty() && !descend).then_some((out, target, last_predicate))
}

/// The step `along` of the name test `test`, its name none of `*`; none
/// if it is a kind test, or a wildcard of a part of a name.
fn step_of(along: Along, test: &StepTest) -> Option<Step> {
    let StepTest::Name(namespace, local) = test else { return None };
    let name = match (namespace, local) {
        (Namespace::Any, None) => None,
        (Namespace::Absent, Some(local)) => Some(QName { prefix: None, uri: None, local: local.clone() }),
        (Namespace::Uri(uri), Some(local)) => Some(QName { prefix: None, uri: Some(uri.clone()), local: local.clone() }),
        (Namespace::Any, Some(_)) | (Namespace::Absent | Namespace::Uri(_), None) => return None,
    };
    Some(Step { axis: along, name })
}

/// `reduction` narrowed by the last step's predicate `predicate`: its
/// relative path's steps added, and its comparison; none if it is of
/// another kind.
fn predicated(predicate: &Op, reduction: &mut Reduction) -> Option<()> {
    if let Op::Path(path) = predicate {
        return below(path, reduction);
    }
    let Op::Compare(compare) = predicate else { return None };
    compared(compare, reduction)
}

/// `reduction`'s path continued by `path`, relative to its last step's
/// nodes, of no predicate; none if `path` starts at `/`, or below an
/// attribute.
fn below(path: &PathOp, reduction: &mut Reduction) -> Option<()> {
    if path.start != PathStart::Context {
        return None;
    }
    let (steps, target, _) = steps_of(&path.steps, false)?;
    let pattern = &mut reduction.lookup.pattern;
    if pattern.target == Target::Attribute {
        return None;
    }
    pattern.steps.extend(steps);
    pattern.target = target;
    Some(())
}

/// `reduction` narrowed by `compare`, a comparison of a relative path, or
/// of `.`, with a literal.
fn compared(compare: &CompareOp, reduction: &mut Reduction) -> Option<()> {
    let (side, literal, flipped) = if let Op::Atomic(literal) = &compare.right {
        (&compare.left, literal, false)
    } else if let Op::Atomic(literal) = &compare.left {
        (&compare.right, literal, true)
    } else {
        return None;
    };
    if let Op::Path(path) = side {
        below(path, reduction)?;
    } else if !matches!(side, Op::ContextItem) {
        return None;
    }
    let general = match compare.op {
        Comparator::General(op) => op,
        Comparator::Value(_) => {
            reduction.raise_free = false;
            return Some(());
        }
        Comparator::Node(_) => return None,
    };
    let op = if flipped { flip(comp_op(general)) } else { comp_op(general) };
    let codepoint = compare.collation.uri() == CODEPOINT;
    match literal_of(literal)? {
        Literal::Number(n) => {
            reduction.raise_free = false;
            reduction.lookup.comparisons.push(Comparison { op, literal: Literal::Number(n) });
        }
        text @ (Literal::String(_) | Literal::Untyped(_)) => {
            if codepoint {
                reduction.lookup.comparisons.push(Comparison { op, literal: text });
            }
        }
    }
    Some(())
}

/// The operator of a general comparison.
fn comp_op(op: GeneralComp) -> CompOp {
    match op {
        GeneralComp::Eq => CompOp::Eq,
        GeneralComp::Ne => CompOp::Ne,
        GeneralComp::Lt => CompOp::Lt,
        GeneralComp::Le => CompOp::Le,
        GeneralComp::Gt => CompOp::Gt,
        GeneralComp::Ge => CompOp::Ge,
    }
}

/// `op` with its operands swapped: `v < a` is `a > v`.
fn flip(op: CompOp) -> CompOp {
    match op {
        CompOp::Eq => CompOp::Eq,
        CompOp::Ne => CompOp::Ne,
        CompOp::Lt => CompOp::Gt,
        CompOp::Le => CompOp::Ge,
        CompOp::Gt => CompOp::Lt,
        CompOp::Ge => CompOp::Le,
    }
}

/// The literal `a` as an index compares a node's value with it: a string
/// or untyped value as a string, a number as an `xs:double` [XQuery
/// §3.5.2]; none of another type.
fn literal_of(a: &Atomic) -> Option<Literal> {
    let primitive = a.kind.primitive();
    if primitive.textual() {
        return Some(Literal::String(a.lexical.to_string()));
    }
    let n = primitive.numeric().then(|| double(&a.lexical)).flatten()?;
    Some(Literal::Number(if primitive == Primitive::Float { round_to_single(n) } else { n }))
}

// Unit tests: each shape reduced, or not (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
