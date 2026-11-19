//! \[EXTENSION\] RULES.md D106: a path's steps taken as an index's
//! [XQuery §3.2.1], each with the conditions of its node, its predicates
//! (`conditions.rs`):
//!
//! - a step of the child, descendant or attribute axis, of a name, `*`,
//!   or `element()` or `attribute()` of a name or none and of no type, is
//!   a step of the lookup; `descendant-or-self::node()` (`//`) makes the
//!   next a descendant step [XQuery §3.2.4], and of the item, a document
//!   node, `descendant-or-self` of a name is `descendant` of it;
//! - `self::node()` and `.` are the node before; `self` of a name narrows
//!   the name the step before tests, none if they differ;
//! - `parent` of a node a child or attribute step selects is the node of
//!   the step before, which has such a child: that step is taken back, a
//!   condition of the step before, as `a/b/..` is `a[b]`, its name tested
//!   there [Pal et al. §2.2, §3.1.2];
//! - as the last step, `ancestor` or `ancestor-or-self` of a name a step
//!   it reaches tests, or of any name and a step it reaches, selects a
//!   node whenever the path before does, so the path is the lookup,
//!   exactly; of another name, and `following`, `following-sibling`,
//!   `preceding` and `preceding-sibling` of a name or any, a node only if
//!   an element of that name stands in that relation to a node the path
//!   before selects: the query's relation (`index::xml::XmlRelated`),
//!   exactly, which the primary answers by the nodes' ORDPATHs [Pal et al.
//!   §2.2]; of a kind test, or a last step of the child or descendant axis
//!   of `text()`, `comment()`, `node()` or `processing-instruction()`, a
//!   node only if the path before selects one, its lookup, not exactly.

use super::super::plan::{Axis, Namespace, Op, PathOp, StepOp, StepTest};
use super::super::types::{ElementKind, NodeKind};
use super::conditions::{Branch, branches};
use crate::index::xml::{Axis as Along, PathPattern, Relation, Step, Target, XmlRelated};
use crate::xml::QName;

/// A condition of a node.
#[derive(Debug)]
pub(super) enum Condition<'a> {
    /// A predicate of a step, as written.
    Written(&'a Op),
    /// A step taken back by a `parent` step, and what is below it.
    Made(Branch),
}

impl Condition<'_> {
    /// Its branches, one of each conjunct; none if one is of another kind.
    pub(super) fn branches(self) -> Option<Vec<Branch>> {
        match self {
            Condition::Written(op) => branches(op),
            Condition::Made(branch) => Some(vec![branch]),
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
/// selects, whether a lookup of them selects exactly the nodes the path
/// does, and the relation of an element to its last node, if a last step
/// makes one.
#[derive(Debug)]
pub(super) struct Twig<'a> {
    pub(super) root: Vec<Condition<'a>>,
    pub(super) steps: Vec<Taken<'a>>,
    pub(super) target: Target,
    pub(super) exact: bool,
    pub(super) related: Option<XmlRelated>,
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

/// A step to take: its axis, its node test and its predicates; whether a
/// descendant step comes next, of `//` before it; whether it is the
/// path's last.
#[derive(Clone, Copy)]
struct Stepping<'s, 'a> {
    axis: Axis,
    test: &'s StepTest,
    own: &'a [Op],
    descend: bool,
    last: bool,
}

/// The name a step tests: an expanded QName, or none of any.
struct Tested(Option<QName>);

/// `path`'s steps taken, from the context item or `/`, which of a value's
/// one node is that node; none if a step is of another kind.
pub(super) fn twig(path: &PathOp) -> Option<Twig<'_>> {
    let mut made = Twig { root: Vec::new(), steps: Vec::new(), target: Target::Element, exact: true, related: None };
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
            StepOp::Axis(axis, test, own) => {
                descend = taken(&mut made, Stepping { axis: *axis, test, own, descend, last })?;
            }
        }
    }
    (!descend).then_some(made)
}

/// `made` with `stepping` taken; whether a descendant step comes next;
/// none if it cannot be taken.
fn taken<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>) -> Option<bool> {
    let any_node = matches!(stepping.test, StepTest::Kind(NodeKind::Any));
    match stepping.axis {
        Axis::SelfNode => self_step(made, stepping, any_node),
        Axis::DescendantOrSelf => {
            if any_node && !stepping.last && stepping.own.is_empty() {
                return Some(true);
            }
            from_item(made, stepping).map(|()| false)
        }
        Axis::Child | Axis::Descendant | Axis::Attribute => forward(made, stepping).map(|()| false),
        Axis::Parent => parent(made, stepping).map(|()| false),
        Axis::Ancestor | Axis::AncestorOrSelf => ancestor(made, stepping).map(|()| false),
        Axis::FollowingSibling | Axis::Following | Axis::PrecedingSibling | Axis::Preceding => elsewhere(made, stepping).map(|()| false),
    }
}

