//! \[EXTENSION\] RULES.md D106: a path's steps taken as an index's
//! [XQuery §3.2.1]: each step of the child, descendant or attribute axis
//! over a name or `*` a step of the lookup, `descendant-or-self::node()`
//! (`//`) making the next a descendant step [XQuery §3.2.4], and
//! `self::node()` and `.` the node before; each with the conditions of its
//! node, its predicates (`conditions.rs`).

use super::super::plan::{Axis, Namespace, Op, PathOp, StepOp, StepTest};
use super::super::types::NodeKind;
use super::conditions::{Branch, branches};
use crate::index::xml::{Axis as Along, Step, Target};
use crate::xml::QName;

/// A condition of a node: a predicate as written.
#[derive(Debug)]
pub(super) enum Condition<'a> {
    /// A predicate of a step.
    Written(&'a Op),
}

impl Condition<'_> {
    /// Its branches, one of each conjunct; none if one is of another kind.
    pub(super) fn branches(self) -> Option<Vec<Branch>> {
        match self {
            Condition::Written(op) => branches(op),
        }
    }
}

/// A step taken: the lookup's step, and the conditions of its node.
#[derive(Debug)]
pub(super) struct Taken<'a> {
    pub(super) step: Step,
    pub(super) conditions: Vec<Condition<'a>>,
}

/// A path taken: the conditions of the item, its steps, what its last
/// selects, and whether a lookup of them selects exactly the nodes the
/// path does.
#[derive(Debug)]
pub(super) struct Twig<'a> {
    pub(super) root: Vec<Condition<'a>>,
    pub(super) steps: Vec<Taken<'a>>,
    pub(super) target: Target,
    pub(super) exact: bool,
}

impl<'a> Twig<'a> {
    /// The conditions of the node of its last step, or of the item.
    fn current(&mut self) -> &mut Vec<Condition<'a>> {
        match self.steps.last_mut() {
            Some(taken) => &mut taken.conditions,
            None => &mut self.root,
        }
    }
}

/// `path`'s steps taken, from the context item or `/`, which of a value's
/// one node is that node; none if a step is of another kind.
pub(super) fn twig(path: &PathOp) -> Option<Twig<'_>> {
    let mut made = Twig { root: Vec::new(), steps: Vec::new(), target: Target::Element, exact: true };
    let mut descend = false;
    for (at, step) in path.steps.iter().enumerate() {
        let last = at.saturating_add(1) == path.steps.len();
        match step {
            // `.`, of its predicates, is the node it is of, as
            // `self::node()` is.
            StepOp::Filter(Op::ContextItem, own) => {
                if descend && !own.is_empty() {
                    return None;
                }
                made.current().extend(own.iter().map(Condition::Written));
            }
            StepOp::Filter(..) => return None,
            StepOp::Axis(axis, test, own) => taken(&mut made, (*axis, test, own), (&mut descend, last))?,
        }
    }
    (!descend).then_some(made)
}

/// `made` with the step of `axis`, `test` and predicates `own` taken,
/// `descend` if a descendant step comes next, `last` if it is the last;
/// none if it cannot be taken.
fn taken<'a>(made: &mut Twig<'a>, (axis, test, own): (Axis, &StepTest, &'a [Op]), (descend, last): (&mut bool, bool)) -> Option<()> {
    let any_node = matches!(test, StepTest::Kind(NodeKind::Any));
    match axis {
        Axis::SelfNode => {
            if !any_node || (*descend && !own.is_empty()) {
                return None;
            }
            made.current().extend(own.iter().map(Condition::Written));
            Some(())
        }
        Axis::DescendantOrSelf => {
            if !any_node || last || !own.is_empty() {
                return None;
            }
            *descend = true;
            Some(())
        }
        Axis::Child | Axis::Descendant | Axis::Attribute => {
            if made.target == Target::Attribute {
                return None;
            }
            let along = if *descend || axis == Axis::Descendant { Along::Descendant } else { Along::Child };
            if axis == Axis::Attribute {
                made.target = Target::Attribute;
            }
            made.steps
                .push(Taken { step: Step { axis: along, name: named(test)?.0 }, conditions: own.iter().map(Condition::Written).collect() });
            *descend = false;
            Some(())
        }
        Axis::FollowingSibling
        | Axis::Following
        | Axis::Parent
        | Axis::Ancestor
        | Axis::PrecedingSibling
        | Axis::Preceding
        | Axis::AncestorOrSelf => None,
    }
}

/// The name a step tests: an expanded QName, or none of `*`.
struct Tested(Option<QName>);

/// The name `test` tests; none if it is a kind test, or a wildcard of a
/// part of a name.
fn named(test: &StepTest) -> Option<Tested> {
    let StepTest::Name(namespace, local) = test else { return None };
    match (namespace, local) {
        (Namespace::Any, None) => Some(Tested(None)),
        (Namespace::Absent, Some(local)) => Some(Tested(Some(QName { prefix: None, uri: None, local: local.clone() }))),
        (Namespace::Uri(uri), Some(local)) => Some(Tested(Some(QName { prefix: None, uri: Some(uri.clone()), local: local.clone() }))),
        (Namespace::Any, Some(_)) | (Namespace::Absent | Namespace::Uri(_), None) => None,
    }
}
