//! \[EXTENSION\] RULES.md D106 [XQuery §3.2.1.1]: a query's relation of an
//! element to a node selected, read as the events come: an element of the
//! name above a node is open as it is selected; one preceding it has
//! closed before it opened, not above it; one preceding it of its parent
//! has opened before it below its parent. One following it opens after it
//! ends, and one following it of its parent opens after it ends, before
//! its parent ends: a node waits for it, as for an anchor, which holds as
//! such an element opens, and does not as its item, or its parent, ends.
//! Elements are those below an item, as an index keys them.

use crate::index::xml::related::{Relation, XmlRelated};
use crate::xml::QName;

/// What an element open knows of the relation: whether it is of the
/// name; whether one above it is; whether one closed before it opened;
/// whether one of its parent opened before it; whether one of its own
/// children has opened; and the anchor of a child of it of the name not
/// yet opened, which a child's following sibling waits for.
pub(super) struct Opened {
    named: bool,
    above: bool,
    before: Before,
    children: bool,
    sibling: Option<u64>,
}

/// Whether an element of the name closed before an element opened, and
/// whether one of its parent opened before it.
struct Before {
    preceded: bool,
    sibling: bool,
}

/// The node a relation is asked of: an element closed, or an attribute of
/// the element open last.
#[derive(Clone, Copy)]
pub(super) enum Of<'o> {
    /// An element, as it was open.
    Element(&'o Opened),
    /// An attribute.
    Attribute,
}

/// What a relation answers of a node: no, yes, or once the anchor holds.
pub(super) enum Related {
    No,
    Yes,
    Waits(u64),
}

/// A relation being read.
pub(super) struct Relating<'l> {
    related: &'l XmlRelated,
    /// Of each name of the item, whether it is the relation's.
    names: Vec<bool>,
    open: Vec<Opened>,
    /// Whether an element of the name has closed in the item.
    closed: bool,
    /// The anchor of an element of the name not yet opened, which a node
    /// that ended waits for.
    following: Option<u64>,
}

impl<'l> Relating<'l> {
    /// The reading of `related`.
    pub(super) fn of(related: &'l XmlRelated) -> Relating<'l> {
        Relating { related, names: Vec::new(), open: Vec::new(), closed: false, following: None }
    }

    /// Takes `names`, the names of the item read.
    pub(super) fn named(&mut self, names: &[QName]) {
        self.names = names.iter().map(|n| self.related.name.as_ref().is_none_or(|r| r.same(n))).collect();
        self.closed = false;
    }

    /// Opens the item's root, if nothing is open, or an element named
    /// `name`: the anchors of nodes waiting for it, which it holds.
    pub(super) fn push(&mut self, name: Option<u32>) -> Vec<u64> {
        let passes = name.and_then(|n| usize::try_from(n).ok()).and_then(|n| self.names.get(n)).copied().unwrap_or(false);
        let named = passes && !self.open.is_empty();
        let (above, sibling_before) = self.open.last().map_or((false, false), |p| (p.named || p.above, p.children));
        let mut held = Vec::new();
        if named {
            held.extend(self.following.take());
            if let Some(parent) = self.open.last_mut() {
                held.extend(parent.sibling.take());
                parent.children = true;
            }
        }
        let before = Before { preceded: self.closed, sibling: sibling_before };
        self.open.push(Opened { named, above, before, children: false, sibling: None });
        held
    }

    /// The element open last, closing.
    pub(super) fn pop(&mut self) -> Option<Opened> {
        self.open.pop()
    }

    /// `closed` closed: the anchors of nodes waiting for it that do not
    /// hold, of a following sibling of a child of it, and as its item ends,
    /// of a following element.
    pub(super) fn closed(&mut self, closed: &Opened) -> Vec<u64> {
        self.closed |= closed.named;
        let mut ended: Vec<u64> = closed.sibling.into_iter().collect();
        if self.open.is_empty() {
            ended.extend(self.following.take());
        }
        ended
    }

    /// What the relation answers of `of`, an anchor made of `next` if it
    /// waits for one.
    pub(super) fn related(&mut self, of: Of<'_>, next: &mut u64) -> Related {
        let element = self.open.last();
        let known = match (self.related.relation, of) {
            (Relation::Ancestor, Of::Element(o)) => o.above,
            (Relation::AncestorOrSelf, Of::Element(o)) => o.above || o.named,
            (Relation::Ancestor | Relation::AncestorOrSelf, Of::Attribute) => element.is_some_and(|e| e.named || e.above),
            (Relation::Preceding, Of::Element(o)) => o.before.preceded,
            (Relation::Preceding, Of::Attribute) => element.is_some_and(|e| e.before.preceded),
            (Relation::PrecedingSibling, Of::Element(o)) => o.before.sibling,
            (Relation::PrecedingSibling | Relation::FollowingSibling, Of::Attribute) => false,
            (Relation::Following, Of::Element(_) | Of::Attribute) => {
                return Related::Waits(*self.following.get_or_insert_with(|| taken(next)));
            }
            (Relation::FollowingSibling, Of::Element(_)) => {
                let Some(parent) = self.open.last_mut() else { return Related::No };
                return Related::Waits(*parent.sibling.get_or_insert_with(|| taken(next)));
            }
        };
        if known { Related::Yes } else { Related::No }
    }
}

/// The number `next` holds, `next` then the one after it.
fn taken(next: &mut u64) -> u64 {
    let id = *next;
    *next = next.saturating_add(1);
    id
}