/// A step of the self axis: `self::node()` the node before, its
/// predicates the node's; `self` of a name narrowing it.
fn self_step<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>, any_node: bool) -> Option<bool> {
    if !any_node {
        return narrowed(made, stepping).map(|()| false);
    }
    if stepping.descend && !stepping.own.is_empty() {
        return None;
    }
    made.current().extend(stepping.own.iter().map(Condition::Written));
    Some(stepping.descend)
}

/// The name a step of `axis` tests by `test`: of a name test, or a kind
/// test of an element, or along the attribute axis of an attribute, of a
/// name or none and of no type; none if it is another, or a wildcard of a
/// part of a name.
fn named(axis: Axis, test: &StepTest) -> Option<Tested> {
    match test {
        StepTest::Name(namespace, local) => match (namespace, local) {
            (Namespace::Any, None) => Some(Tested(None)),
            (Namespace::Absent, Some(local)) => Some(Tested(Some(QName { prefix: None, uri: None, local: local.clone() }))),
            (Namespace::Uri(uri), Some(local)) => Some(Tested(Some(QName { prefix: None, uri: Some(uri.clone()), local: local.clone() }))),
            (Namespace::Any, Some(_)) | (Namespace::Absent | Namespace::Uri(_), None) => None,
        },
        StepTest::Kind(NodeKind::Element(kind)) => untyped(kind).filter(|_| axis != Axis::Attribute),
        StepTest::Kind(NodeKind::Attribute(kind)) => {
            (axis == Axis::Attribute && kind.annotation.is_none()).then(|| Tested(kind.name.clone()))
        }
        StepTest::Kind(NodeKind::Any | NodeKind::Document(_) | NodeKind::Pi(_) | NodeKind::Comment | NodeKind::Text) => None,
    }
}

/// The name an element test of `kind` tests, if it tests a name alone.
fn untyped(kind: &ElementKind) -> Option<Tested> {
    (kind.annotation.is_none() && kind.substitutes.is_empty()).then(|| Tested(kind.name.clone()))
}

/// `made` with a step of the child, descendant or attribute axis taken; a
/// last one of a kind test of no name the path before's, not exactly.
fn forward<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>) -> Option<()> {
    if made.target == Target::Attribute {
        return None;
    }
    let Some(Tested(name)) = named(stepping.axis, stepping.test) else { return trailing(made, stepping) };
    let along = if stepping.descend || stepping.axis == Axis::Descendant { Along::Descendant } else { Along::Child };
    if stepping.axis == Axis::Attribute {
        made.target = Target::Attribute;
    }
    made.steps.push(Taken { step: Step { axis: along, name }, conditions: stepping.own.iter().map(Condition::Written).collect() });
    Some(())
}

/// A last step of `text()`, `comment()`, `node()` or
/// `processing-instruction()`, of no predicate: a node it selects is of a
/// node the path before selects, so the path before is its lookup, not
/// exactly.
fn trailing(made: &mut Twig<'_>, stepping: Stepping<'_, '_>) -> Option<()> {
    let StepTest::Kind(NodeKind::Any | NodeKind::Pi(_) | NodeKind::Comment | NodeKind::Text) = stepping.test else { return None };
    if !stepping.last || !stepping.own.is_empty() || made.steps.is_empty() || stepping.axis == Axis::Attribute {
        return None;
    }
    made.exact = false;
    Some(())
}

/// `descendant-or-self` of a name, of the item, a document node, which
/// is no element: `descendant` of it.
fn from_item<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>) -> Option<()> {
    if !made.steps.is_empty() || !made.root.is_empty() || matches!(stepping.test, StepTest::Kind(NodeKind::Any)) {
        return None;
    }
    forward(made, Stepping { axis: Axis::Descendant, ..stepping })
}

/// `self` of a name: the name the step before tests narrowed to it, its
/// predicates the step's; after `//` of the item, `descendant` of it;
/// none of the item, of an attribute, or of names apart.
fn narrowed<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>) -> Option<()> {
    if stepping.descend {
        return from_item(made, Stepping { axis: Axis::Descendant, descend: false, ..stepping });
    }
    let Tested(name) = named(Axis::Child, stepping.test)?;
    if made.target == Target::Attribute {
        return None;
    }
    let taken = made.steps.last_mut()?;
    taken.step.name = both(taken.step.name.take(), name)?.0;
    taken.conditions.extend(stepping.own.iter().map(Condition::Written));
    Some(())
}

