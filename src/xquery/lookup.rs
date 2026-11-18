//! \[EXTENSION\] RULES.md D86: a query reduced to the query of an XML
//! index (`index::xml::XmlQuery`) that selects every value of one node
//! the query's value is not empty of [ISO/IEC 9075-14:2006 draft §8.4 GR2],
//! so a planner may read only the rows the index gives, each checked by
//! the query itself, or answered by the index where it is exact.
//!
//! A query reduces if its body is one path [XQuery §3.2] from the context
//! item or `/`: steps of the child, descendant, `descendant-or-self::
//! node()` (`//`) and self axes over a name or `*`, its last of the
//! attribute axis if it selects attributes. Its last step may have one
//! predicate: a relative path of such steps, or a general or value
//! comparison of one with a literal [XQuery §3.5.1, §3.5.2], `.` standing
//! for the step's own nodes. `a[b op v]` selects an `a` with a `b` the
//! comparison holds of, as `a/b op v` does, so its lookup is `a/b` and the
//! comparison. One step before it, of child steps after it, may have one
//! too, a relative path or its general comparison with a string: the
//! query's filter of an ancestor (`index::xml::XmlFilter`), as in
//! `/BOOK[@ISBN = "1"]/SECTION`. Anything else reduces to none: a planner
//! then reads every row.
//!
//! A general comparison with a string literal, by the codepoint
//! collation, or with a number is the lookup's comparison, as an index
//! keys a node's value as a string and as an `xs:double` (RULES.md D86);
//! any other comparison is left to the check of each row, the lookup the
//! path alone. A predicate of an ancestor that may raise an error, a
//! comparison with a number or a value comparison, reduces to none: it is
//! evaluated of every ancestor, whether a node below it is selected or not.

use super::Query;
use super::ast::{Comparator, GeneralComp};
use super::collation::CODEPOINT;
use super::plan::{Axis, CompareOp, Namespace, Op, PathOp, PathStart, StepOp, StepTest};
use super::types::NodeKind;
use crate::index::xml::{Axis as Along, Comparison, Literal, MAX_STEPS, PathPattern, Step, Target, XmlFilter, XmlLookup, XmlQuery};
use crate::value::{CompOp, round_to_single};
use crate::xml::{Atomic, Primitive, QName, double};

/// \[EXTENSION\] RULES.md D86: a query reduced to an XML index's query.
#[derive(Debug, Clone)]
pub struct Reduction {
    /// The index's query: of a value of one node, a row its lookup does
    /// not give is one the query's value is empty of, and so is one of
    /// which its filter of an ancestor, if any, holds of none.
    pub query: XmlQuery,
    /// Whether the query's evaluation of a value of one node raises no
    /// error: a path alone, or compared with a string. A comparison with a
    /// number casts each node's value \[err:FORG0001\]; a value comparison
    /// of more than one node fails \[err:XPTY0004\]. A row such a query
    /// may fail of, its own lookup gives (RULES.md D15).
    pub raise_free: bool,
    /// Whether the index's query is the query's own test, its every
    /// comparison kept and raising no error, so a row the index answers
    /// holds of it needs no check (`XmlQuery::matches`).
    pub exact: bool,
}

impl Query {
    /// The query reduced to an XML index's query, if it is a path the
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

/// A path reduced as it is taken.
struct Taking<'a> {
    /// Its steps.
    steps: Vec<Step>,
    /// What its last selects.
    target: Target,
    /// The predicates of its steps, each of its step's place.
    predicates: Vec<(usize, &'a Op)>,
}

/// A path reduced, from the context item or `/`, which of a value's one
/// node is that node: its steps, its last step's predicate's, and its
/// filter of an ancestor.
fn reduce(path: &PathOp) -> Option<Reduction> {
    let mut path = steps_of(&path.steps, true)?;
    let last = path.steps.len().saturating_sub(1);
    let own = path.predicates.iter().position(|&(at, _)| at == last).map(|i| path.predicates.swap_remove(i).1);
    let ancestor = match path.predicates.as_slice() {
        [] => None,
        [one] => Some(*one),
        [_, _, ..] => return None,
    };
    let mut reduction = Reduction {
        query: XmlQuery {
            lookup: XmlLookup { pattern: PathPattern { steps: path.steps, target: path.target }, comparisons: Vec::new(), row: None },
            filter: None,
        },
        raise_free: true,
        exact: true,
    };
    if let Some(predicate) = own {
        predicated(predicate, &mut reduction)?;
    }
    if let Some((at, predicate)) = ancestor {
        reduction.query.filter = Some(filter_of(predicate, at, &mut reduction)?);
    }
    reduction.exact &= reduction.raise_free;
    (reduction.query.lookup.pattern.steps.len() <= MAX_STEPS).then_some(reduction)
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

/// The predicate of a step of `predicates`, if it has one, which a path
/// may have if `allowed`; none if it may not, or has more than one.
fn own_predicate(predicates: &[Op], allowed: bool) -> Result<Option<&Op>, ()> {
    match predicates {
        [] => Ok(None),
        [one] => {
            if allowed {
                Ok(Some(one))
            } else {
                Err(())
            }
        }
        [_, _, ..] => Err(()),
    }
}

/// The steps of a path `steps`, what its last selects, and the predicate
/// of each, if `predicates` allows them; none if a step is of another
/// kind, or none selects nodes by a name test.
fn steps_of(steps: &[StepOp], predicates: bool) -> Option<Taking<'_>> {
    let (mut descend, mut taking) = (false, Taking { steps: Vec::new(), target: Target::Element, predicates: Vec::new() });
    for (at, step) in steps.iter().enumerate() {
        let (axis, test, own) = match step {
            StepOp::Axis(axis, test, own) => (axis, test, own),
            // `.`, of no predicate, is the node it is of, as `self::node()`.
            StepOp::Filter(Op::ContextItem, own) => {
                if !own.is_empty() {
                    return None;
                }
                continue;
            }
            StepOp::Filter(..) => return None,
        };
        if taking.target == Target::Attribute {
            return None;
        }
        let own = own_predicate(own, predicates).ok()?;
        match taken(*axis, matches!(test, StepTest::Kind(NodeKind::Any)))? {
            Taken::Nothing => {
                if own.is_some() {
                    return None;
                }
                continue;
            }
            Taken::Descend => {
                if at.saturating_add(1) == steps.len() || own.is_some() {
                    return None;
                }
                descend = true;
                continue;
            }
            Taken::Named => {}
        }
        let along = if descend || *axis == Axis::Descendant { Along::Descendant } else { Along::Child };
        if *axis == Axis::Attribute {
            taking.target = Target::Attribute;
        }
        taking.steps.push(step_of(along, test)?);
        taking.predicates.extend(own.map(|p| (taking.steps.len().saturating_sub(1), p)));
        descend = false;
    }
    (!taking.steps.is_empty() && !descend).then_some(taking)
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
        return below(path, &mut reduction.query.lookup.pattern);
    }
    let Op::Compare(compare) = predicate else { return None };
    let (side, comparison) = compared(compare)?;
    match side {
        Side::Path(path) => below(path, &mut reduction.query.lookup.pattern)?,
        Side::Item => {}
    }
    match comparison {
        Compared::Kept(c) => {
            reduction.raise_free &= matches!(c.literal, Literal::String(_) | Literal::Untyped(_));
            reduction.query.lookup.comparisons.push(c);
        }
        Compared::Dropped => reduction.exact = false,
        Compared::Raising => reduction.raise_free = false,
    }
    Some(())
}

