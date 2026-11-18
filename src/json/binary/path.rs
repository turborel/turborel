//! \[DESIGN\] A path of members and subscripts evaluated on a stored
//! document in place [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 to 4], so a
//! JSON_VALUE or JSON_QUERY of a value of the JSON type reads only the
//! nodes on its way and makes an item of what it selects alone.
//!
//! The path is to be lax, its wff `$` followed by member accessors,
//! wildcard member accessors, wildcard array accessors and array accessors
//! whose subscripts are integer literals, the shapes most paths have. Each
//! op is applied as the evaluator applies it in lax mode
//! (`json/path/eval/accessors.rs`): a member accessor unwraps an array of
//! its base and skips what is no object or has no member of its key,
//! taking the first of a key (UA075); an array accessor wraps what is no
//! array and takes the elements there are. Any other path is evaluated
//! on the item the document is, as JSON text's is.

use super::{Node, NodeKind};
use crate::error::Result;
use crate::json::Item;
use crate::json::path::{Accessor, Expr, Mode, Path, Subscript};
use crate::value::Value;

/// One op of a path this module evaluates.
enum Op<'p> {
    /// `.key`.
    Member(&'p str),
    /// `.*`.
    AnyMember,
    /// `[*]`.
    AnyElement,
    /// `[...]`, each subscript its first and last.
    Subscripts(Vec<(i128, i128)>),
}

/// [§9.46 GR11] The SQL/JSON sequence `path` gives of the document whose
/// root is `root`, if the path is one the module's documentation says it
/// evaluates; none if it is not.
///
/// # Errors
///
/// SQLCODE -916 if the document is damaged; -900 if what it selects
/// nests deeper than `MAX_NESTING`.
pub fn fast_path(path: &Path, root: Node<'_>) -> Result<Option<Vec<Item>>> {
    if path.mode != Mode::Lax {
        return Ok(None);
    }
    let Some(ops) = ops_of(&path.expr) else { return Ok(None) };
    let mut seq = vec![root];
    for op in &ops {
        seq = apply(op, &seq)?;
    }
    seq.iter().map(Node::item).collect::<Result<Vec<_>>>().map(Some)
}

/// The ops of `expr` from `$` on, if each is one this module evaluates.
fn ops_of(expr: &Expr) -> Option<Vec<Op<'_>>> {
    let mut ops = Vec::new();
    let mut at = expr;
    loop {
        match at {
            Expr::Context => break,
            Expr::Access(base, op) => {
                ops.push(op_of(op)?);
                at = base;
            }
            Expr::Literal(_) | Expr::Variable(_) | Expr::Current | Expr::Last | Expr::Signed(..) | Expr::Arithmetic(..) => return None,
        }
    }
    ops.reverse();
    Some(ops)
}

/// The op `op`, if this module evaluates it.
fn op_of(op: &Accessor) -> Option<Op<'_>> {
    match op {
        Accessor::Member(key) => Some(Op::Member(key)),
        Accessor::AnyMember => Some(Op::AnyMember),
        Accessor::AnyElement => Some(Op::AnyElement),
        Accessor::Subscripts(list) => list.iter().map(subscript).collect::<Option<Vec<_>>>().map(Op::Subscripts),
        Accessor::Filter(_) | Accessor::Method(_) => None,
    }
}

/// The first and last of `s`, if each is an integer literal.
fn subscript(s: &Subscript) -> Option<(i128, i128)> {
    let from = integer(&s.from)?;
    let to = match &s.to {
        Some(to) => integer(to)?,
        None => from,
    };
    Some((from, to))
}

/// The value of `expr`, if it is an exact literal of scale 0.
fn integer(expr: &Expr) -> Option<i128> {
    let Expr::Literal(Item::Scalar(s)) = expr else { return None };
    let Value::Exact(d) = s.value() else { return None };
    (d.scale == 0).then_some(d.units)
}

/// `op` applied to each node of `seq`, in lax mode.
fn apply<'a>(op: &Op<'_>, seq: &[Node<'a>]) -> Result<Vec<Node<'a>>> {
    let mut out = Vec::new();
    match op {
        Op::Member(key) => {
            for node in unwrapped(seq)? {
                out.extend(node.member(key)?);
            }
        }
        Op::AnyMember => {
            for node in unwrapped(seq)? {
                out.extend(node.members()?.unwrap_or_default().into_iter().map(|(_, value)| value));
            }
        }
        Op::AnyElement => {
            for node in seq {
                match node.elements()? {
                    Some(elements) => out.extend(elements),
                    None => out.push(*node),
                }
            }
        }
        Op::Subscripts(list) => {
            for node in seq {
                select(*node, list, &mut out)?;
            }
        }
    }
    Ok(out)
}

/// [§9.46 GR8] Unwrap of `seq`: each array's elements, one level.
fn unwrapped<'a>(seq: &[Node<'a>]) -> Result<Vec<Node<'a>>> {
    let mut out = Vec::with_capacity(seq.len());
    for node in seq {
        match node.elements()? {
            Some(elements) => out.extend(elements),
            None => out.push(*node),
        }
    }
    Ok(out)
}

/// [§9.46 GR11g ii 3] The elements of `node`, wrapped if no array, that
/// the subscripts `list` select, appended to `out`: of a range, those
/// within the array.
fn select<'a>(node: Node<'a>, list: &[(i128, i128)], out: &mut Vec<Node<'a>>) -> Result<()> {
    let wrapped = node.kind()? != NodeKind::Array;
    let len = if wrapped { 1 } else { node.len()?.unwrap_or(0) };
    let len = i128::try_from(len).unwrap_or(i128::MAX);
    for &(from, to) in list {
        let (low, high) = (from.max(0), to.min(len.saturating_sub(1)));
        let mut i = low;
        while i <= high {
            let element = if wrapped { Some(node) } else { node.element(usize::try_from(i).unwrap_or(usize::MAX))? };
            out.extend(element);
            i = i.saturating_add(1);
        }
    }
    Ok(())
}
