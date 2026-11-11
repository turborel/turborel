//! The operators shared by the AST, planner and executor: `<comp op>`
//! [X3.135-1986 §5.11] and the dyadic arithmetic operators [§5.9].

use std::cmp::Ordering;

/// [X3.135-1986 §5.11] `<comp op>` ::= = | <> | < | > | <= | >=
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompOp {
    /// =
    Eq,
    /// <>
    Ne,
    /// <
    Lt,
    /// >
    Gt,
    /// <=
    Le,
    /// >=
    Ge,
}

impl CompOp {
    /// [X3.135-1986 §5.11 GR3] the truth of "x `<op>` y" given how x compares to y.
    pub fn holds(self, ord: Ordering) -> bool {
        match self {
            CompOp::Eq => ord == Ordering::Equal,
            CompOp::Ne => ord != Ordering::Equal,
            CompOp::Lt => ord == Ordering::Less,
            CompOp::Gt => ord == Ordering::Greater,
            CompOp::Le => ord != Ordering::Greater,
            CompOp::Ge => ord != Ordering::Less,
        }
    }
    /// The operator to use when the operands are swapped: "5 < X" is "X > 5".
    #[must_use]
    pub fn flip(self) -> CompOp {
        match self {
            CompOp::Lt => CompOp::Gt,
            CompOp::Gt => CompOp::Lt,
            CompOp::Le => CompOp::Ge,
            CompOp::Ge => CompOp::Le,
            CompOp::Eq | CompOp::Ne => self,
        }
    }
    /// The operator as written in SQL.
    pub fn symbol(self) -> &'static str {
        match self {
            CompOp::Eq => "=",
            CompOp::Ne => "<>",
            CompOp::Lt => "<",
            CompOp::Gt => ">",
            CompOp::Le => "<=",
            CompOp::Ge => ">=",
        }
    }
}

/// [X3.135-1986 §5.9] dyadic arithmetic operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    /// +
    Add,
    /// -
    Sub,
    /// *
    Mul,
    /// /
    Div,
}

impl ArithOp {
    /// The operator as written in SQL.
    pub fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}