/// The name both tests of `one` and `other` pass, none of either of any;
/// none if they differ.
fn both(one: Option<QName>, other: Option<QName>) -> Option<Tested> {
    match (one, other) {
        (None, other) => Some(Tested(other)),
        (Some(one), None) => Some(Tested(Some(one))),
        (Some(one), Some(other)) => one.same(&other).then_some(Tested(Some(one))),
    }
}

/// `parent`: the last step, a child or attribute step, taken back, a
/// condition of the step before, whose name the parent's test narrows;
/// none of a descendant step, of the item, or of a step taken back of
/// more than one condition.
fn parent<'a>(made: &mut Twig<'a>, stepping: Stepping<'_, 'a>) -> Option<()> {
    if stepping.descend {
        return None;
    }
    let name = if matches!(stepping.test, StepTest::Kind(NodeKind::Any)) { None } else { Some(named(Axis::Child, stepping.test)?.0) };
    let popped = made.steps.pop()?;
    if popped.step.axis != Along::Child {
        // A node a descendant step selects has a parent, an element or the
        // item, so a last `..` of it selects a node exactly when the path
        // before does.
        let alone = stepping.last && name.is_none() && stepping.own.is_empty();
        made.steps.push(popped);
        return alone.then_some(());
    }
    let mut branch = Branch::of(PathPattern { steps: vec![popped.step], target: made.target });
    made.target = Target::Element;
    let mut conditions = popped.conditions.into_iter();
    if let Some(below) = conditions.next() {
        let [below] = <[Branch; 1]>::try_from(below.branches()?).ok()?;
        branch = branch.then(below)?;
    }
    if conditions.next().is_some() {
        return None;
    }
    if let Some(name) = name {
        let taken = made.steps.last_mut()?;
        taken.step.name = both(taken.step.name.take(), name)?.0;
    }
    let current = made.current();
    current.push(Condition::Made(branch));
    current.extend(stepping.own.iter().map(Condition::Written));
    Some(())
}

/// A last step of `ancestor` or `ancestor-or-self`, of no predicate: of
/// `node()`, of a name a step it reaches tests, or of any name and a step
/// it reaches, the path alone, exactly; of another name, the path and the
/// condition of the item that an element of that name is below it, not
/// exactly.
fn ancestor(made: &mut Twig<'_>, stepping: Stepping<'_, '_>) -> Option<()> {
    if !stepping.last || stepping.descend || !stepping.own.is_empty() || made.steps.is_empty() {
        return None;
    }
    if matches!(stepping.test, StepTest::Kind(NodeKind::Any)) {
        return Some(());
    }
    let Tested(name) = named(Axis::Child, stepping.test)?;
    // An attribute is no element, and its own element is an ancestor of it.
    let own = stepping.axis == Axis::AncestorOrSelf && made.target == Target::Element;
    let reached = made.steps.len().saturating_sub(usize::from(!own));
    let tests = |taken: &Taken<'_>| name.as_ref().is_none_or(|n| taken.step.name.as_ref().is_some_and(|t| t.same(n)));
    if !made.steps.iter().take(reached).any(tests) {
        let relation = if stepping.axis == Axis::Ancestor { Relation::Ancestor } else { Relation::AncestorOrSelf };
        made.related = Some(XmlRelated { relation, name });
    }
    Some(())
}

/// A last step of `following`, `preceding` or their siblings, of no
/// predicate: of a name or any, the path and its relation; of a kind
/// test, the path, not exactly.
fn elsewhere(made: &mut Twig<'_>, stepping: Stepping<'_, '_>) -> Option<()> {
    if !stepping.last || stepping.descend || !stepping.own.is_empty() || made.steps.is_empty() {
        return None;
    }
    let StepTest::Name(..) = stepping.test else {
        made.exact = false;
        return Some(());
    };
    let relation = match stepping.axis {
        Axis::Following => Relation::Following,
        Axis::FollowingSibling => Relation::FollowingSibling,
        Axis::Preceding => Relation::Preceding,
        Axis::PrecedingSibling => Relation::PrecedingSibling,
        Axis::Child
        | Axis::Descendant
        | Axis::Attribute
        | Axis::SelfNode
        | Axis::DescendantOrSelf
        | Axis::Parent
        | Axis::Ancestor
        | Axis::AncestorOrSelf => return None,
    };
    made.related = Some(XmlRelated { relation, name: named(Axis::Child, stepping.test)?.0 });
    Some(())
}
