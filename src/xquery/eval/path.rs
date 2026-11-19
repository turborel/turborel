//! Paths, axes and predicates evaluated [XQuery §3.2, §3.3.2]: each step
//! of a path evaluated with each node before it as its focus, the nodes
//! it gives in document order each once [XQuery §3.2]; an axis's nodes
//! read by the links of their tree in document order [XQuery §3.2.1.1],
//! a reverse axis's positions counted from its last [XQuery §3.2.2].

use super::super::error::{ErrorCode, error};
use super::super::library::op;
use super::super::plan::{Axis, Op, PathOp, PathStart, StepOp, StepTest};
use super::sequence::{in_document_order, nodes};
use super::{Evaluator, Focus, Frame, Seq, ebv};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Kind, Node, NodeId, Tree};

impl Evaluator<'_> {
    /// The value of a path [XQuery §3.2].
    pub(super) fn path(&self, p: &PathOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut steps = p.steps.iter();
        let mut current = match p.start {
            PathStart::Root => {
                let root = root_of(focus)?;
                let Some(first) = steps.next() else { return Ok(vec![Item::Node(root)]) };
                self.step_of_nodes(first, vec![root], frame)?
            }
            PathStart::Context => match steps.next() {
                Some(first) => self.step(first, frame, focus)?,
                None => return focus.item.clone().map(|i| vec![i]).ok_or_else(super::no_context_item),
            },
        };
        for step in steps {
            let Some(input) = nodes(current) else {
                return Err(error(ErrorCode::XPTY0019, "a step other than the last of a path gives an atomic value [XQuery §3.2]"));
            };
            current = self.step_of_nodes(step, input, frame)?;
        }
        Ok(current)
    }

    /// `E1/E2` of the nodes `input` of E1 and the step E2 [XQuery §3.2]:
    /// E2 evaluated with each node as the focus, its nodes in document
    /// order each once, or its atomic values in order; both,
    /// err:XPTY0018.
    fn step_of_nodes(&self, step: &StepOp, input: Vec<Node>, frame: &mut Frame) -> Result<Seq> {
        let size = input.len();
        let (mut found_nodes, mut atomics) = (Vec::new(), Vec::new());
        for (n, node) in input.into_iter().enumerate() {
            self.check()?;
            let focus = Focus::of(Item::Node(node), n.saturating_add(1), size);
            for item in self.step(step, frame, &focus)? {
                match item {
                    Item::Node(found) => found_nodes.push(found),
                    Item::Atomic(_) => atomics.push(item),
                }
            }
            super::sequence::bounded(found_nodes.len().saturating_add(atomics.len()))?;
        }
        if atomics.is_empty() {
            return Ok(in_document_order(found_nodes));
        }
        if !found_nodes.is_empty() {
            return Err(error(ErrorCode::XPTY0018, "the last step of a path gives both nodes and atomic values [XQuery §3.2]"));
        }
        Ok(atomics)
    }

    /// A step evaluated in `focus` [XQuery §3.2.1]: an axis step of the
    /// context node, or a filter expression.
    fn step(&self, step: &StepOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        match step {
            StepOp::Axis(axis, test, predicates) => {
                let node = match &focus.item {
                    Some(Item::Node(node)) => node,
                    Some(Item::Atomic(_)) => {
                        return Err(error(ErrorCode::XPTY0020, "the context item of an axis step is no node [XQuery §3.2.1]"));
                    }
                    None => return Err(super::no_context_item()),
                };
                let ids: Vec<NodeId> =
                    along(node.tree(), node.id(), *axis).into_iter().filter(|&id| passes(node, id, *axis, test)).collect();
                let mut items: Seq = ids.into_iter().map(|id| Item::Node(node.at(id))).collect();
                if axis.reverse() {
                    items.reverse();
                }
                let mut kept = self.predicates(items, predicates, frame)?;
                if axis.reverse() {
                    kept.reverse();
                }
                Ok(kept)
            }
            StepOp::Filter(primary, predicates) => {
                let items = self.eval(primary, frame, focus)?;
                self.predicates(items, predicates, frame)
            }
        }
    }

    /// A filter expression [XQuery §3.3.2]: the items of its primary
    /// expression for which every predicate is true.
    pub(super) fn filter(&self, base: &Op, predicates: &[Op], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let items = self.shared(base, frame, focus)?;
        self.predicates((*items).clone(), predicates, frame)
    }

    /// `items` filtered by each predicate in turn [XQuery §3.2.2]: an item
    /// kept if the predicate's value, evaluated with the item as the
    /// focus, is a number equal to its position, or else if its effective
    /// boolean value is true.
    fn predicates(&self, mut items: Seq, predicates: &[Op], frame: &mut Frame) -> Result<Seq> {
        for predicate in predicates {
            let size = items.len();
            let mut kept = Vec::new();
            for (n, item) in items.into_iter().enumerate() {
                self.check()?;
                let position = n.saturating_add(1);
                let focus = Focus::of(item, position, size);
                if truth_of(&self.eval(predicate, frame, &focus)?, position)? {
                    kept.extend(focus.item);
                }
            }
            items = kept;
        }
        Ok(items)
    }
}

