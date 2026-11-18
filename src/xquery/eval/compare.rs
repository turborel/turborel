//! Comparisons evaluated [XQuery §3.5]: value comparisons of one atomic
//! value each [XQuery §3.5.1], general comparisons of every pair of the
//! atomized operands [XQuery §3.5.2], and node comparisons by identity and
//! document order [XQuery §3.5.3]; the atomic values compared by the
//! operator mapping [XQuery Appendix B.2], strings by the collation.

use std::cmp::Ordering;

use super::super::ast::{Comparator, GeneralComp, NodeComp, ValueComp};
use super::super::collation::Collator;
use super::super::error::{ErrorCode, error};
use super::super::library::{op, value_comparison};
use super::super::plan::CompareOp;
use super::sequence::{atomize, boolean};
use super::types::cast_atomic;
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Primitive};

impl Evaluator<'_> {
    /// The value of a comparison [XQuery §3.5].
    pub(super) fn compare(&self, c: &CompareOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let left = self.eval(&c.left, frame, focus)?;
        let right = self.eval(&c.right, frame, focus)?;
        match c.op {
            Comparator::Value(op) => {
                let (Some(a), Some(b)) = (one_atomic(&left)?, one_atomic(&right)?) else { return Ok(Vec::new()) };
                Ok(boolean(self.value_compare(op, &untyped_as_string(a), &untyped_as_string(b), &c.collation)?))
            }
            Comparator::General(op) => self.general(op, &left, &right, &c.collation).map(boolean),
            Comparator::Node(op) => node_compare(op, &left, &right),
        }
    }

    /// `a op b` of two atomic values [XQuery §3.5.1, Appendix B.2].
    ///
    /// # Errors
    ///
    /// err:XPTY0004 if the operator mapping has no such combination.
    pub(super) fn value_compare(&self, op: ValueComp, a: &Atomic, b: &Atomic, collation: &Collator) -> Result<bool> {
        let order = self.order(a, b, collation, matches!(op, ValueComp::Eq | ValueComp::Ne))?;
        Ok(match op {
            ValueComp::Eq => order == Some(Ordering::Equal),
            ValueComp::Ne => order != Some(Ordering::Equal),
            ValueComp::Lt => order == Some(Ordering::Less),
            ValueComp::Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
            ValueComp::Gt => order == Some(Ordering::Greater),
            ValueComp::Ge => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        })
    }

    /// How `a` compares with `b` by the operator mapping [XQuery Appendix
    /// B.2], for equality alone if `equality`: none if they are unordered,
    /// a NaN; the library's operators (`library::value_comparison`).
    ///
    /// # Errors
    ///
    /// err:XPTY0004 if their types have no comparison.
    pub(super) fn order(&self, a: &Atomic, b: &Atomic, collation: &Collator, equality: bool) -> Result<Option<Ordering>> {
        value_comparison(a, b, collation, i32::from(self.context.timezone), equality)
    }

    /// A general comparison of `left` and `right` [XQuery §3.5.2]: true if
    /// a pair of their atomized values, the untyped converted by the
    /// other's type, compares so.
    fn general(&self, op: GeneralComp, left: &[Item], right: &[Item], collation: &Collator) -> Result<bool> {
        let value_op = match op {
            GeneralComp::Eq => ValueComp::Eq,
            GeneralComp::Ne => ValueComp::Ne,
            GeneralComp::Lt => ValueComp::Lt,
            GeneralComp::Le => ValueComp::Le,
            GeneralComp::Gt => ValueComp::Gt,
            GeneralComp::Ge => ValueComp::Ge,
        };
        let (left, right) = (atomize(left)?, atomize(right)?);
        for a in &left {
            self.check()?;
            for b in &right {
                let (x, y) = converted(a, b)?;
                if self.value_compare(value_op, &x, &y, collation)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Whether a value of `p` is compared as a string [XQuery Appendix B.1]:
/// a string, or a type derived from it, an untyped value cast to it, or an
/// `xs:anyURI` promoted to it.
fn textual(p: Primitive) -> bool {
    p.textual() || p == Primitive::AnyUri
}

/// The one atomic value `items` atomize to, none of none [XQuery §3.5.1].
///
/// # Errors
///
/// err:XPTY0004 of more than one.
fn one_atomic(items: &[Item]) -> Result<Option<Atomic>> {
    let mut atomized = atomize(items)?;
    if atomized.len() > 1 {
        return Err(error(ErrorCode::XPTY0004, "an operand of a value comparison is more than one atomic value [XQuery §3.5.1]"));
    }
    Ok(atomized.pop())
}

/// `a`, an untyped value cast to `xs:string` [XQuery §3.5.1].
fn untyped_as_string(a: Atomic) -> Atomic {
    if a.kind == AtomicType::UntypedAtomic { Atomic { kind: AtomicType::String, ..a } } else { a }
}

/// `a` and `b` as a general comparison compares them [XQuery §3.5.2]: an
/// untyped value beside a number cast to `xs:double`, beside a string or
/// another untyped value to `xs:string`, else to the other's type.
///
/// # Errors
///
/// err:FORG0001 of a cast that fails.
fn converted(a: &Atomic, b: &Atomic) -> Result<(Atomic, Atomic)> {
    let to = |x: &Atomic, other: &Atomic| -> Result<Atomic> {
        if x.kind != AtomicType::UntypedAtomic {
            return Ok(x.clone());
        }
        let target = if other.kind.primitive().numeric() {
            AtomicType::Double
        } else if textual(other.kind.primitive()) {
            AtomicType::String
        } else {
            other.kind
        };
        cast_atomic(x, target)
    };
    Ok((to(a, b)?, to(b, a)?))
}

/// A node comparison [XQuery §3.5.3]: each operand one node or none
/// \[err:XPTY0004\]; none if either is none.
fn node_compare(op: NodeComp, left: &[Item], right: &[Item]) -> Result<Seq> {
    let one = |items: &[Item]| match items {
        [] => Ok(None),
        [Item::Node(n)] => Ok(Some(n.clone())),
        _ => Err(error(ErrorCode::XPTY0004, "an operand of a node comparison is one node or none [XQuery §3.5.3]")),
    };
    let (Some(a), Some(b)) = (one(left)?, one(right)?) else { return Ok(Vec::new()) };
    Ok(boolean(match op {
        NodeComp::Is => op::is_same_node(&a, &b),
        NodeComp::Precedes => op::node_before(&a, &b),
        NodeComp::Follows => op::node_after(&a, &b),
    }))
}
