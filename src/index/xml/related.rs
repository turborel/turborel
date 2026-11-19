//! \[EXTENSION\] RULES.md D106 [XQuery §3.2.1.1]: a query's node related
//! to an element of a name, or of any, by the last step of a path: its
//! ancestor, following, preceding or sibling. The primary answers it by
//! the nodes' ORDPATHs (`primary/related.rs`), the check as the events
//! come (`check/reading/relating.rs`).

use super::lookup::{Axis, PathPattern, Step, Target};
use crate::xml::QName;

/// \[EXTENSION\] How an element is related to a node a query selects
/// [XQuery §3.2.1.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// `ancestor`: an element above the node.
    Ancestor,
    /// `ancestor-or-self`: the node, an element, or one above it.
    AncestorOrSelf,
    /// `following`: an element after the node and its descendants.
    Following,
    /// `following-sibling`: an element after the node, of its parent.
    FollowingSibling,
    /// `preceding`: an element before the node, not above it.
    Preceding,
    /// `preceding-sibling`: an element before the node, of its parent.
    PrecedingSibling,
}

/// \[EXTENSION\] RULES.md D106: of a query, a node it selects holds only
/// if an element of a name, or of any, stands in a relation to it, as the
/// last step `following::name` selects of the path before.
#[derive(Debug, Clone)]
pub struct XmlRelated {
    /// The relation.
    pub relation: Relation,
    /// The element's expanded name, its prefix ignored; none for any.
    pub name: Option<QName>,
}

impl XmlRelated {
    /// The path of the elements it relates a node to, from the item:
    /// `//name`.
    #[must_use]
    pub fn pattern(&self) -> PathPattern {
        PathPattern { steps: vec![Step { axis: Axis::Descendant, name: self.name.clone() }], target: Target::Element }
    }
}
