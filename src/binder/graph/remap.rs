//! \[EXTENSION\] A GRAPH_TABLE's expressions moved from the binder's row
//! to its match row (`logical/graph.rs`). The binder's row is the FROM
//! row before it, then each element variable's columns, then the slots;
//! the match row is the variables' columns, the slots, then the operands:
//! every column of the FROM row before it, outer reference and dynamic
//! parameter an expression reads, each evaluated of the GRAPH_TABLE's
//! input row (`Unnest::collections`).

use std::collections::BTreeSet;

use crate::logical::{Cond, Expr};

/// The operands of a GRAPH_TABLE, as they are found.
pub(super) struct Operands {
    /// The first column of the variables in the binder's row.
    pub(super) offset: usize,
    /// The columns of the variables and the slots.
    pub(super) base: usize,
    /// The operands, over the FROM row before it.
    pub(super) exprs: Vec<Expr>,
    /// The columns of the FROM row before it they read.
    pub(super) reads: BTreeSet<usize>,
}

impl Operands {
    /// No operands yet: the variables from `offset` of the binder's row,
    /// `base` columns of variables and slots.
    pub(super) fn new(offset: usize, base: usize) -> Operands {
        Operands { offset, base, exprs: Vec::new(), reads: BTreeSet::new() }
    }

    /// The match-row column of operand `e`, made one if it is not.
    fn operand(&mut self, e: Expr) -> Expr {
        if let Expr::Column(c) = &e {
            self.reads.insert(*c);
        }
        let k = if let Some(k) = self.exprs.iter().position(|x| same(x, &e)) {
            k
        } else {
            self.exprs.push(e);
            self.exprs.len().saturating_sub(1)
        };
        Expr::Column(self.base.saturating_add(k))
    }

    /// `e` over the match row.
    pub(super) fn expr(&mut self, e: Expr) -> Expr {
        match e {
            Expr::Column(c) => {
                if c >= self.offset {
                    return Expr::Column(c.saturating_sub(self.offset));
                }
                self.operand(Expr::Column(c))
            }
            Expr::Outer(..) | Expr::Dynamic(_) => self.operand(e),
            other @ (Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_)) => other.map_children(&mut |child| self.expr(child)),
        }
    }

    /// `c` over the match row.
    pub(super) fn cond(&mut self, c: Cond) -> Cond {
        c.map_exprs(&mut |e| self.expr(e))
    }
}

/// Whether two operands are one: the same column, slot or parameter.
fn same(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Column(x), Expr::Column(y)) | (Expr::Outer(x, _), Expr::Outer(y, _)) => x == y,
        (Expr::Dynamic(x), Expr::Dynamic(y)) => x == y,
        (
            Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_),
            _,
        ) => false,
    }
}