/// `pattern` continued by `path`, relative to its last step's nodes, of
/// no predicate; none if `path` starts at `/`, or below an attribute.
fn below(path: &PathOp, pattern: &mut PathPattern) -> Option<()> {
    if path.start != PathStart::Context || pattern.target == Target::Attribute {
        return None;
    }
    let taking = steps_of(&path.steps, false)?;
    pattern.steps.extend(taking.steps);
    pattern.target = taking.target;
    Some(())
}

/// The filter of an ancestor a predicate `predicate` of the `at`th step
/// of `reduction`'s lookup makes: of a relative path, or of its general
/// comparison with a string, the steps below it child steps; none of
/// another, or of one that may raise an error.
fn filter_of(predicate: &Op, at: usize, reduction: &mut Reduction) -> Option<XmlFilter> {
    let steps = &reduction.query.lookup.pattern.steps;
    if !steps.get(at.saturating_add(1)..)?.iter().all(|s| s.axis == Along::Child) {
        return None;
    }
    let above = steps.len().saturating_sub(at).saturating_sub(1);
    let mut pattern = PathPattern { steps: Vec::new(), target: Target::Element };
    let mut comparisons = Vec::new();
    if let Op::Path(path) = predicate {
        below(path, &mut pattern)?;
    } else {
        let Op::Compare(compare) = predicate else { return None };
        let (Side::Path(path), comparison) = compared(compare)? else { return None };
        below(path, &mut pattern)?;
        match comparison {
            Compared::Kept(c @ Comparison { literal: Literal::String(_) | Literal::Untyped(_), .. }) => comparisons.push(c),
            Compared::Dropped => reduction.exact = false,
            Compared::Kept(Comparison { literal: Literal::Number(_), .. }) | Compared::Raising => return None,
        }
    }
    Some(XmlFilter { above, pattern, comparisons })
}

/// The side of a comparison a path is on.
enum Side<'a> {
    /// A relative path.
    Path(&'a PathOp),
    /// `.`.
    Item,
}

/// What an index keeps of a comparison.
enum Compared {
    /// A general comparison with a string, by the codepoint collation, or
    /// with a number.
    Kept(Comparison),
    /// A general comparison with a string by another collation, which
    /// raises no error.
    Dropped,
    /// A value comparison, which may raise one.
    Raising,
}

/// `compare`, of a relative path or `.` and a literal: its path, and what
/// an index keeps of it; none of another.
fn compared(compare: &CompareOp) -> Option<(Side<'_>, Compared)> {
    let (side, literal, flipped) = if let Op::Atomic(literal) = &compare.right {
        (&compare.left, literal, false)
    } else if let Op::Atomic(literal) = &compare.left {
        (&compare.right, literal, true)
    } else {
        return None;
    };
    let side = if let Op::Path(path) = side { Side::Path(path) } else { matches!(side, Op::ContextItem).then_some(Side::Item)? };
    let general = match compare.op {
        Comparator::General(op) => op,
        Comparator::Value(_) => return Some((side, Compared::Raising)),
        Comparator::Node(_) => return None,
    };
    let op = if flipped { flip(comp_op(general)) } else { comp_op(general) };
    let literal = literal_of(literal)?;
    let codepoint = compare.collation.uri() == CODEPOINT;
    if matches!(literal, Literal::String(_) | Literal::Untyped(_)) && !codepoint {
        return Some((side, Compared::Dropped));
    }
    Some((side, Compared::Kept(Comparison { op, literal })))
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
