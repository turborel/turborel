//! Arithmetic and the sequence operators evaluated [XQuery §3.4, §3.3.1,
//! §3.3.3]: each arithmetic operand atomized, one value or none, an
//! untyped one cast to `xs:double`, then the operator mapping's function
//! [XQuery Appendix B.2]; ranges of integers; and the union, intersection
//! and difference of node sequences in document order.

use std::cmp::Ordering;

use super::super::error::{ErrorCode, error};
use super::super::plan::{Arith, Op};
use super::sequence::{atomize, bounded, in_document_order, nodes};
use super::types::cast_atomic;
use super::{Evaluator, Focus, Frame, Seq, numeric, temporal};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Node};

/// What an arithmetic operand is: none, or one atomic value.
type Operand = Option<Atomic>;

impl Evaluator<'_> {
    /// An arithmetic chain [XQuery §3.4], left to right.
    pub(super) fn arithmetic(&self, first: &Op, rest: &[(Arith, Op)], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut value = operand(&self.eval(first, frame, focus)?)?;
        for (op, next) in rest {
            let right = operand(&self.eval(next, frame, focus)?)?;
            value = match (value, right) {
                (Some(a), Some(b)) => Some(self.binary(*op, &a, &b)?),
                (None | Some(_), None) | (None, Some(_)) => None,
            };
        }
        Ok(value.map(Item::Atomic).into_iter().collect())
    }

    /// `a op b` [XQuery Appendix B.2]: numbers by op:numeric-add and the
    /// rest, dates, times and durations by their functions.
    ///
    /// # Errors
    ///
    /// err:XPTY0004 if the mapping has no such combination; the
    /// function's own.
    fn binary(&self, op: Arith, a: &Atomic, b: &Atomic) -> Result<Atomic> {
        if let (Some(x), Some(y)) = (numeric::of(a)?, numeric::of(b)?) {
            return Ok(numeric::atomic(numeric::arithmetic(op, x, y)?));
        }
        temporal::arithmetic(op, a, b, self.context.timezone)?.ok_or_else(|| {
            let why = format!("{} and {} have no arithmetic operator so [XQuery Appendix B.2]", a.kind.name(), b.kind.name());
            error(ErrorCode::XPTY0004, &why)
        })
    }

    /// A unary operator [XQuery §3.4]: of a number, `-` its negation and
    /// `+` itself [XQueryFO §6.2.7, §6.2.8].
    pub(super) fn unary(&self, negate: bool, operand_op: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let Some(a) = operand(&self.eval(operand_op, frame, focus)?)? else { return Ok(Vec::new()) };
        let Some(n) = numeric::of(&a)? else {
            return Err(error(
                ErrorCode::XPTY0004,
                &format!("a unary operator is of a number, not {} [XQuery Appendix B.2]", a.kind.name()),
            ));
        };
        let result = if negate { numeric::negated(n)? } else { n };
        Ok(vec![Item::Atomic(numeric::atomic(result))])
    }

    /// A range [XQuery §3.3.1]: each operand converted as an argument of
    /// type `xs:integer?`; none if either is none or the first is the
    /// greater; else every integer from the first to the second.
    pub(super) fn range(&self, operands: &[Op; 2], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let [from, to] = operands;
        let (Some(from), Some(to)) = (integer(&self.eval(from, frame, focus)?)?, integer(&self.eval(to, frame, focus)?)?) else {
            return Ok(Vec::new());
        };
        if from > to {
            return Ok(Vec::new());
        }
        let count = to.checked_sub(from).and_then(|d| d.checked_add(1)).and_then(|n| usize::try_from(n).ok()).unwrap_or(usize::MAX);
        bounded(count)?;
        Ok((from..=to).map(|i| Item::Atomic(Atomic { kind: AtomicType::Integer, lexical: i.to_string().into() })).collect())
    }

    /// `union` [XQuery §3.3.3]: every node of its operands, in document
    /// order, each once.
    pub(super) fn union(&self, operands: &[Op], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut all = Vec::new();
        for op in operands {
            all.extend(node_operand(self.eval(op, frame, focus)?)?);
            bounded(all.len())?;
        }
        Ok(in_document_order(all))
    }

    /// `intersect` and `except` [XQuery §3.3.3], left to right: the nodes
    /// of the first in the second, or not in it, in document order.
    pub(super) fn intersect_except(
        &self,
        first: &Op,
        rest: &[(super::super::ast::SetOp, Op)],
        frame: &mut Frame,
        focus: &Focus,
    ) -> Result<Seq> {
        let mut value = sorted(node_operand(self.eval(first, frame, focus)?)?);
        for (op, next) in rest {
            let other = sorted(node_operand(self.eval(next, frame, focus)?)?);
            let keep = *op == super::super::ast::SetOp::Intersect;
            value.retain(|n| other.binary_search_by(|o| o.document_order(n)).is_ok() == keep);
        }
        Ok(value.into_iter().map(Item::Node).collect())
    }
}

/// `nodes` in document order, each once.
fn sorted(mut nodes: Vec<Node>) -> Vec<Node> {
    nodes.sort_by(Node::document_order);
    nodes.dedup_by(|a, b| a.document_order(b) == Ordering::Equal);
    nodes
}

/// The nodes of an operand of `union`, `intersect` or `except` [XQuery
/// §3.3.3]: err:XPTY0004 if one item is no node.
fn node_operand(items: Seq) -> Result<Vec<Node>> {
    nodes(items)
        .ok_or_else(|| error(ErrorCode::XPTY0004, "an operand of union, intersect or except holds an item that is no node [XQuery §3.3.3]"))
}

/// An arithmetic operand [XQuery §3.4]: atomized, none or one value
/// \[err:XPTY0004\], an untyped one cast to `xs:double` \[err:FORG0001\].
fn operand(items: &[Item]) -> Result<Operand> {
    let mut atomized = atomize(items);
    if atomized.len() > 1 {
        return Err(error(ErrorCode::XPTY0004, "an arithmetic operand is more than one atomic value [XQuery §3.4]"));
    }
    let Some(a) = atomized.pop() else { return Ok(None) };
    if a.kind == AtomicType::UntypedAtomic { cast_atomic(&a, AtomicType::Double).map(Some) } else { Ok(Some(a)) }
}

/// An operand of a range, converted as an argument of type `xs:integer?`
/// [XQuery §3.3.1, §3.1.5]: an untyped value cast to `xs:integer`, another
/// an `xs:integer` \[err:XPTY0004\].
fn integer(items: &[Item]) -> Result<Option<i128>> {
    let mut atomized = atomize(items);
    if atomized.len() > 1 {
        return Err(error(ErrorCode::XPTY0004, "an operand of a range is more than one atomic value [XQuery §3.3.1]"));
    }
    let Some(a) = atomized.pop() else { return Ok(None) };
    let a = if a.kind == AtomicType::UntypedAtomic { cast_atomic(&a, AtomicType::Integer)? } else { a };
    if !a.kind.derives_from(AtomicType::Integer) {
        return Err(error(ErrorCode::XPTY0004, &format!("an operand of a range is an xs:integer, not {} [XQuery §3.3.1]", a.kind.name())));
    }
    match numeric::of(&a)? {
        Some(numeric::Num::Integer(i)) => Ok(Some(i)),
        Some(numeric::Num::Decimal(..) | numeric::Num::Float(_) | numeric::Num::Double(_)) | None => Ok(None),
    }
}
