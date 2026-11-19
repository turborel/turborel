//! `fn:deep-equal` [XQueryFO §15.3.1]: whether two sequences have items
//! deep-equal in pairs.
//!
//! \[DESIGN\] RULES.md D106: two trees are compared by a list of the pairs
//! of nodes left to compare, each pair's children added to it, so no
//! comparison recurses, however deep the trees. An element no schema
//! typed has mixed content [XQueryDM §6.2.3], so its element and text
//! children are compared, its comments and processing instructions not.

use super::super::collation::Collator;
use super::super::functions::Context;
use super::Builtin;
use super::args::arg;
use super::args::collator;
use super::comparison::value_equal;
use crate::error::Result;
use crate::xml::{Atomic, Item, Kind, Node, NodeId, Sequence, Tree};

/// The functions of [XQueryFO §15.3.1].
pub(super) const FUNCTIONS: [Builtin; 1] = [Builtin::function("deep-equal", 2, 3, deep_equal)];

/// How two items are compared: by a collation, in the implicit time zone.
struct Comparer {
    collation: Collator,
    zone: i32,
}

impl Comparer {
    /// Whether atomic values `a` and `b` are deep-equal: `eq`, or both
    /// NaN; false if `eq` is not defined for them.
    fn atomic(&self, a: &Atomic, b: &Atomic) -> Result<bool> {
        Ok(value_equal(a, b, &self.collation, self.zone)? == Some(true) || (super::numeric::is_nan(a) && super::numeric::is_nan(b)))
    }

    /// Whether strings `a` and `b` are equal by the collation.
    fn text(&self, a: &str, b: &str) -> bool {
        self.collation.equal(a, b)
    }

    /// Whether nodes `a` and `b` are deep-equal, every pair below them
    /// compared from a list.
    fn nodes(&self, first: &Node, second: &Node) -> bool {
        let (ours, theirs) = (first.tree(), second.tree());
        let mut left = vec![(first.id(), second.id())];
        while let Some((mine, other)) = left.pop() {
            if !self.shallow((ours, mine), (theirs, other)) {
                return false;
            }
            let (below_mine, below_other) = (content(ours, mine), content(theirs, other));
            if below_mine.len() != below_other.len() {
                return false;
            }
            left.extend(below_mine.into_iter().zip(below_other));
        }
        true
    }

    /// Whether node `mine` of `ours` and `other` of `theirs` are deep-equal
    /// but for their content: of one kind and name; elements of the same
    /// attributes, each of one name and value, in any order; text,
    /// comments and processing instructions of the same string value
    /// [XQueryFO §15.3.1].
    fn shallow(&self, (ours, mine): (&Tree, NodeId), (theirs, other): (&Tree, NodeId)) -> bool {
        let named_alike = match (ours.name(mine), theirs.name(other)) {
            (Some(left), Some(right)) => left.same(right),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        if ours.kind(mine) != theirs.kind(other) || !named_alike {
            return false;
        }
        match ours.kind(mine) {
            Some(Kind::Element) => self.attributes((ours, mine), (theirs, other)),
            Some(Kind::Attribute | Kind::Text | Kind::Comment | Kind::ProcessingInstruction) => {
                self.text(ours.content(mine), theirs.content(other))
            }
            Some(Kind::Document) | None => true,
        }
    }

    /// Whether elements `x` of `s` and `y` of `t` have the same number of
    /// attributes, each of `x`'s deep-equal to one of `y`'s.
    fn attributes(&self, (s, x): (&Tree, NodeId), (t, y): (&Tree, NodeId)) -> bool {
        let theirs: Vec<NodeId> = t.attributes(y).collect();
        s.attributes(x).count() == theirs.len()
            && s.attributes(x).all(|a| {
                theirs.iter().any(|&b| s.name(a).zip(t.name(b)).is_some_and(|(p, q)| p.same(q)) && self.text(s.content(a), t.content(b)))
            })
    }

    /// Whether items `a` and `b` are deep-equal.
    fn items(&self, a: &Item, b: &Item) -> Result<bool> {
        match (a, b) {
            (Item::Atomic(x), Item::Atomic(y)) => self.atomic(x, y),
            (Item::Node(x), Item::Node(y)) => Ok(self.nodes(x, y)),
            (Item::Atomic(_), Item::Node(_)) | (Item::Node(_), Item::Atomic(_)) => Ok(false),
        }
    }
}

/// The children of document or element node `id` that deep-equal
/// compares: its elements and text nodes, `$i/(*|text())`; none of another
/// node.
fn content(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    if !matches!(tree.kind(id), Some(Kind::Document | Kind::Element)) {
        return Vec::new();
    }
    tree.children(id).filter(|&c| matches!(tree.kind(c), Some(Kind::Element | Kind::Text))).collect()
}

/// Whether `a` and `b` are deep-equal [XQueryFO §15.3.1], strings compared
/// by `collation` and dates and times in the implicit time zone `zone`.
///
/// # Errors
///
/// As the value comparisons of atomic values.
pub fn sequences_deep_equal(a: &Sequence, b: &Sequence, collation: Collator, zone: i32) -> Result<bool> {
    if a.items().len() != b.items().len() {
        return Ok(false);
    }
    let comparer = Comparer { collation, zone };
    for (x, y) in a.items().iter().zip(b.items()) {
        if !comparer.items(x, y)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// [XQueryFO §15.3.1] `fn:deep-equal($parameter1 as item()*, $parameter2
/// as item()*, $collation as xs:string) as xs:boolean`.
///
/// # Errors
///
/// err:FOCH0002 of a collation not supported.
fn deep_equal(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let collation = collator(focus, args, 2)?;
    Ok(super::args::boolean(sequences_deep_equal(arg(args, 0), arg(args, 1), collation, i32::from(focus.timezone()))?))
}

#[cfg(test)]
mod tests;
