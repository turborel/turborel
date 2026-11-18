//! \[EXTENSION\] RULES.md D106: the conditions of a step's node, its
//! predicates' conjuncts [XQuery §3.2.2, §3.6], each a branch: a relative
//! path from the node, none of `.`, and what an index keeps of its
//! comparison with a literal [XQuery §3.5.1, §3.5.2].

use super::super::ast::{Comparator, GeneralComp};
use super::super::collation::CODEPOINT;
use super::super::plan::{CompareOp, Op, PathOp, PathStart};
use super::steps::twig;
use crate::index::xml::{Axis, Comparison, Literal, PathPattern, Target};
use crate::value::{CompOp, round_to_single};
use crate::xml::{Atomic, Primitive, double};

/// \[EXTENSION\] A condition of a node: a path relative to it, of no
/// step of `.`, and comparisons of the nodes it selects.
#[derive(Debug, Clone)]
pub(super) struct Branch {
    /// The path, of no step of the node itself.
    pub(super) pattern: PathPattern,
    /// The comparisons an index keeps.
    pub(super) comparisons: Vec<Comparison>,
    /// Whether its comparison may raise an error: of a number, or a value
    /// comparison.
    pub(super) raises: bool,
    /// Whether a comparison is left to the check of each row.
    pub(super) dropped: bool,
}

impl Branch {
    /// The branch of the path `pattern` alone.
    pub(super) fn of(pattern: PathPattern) -> Branch {
        Branch { pattern, comparisons: Vec::new(), raises: false, dropped: false }
    }

    /// Whether its path has steps, each a child step.
    pub(super) fn child_steps(&self) -> bool {
        !self.pattern.steps.is_empty() && self.pattern.steps.iter().all(|s| s.axis == Axis::Child)
    }
}

/// The branches of `op`, a predicate: one of each conjunct [XQuery §3.6];
/// none if one is of another kind.
pub(super) fn branches(op: &Op) -> Option<Vec<Branch>> {
    if let Op::And(operands) = op {
        return operands.iter().map(branches).collect::<Option<Vec<_>>>().map(|b| b.concat());
    }
    if let Op::Path(path) = op {
        return Some(vec![Branch::of(relative(path)?)]);
    }
    // A positional predicate, or any other expression, is no branch.
    let Op::Compare(compare) = op else { return None };
    compared(compare).map(|b| vec![b])
}

/// The steps of `path`, relative to a node, of no condition, each kept
/// exactly; none if it starts at `/`, or is of another kind.
fn relative(path: &PathOp) -> Option<PathPattern> {
    if path.start != PathStart::Context {
        return None;
    }
    let made = twig(path)?;
    let plain = made.exact && made.root.is_empty() && made.steps.iter().all(|t| t.conditions.is_empty());
    let steps: Vec<_> = made.steps.into_iter().map(|t| t.step).collect();
    (plain && !steps.is_empty()).then_some(PathPattern { steps, target: made.target })
}

/// The branch of `compare`, of a relative path or `.` and a literal; none
/// of another.
fn compared(compare: &CompareOp) -> Option<Branch> {
    let (side, literal, flipped) = if let Op::Atomic(literal) = &compare.right {
        (&compare.left, literal, false)
    } else if let Op::Atomic(literal) = &compare.left {
        (&compare.right, literal, true)
    } else {
        return None;
    };
    let pattern = if let Op::Path(path) = side {
        relative(path)?
    } else if matches!(side, Op::ContextItem) {
        PathPattern { steps: Vec::new(), target: Target::Element }
    } else {
        return None;
    };
    let mut branch = Branch::of(pattern);
    let general = match compare.op {
        Comparator::General(op) => op,
        // err:XPTY0004 of more than one node, or of a string and a number.
        Comparator::Value(_) => {
            branch.raises = true;
            branch.dropped = true;
            return Some(branch);
        }
        Comparator::Node(_) => return None,
    };
    let op = if flipped { flip(comp_op(general)) } else { comp_op(general) };
    let literal = literal_of(literal)?;
    let textual = matches!(literal, Literal::String(_) | Literal::Untyped(_));
    if textual && compare.collation.uri() != CODEPOINT {
        branch.dropped = true;
        return Some(branch);
    }
    branch.raises = !textual;
    branch.comparisons.push(Comparison { op, literal });
    Some(branch)
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
