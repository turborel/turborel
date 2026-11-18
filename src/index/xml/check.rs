//! \[EXTENSION\] RULES.md D86: a lookup's own answer of one value, read as
//! a tree: whether a node its path selects has a value its comparisons
//! hold of [XQuery §3.2, §3.5.2]. What a caller of a lookup checks each
//! row it fetches by, as for a function lookup (RULES.md D34), and what
//! the tests of an index compare its candidates with.

use super::lookup::{Axis, Comparison, Literal, Target, XmlLookup, steps_match};
use crate::error::Result;
use crate::value::CompOp;
use crate::value::xml::XmlValue;
use crate::xml::{Atomic, AtomicType, Item, Kind, NodeId, QName, Tree, cast};

impl XmlLookup {
    /// Whether `value` has a node the lookup's path selects of which every
    /// comparison holds; nodes are taken in document order, and the first
    /// that holds answers.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, err:FORG0001, if a numeric comparison meets a node,
    /// before any that holds, whose value does not cast to xs:double
    /// [XQuery §3.5.2]; the errors of reading the value.
    pub fn matches(&self, value: &XmlValue) -> Result<bool> {
        let sequence = value.sequence()?;
        for item in sequence.items() {
            let Item::Node(node) = item else { continue };
            if self.matches_below(node.tree(), node.id())? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether a node below `root`, the item, is one it selects and holds
    /// of: the item's attributes, then each element below it and its
    /// attributes.
    fn matches_below(&self, tree: &Tree, root: NodeId) -> Result<bool> {
        let below = std::iter::once(root).chain(tree.descendants(root)).filter(|&n| tree.kind(n) == Some(Kind::Element));
        for element in below {
            if element != root && self.holds_of(tree, (root, element))? {
                return Ok(true);
            }
            for attribute in tree.attributes(element) {
                if self.holds_of(tree, (root, attribute))? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Whether `node`, below item `root`, is selected and its comparisons
    /// hold of its value.
    fn holds_of(&self, tree: &Tree, (root, node): (NodeId, NodeId)) -> Result<bool> {
        let kind = match self.pattern.target {
            Target::Element => Kind::Element,
            Target::Attribute => Kind::Attribute,
        };
        if tree.kind(node) != Some(kind) {
            return Ok(false);
        }
        let above = std::iter::successors(Some(node), |&n| tree.parent(n)).take_while(|&n| n != root);
        let mut names: Vec<Named<'_>> = above.filter_map(|n| tree.name(n)).map(Named).collect();
        names.reverse();
        let steps: Vec<(Axis, Option<Named<'_>>)> = self.pattern.steps.iter().map(|s| (s.axis, s.name.as_ref().map(Named))).collect();
        if !steps_match(&steps, &names) {
            return Ok(false);
        }
        let text = tree.string_value(node);
        for comparison in &self.comparisons {
            if !holds(comparison, &text)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// A name compared as an expanded QName, whatever its prefix.
struct Named<'n>(&'n QName);

impl PartialEq for Named<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same(other.0)
    }
}

/// Whether `comparison` holds of a node's `xdt:untypedAtomic` value
/// `text` [XQuery §3.5.2]: as a string, by code points, with a string or
/// `xdt:untypedAtomic` literal; as an xs:double with a number.
///
/// # Errors
///
/// err:FORG0001 if a number is compared and `text` does not cast to one.
fn holds(comparison: &Comparison, text: &str) -> Result<bool> {
    let order = match &comparison.literal {
        Literal::String(s) | Literal::Untyped(s) => Some(text.cmp(s.as_str())),
        Literal::Number(n) => {
            let cast = cast(&Atomic { kind: AtomicType::UntypedAtomic, lexical: text.into() }, AtomicType::Double)?;
            let number = match &*cast.lexical {
                "INF" => f64::INFINITY,
                "-INF" => f64::NEG_INFINITY,
                lexical => lexical.parse::<f64>().unwrap_or(f64::NAN),
            };
            number.partial_cmp(n)
        }
    };
    Ok(order.is_some_and(|o| match comparison.op {
        CompOp::Eq => o.is_eq(),
        CompOp::Ne => o.is_ne(),
        CompOp::Lt => o.is_lt(),
        CompOp::Le => o.is_le(),
        CompOp::Gt => o.is_gt(),
        CompOp::Ge => o.is_ge(),
    }))
}