/// The predicate truth value of `value` at `position` [XQuery §3.2.2]: of
/// one number, whether it is equal to the position; else the effective
/// boolean value.
fn truth_of(value: &[Item], position: usize) -> Result<bool> {
    if let [Item::Atomic(a)] = value
        && super::super::types::numeric(a.kind)
    {
        return op::numeric_equal(a, &Atomic::new(AtomicType::Integer, position.to_string()));
    }
    ebv(value)
}

/// The root of the tree of the context node, a document node [XQuery
/// §3.2]: no context item is err:XPDY0002, one not a node
/// err:XPTY0020, a root no document node err:XPDY0050.
fn root_of(focus: &Focus) -> Result<Node> {
    let node = match &focus.item {
        Some(Item::Node(node)) => node,
        Some(Item::Atomic(_)) => {
            return Err(error(ErrorCode::XPTY0020, "the context item of a path's leading '/' is no node [XQuery §3.2]"));
        }
        None => return Err(super::no_context_item()),
    };
    let tree = node.tree();
    let root = std::iter::successors(Some(node.id()), |&id| tree.parent(id)).last().unwrap_or(node.id());
    if tree.kind(root) != Some(Kind::Document) {
        return Err(error(ErrorCode::XPDY0050, "the root of the context node's tree is no document node [XQuery §3.2]"));
    }
    Ok(node.at(root))
}

/// The nodes of `axis` from `id` [XQuery §3.2.1.1], in document order.
fn along(tree: &Tree, id: NodeId, axis: Axis) -> Vec<NodeId> {
    let attribute = tree.kind(id) == Some(Kind::Attribute);
    match axis {
        Axis::Child => tree.children(id).collect(),
        Axis::Descendant => tree.descendants(id).collect(),
        Axis::DescendantOrSelf => std::iter::once(id).chain(tree.descendants(id)).collect(),
        Axis::Attribute => tree.attributes(id).collect(),
        Axis::SelfNode => vec![id],
        Axis::Parent => tree.parent(id).into_iter().collect(),
        Axis::Ancestor => ancestors(tree, id, false),
        Axis::AncestorOrSelf => ancestors(tree, id, true),
        Axis::FollowingSibling => {
            if attribute {
                Vec::new()
            } else {
                std::iter::successors(tree.next_sibling(id), |&s| tree.next_sibling(s)).collect()
            }
        }
        Axis::PrecedingSibling => match tree.parent(id).filter(|_| !attribute) {
            Some(parent) => tree.children(parent).take_while(|&c| c != id).collect(),
            None => Vec::new(),
        },
        Axis::Following => {
            let start = if attribute { Some(id) } else { tree.following(id) };
            match start {
                Some(start) => tree.nodes().filter(|&n| n >= start && n != id && tree.kind(n) != Some(Kind::Attribute)).collect(),
                None => Vec::new(),
            }
        }
        Axis::Preceding => {
            let above = ancestors(tree, id, false);
            tree.nodes().take_while(|&n| n < id).filter(|n| tree.kind(*n) != Some(Kind::Attribute) && !above.contains(n)).collect()
        }
    }
}

/// The ancestors of `id`, and itself if `with_self`, in document order.
fn ancestors(tree: &Tree, id: NodeId, with_self: bool) -> Vec<NodeId> {
    let first = if with_self { Some(id) } else { tree.parent(id) };
    let mut up: Vec<NodeId> = std::iter::successors(first, |&a| tree.parent(a)).collect();
    up.reverse();
    up
}

/// Whether node `id` of the tree of `node` passes `test` on `axis` [XQuery
/// §3.2.1.2]: a name test of a node of the axis's principal kind and the
/// name; a kind test as it matches.
fn passes(node: &Node, id: NodeId, axis: Axis, test: &StepTest) -> bool {
    let tree = node.tree();
    match test {
        StepTest::Name(uri, local) => {
            let name = tree.name(id);
            let named = name.is_some_and(|n| uri.admits(n.uri.as_deref()) && local.as_deref().is_none_or(|l| &*n.local == l));
            tree.kind(id) == Some(axis.principal()) && named
        }
        StepTest::Kind(kind) => kind.matches(&node.at(id)),
    }
}
