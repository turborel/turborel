//! The operators [XQuery Appendix A.1 \[46\] to \[59\]], by their
//! precedence [Appendix A.4]: the binary operators from `or` to
//! `intersect` and `except` by precedence climbing, each chain of one
//! precedence one node; then `instance of`, `treat`, `castable` and
//! `cast`, each at most once and in that order; then the unary signs.

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{AddOp, Comparator, Comparison, Expr, GeneralComp, MulOp, NodeComp, SetOp, Sign, ValueComp};
use crate::xquery::keywords::{
    AND, AS, CAST, CASTABLE, DIV, EQ, EXCEPT, GE, GT, IDIV, INSTANCE, INTERSECT, IS, LE, LT, MOD, NE, OF, OR, TO, TREAT, UNION,
};
use crate::xquery::lexer::{Sym, Tok};

/// A precedence of the binary operators, lowest first [XQuery Appendix
/// A.4].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    /// `or` \[46\].
    Or,
    /// `and` \[47\].
    And,
    /// The comparisons \[48\].
    Comparison,
    /// `to` \[49\].
    Range,
    /// `+` and `-` \[50\].
    Additive,
    /// `*`, `div`, `idiv` and `mod` \[51\].
    Multiplicative,
    /// `union` and `|` \[52\].
    Union,
    /// `intersect` and `except` \[53\].
    IntersectExcept,
}

impl Level {
    /// The precedence just above this one, none above the highest.
    fn above(self) -> Option<Level> {
        match self {
            Level::Or => Some(Level::And),
            Level::And => Some(Level::Comparison),
            Level::Comparison => Some(Level::Range),
            Level::Range => Some(Level::Additive),
            Level::Additive => Some(Level::Multiplicative),
            Level::Multiplicative => Some(Level::Union),
            Level::Union => Some(Level::IntersectExcept),
            Level::IntersectExcept => None,
        }
    }
}

/// A binary operator [XQuery Appendix A.1 \[46\] to \[53\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binary {
    /// `or`.
    Or,
    /// `and`.
    And,
    /// A comparison.
    Compare(Comparator),
    /// `to`.
    To,
    /// `+` or `-`.
    Add(AddOp),
    /// `*`, `div`, `idiv` or `mod`.
    Mul(MulOp),
    /// `union` or `|`.
    Union,
    /// `intersect` or `except`.
    Set(SetOp),
}

impl Binary {
    /// Its precedence.
    fn level(self) -> Level {
        match self {
            Binary::Or => Level::Or,
            Binary::And => Level::And,
            Binary::Compare(_) => Level::Comparison,
            Binary::To => Level::Range,
            Binary::Add(_) => Level::Additive,
            Binary::Mul(_) => Level::Multiplicative,
            Binary::Union => Level::Union,
            Binary::Set(_) => Level::IntersectExcept,
        }
    }
}

/// [XQuery Appendix A.1 \[50\] to \[52\], \[60\], \[62\]] The binary
/// operators written as symbols.
const SYMBOL_OPERATORS: [(Sym, Binary); 12] = [
    (Sym::Eq, Binary::Compare(Comparator::General(GeneralComp::Eq))),
    (Sym::Ne, Binary::Compare(Comparator::General(GeneralComp::Ne))),
    (Sym::Lt, Binary::Compare(Comparator::General(GeneralComp::Lt))),
    (Sym::Le, Binary::Compare(Comparator::General(GeneralComp::Le))),
    (Sym::Gt, Binary::Compare(Comparator::General(GeneralComp::Gt))),
    (Sym::Ge, Binary::Compare(Comparator::General(GeneralComp::Ge))),
    (Sym::Precedes, Binary::Compare(Comparator::Node(NodeComp::Precedes))),
    (Sym::Follows, Binary::Compare(Comparator::Node(NodeComp::Follows))),
    (Sym::Plus, Binary::Add(AddOp::Plus)),
    (Sym::Minus, Binary::Add(AddOp::Minus)),
    (Sym::Star, Binary::Mul(MulOp::Times)),
    (Sym::Bar, Binary::Union),
];

/// [XQuery Appendix A.1 \[46\] to \[53\], \[61\], \[62\]] The binary
/// operators written as keywords.
const KEYWORD_OPERATORS: [(&str, Binary); 16] = [
    (OR, Binary::Or),
    (AND, Binary::And),
    (EQ, Binary::Compare(Comparator::Value(ValueComp::Eq))),
    (NE, Binary::Compare(Comparator::Value(ValueComp::Ne))),
    (LT, Binary::Compare(Comparator::Value(ValueComp::Lt))),
    (LE, Binary::Compare(Comparator::Value(ValueComp::Le))),
    (GT, Binary::Compare(Comparator::Value(ValueComp::Gt))),
    (GE, Binary::Compare(Comparator::Value(ValueComp::Ge))),
    (IS, Binary::Compare(Comparator::Node(NodeComp::Is))),
    (TO, Binary::To),
    (DIV, Binary::Mul(MulOp::Div)),
    (IDIV, Binary::Mul(MulOp::Idiv)),
    (MOD, Binary::Mul(MulOp::Mod)),
    (UNION, Binary::Union),
    (INTERSECT, Binary::Set(SetOp::Intersect)),
    (EXCEPT, Binary::Set(SetOp::Except)),
];

impl Parser {
    /// `OrExpr` [XQuery Appendix A.1 \[46\]] and the binary operators
    /// below it.
    pub(super) fn or_expr(&mut self) -> Result<Expr> {
        self.binary(Level::Or)
    }

    /// The expression of the operators of precedence `min` and above:
    /// operands, each of the operators above `min` and the postfix ones,
    /// joined by the operators of `min` and above, each precedence a
    /// chain [XQuery Appendix A.4].
    fn binary(&mut self, min: Level) -> Result<Expr> {
        let left = self.instanceof_expr()?;
        self.binary_rest(min, left)
    }

    /// `left`, joined by the operators of precedence `min` and above
    /// after it to their operands. Apart from `binary`, so the frame of
    /// each level a query nests holds only what its recursion needs
    /// (RULES.md R-CF-6).
    fn binary_rest(&mut self, min: Level, first: Expr) -> Result<Expr> {
        let mut left = first;
        while let Some(op) = self.binary_operator().filter(|op| op.level() >= min) {
            let at = self.here();
            self.ahead.pop_front();
            let right = match op.level().above() {
                Some(next) => self.nested(|p| p.binary(next))?,
                None => self.instanceof_expr()?,
            };
            left = joined(left, op, right).ok_or_else(|| {
                self.lexer.error(at, "a comparison or range has two operands, neither one itself [XQuery Appendix A.1 [48], [49]]")
            })?;
        }
        Ok(left)
    }

    /// The binary operator the next token is, in an operator's place.
    fn binary_operator(&mut self) -> Option<Binary> {
        match self.peek(0)? {
            Tok::Sym(sym) => SYMBOL_OPERATORS.iter().find(|(s, _)| s == sym).map(|&(_, op)| op),
            Tok::Name(name) => {
                let keyword = name.prefix.is_none().then_some(name.local.as_str())?;
                KEYWORD_OPERATORS.iter().find(|(k, _)| *k == keyword).map(|&(_, op)| op)
            }
            Tok::PrefixWildcard(_)
            | Tok::LocalWildcard(_)
            | Tok::Integer(_)
            | Tok::Decimal(_)
            | Tok::Double(_)
            | Tok::String(_)
            | Tok::Pragma(_)
            | Tok::End => None,
        }
    }

    /// `InstanceofExpr` [XQuery Appendix A.1 \[54\]] and the postfix
    /// operators below it, `TreatExpr` \[55\], `CastableExpr` \[56\] and
    /// `CastExpr` \[57\]: each at most once, the innermost first.
    fn instanceof_expr(&mut self) -> Result<Expr> {
        let operand = self.unary_expr()?;
        self.postfix(operand)
    }

    /// `operand` with the postfix operators after it, the innermost
    /// first [XQuery Appendix A.1 \[54\] to \[57\]].
    fn postfix(&mut self, operand: Expr) -> Result<Expr> {
        let mut expr = operand;
        if self.keywords(CAST, AS) {
            expr = Expr::Cast(Box::new(expr), Box::new(self.single_type()?));
        }
        if self.keywords(CASTABLE, AS) {
            expr = Expr::Castable(Box::new(expr), Box::new(self.single_type()?));
        }
        if self.keywords(TREAT, AS) {
            expr = Expr::Treat(Box::new(expr), Box::new(self.sequence_type()?));
        }
        if self.keywords(INSTANCE, OF) {
            expr = Expr::InstanceOf(Box::new(expr), Box::new(self.sequence_type()?));
        }
        Ok(expr)
    }

    /// Takes the next two tokens if they are the keywords `first` and
    /// `second`.
    pub(super) fn keywords(&mut self, first: &str, second: &str) -> bool {
        let matched = self.is_keyword(0, first) && self.is_keyword(1, second);
        if matched {
            self.ahead.pop_front();
            self.ahead.pop_front();
        }
        matched
    }

    /// `UnaryExpr` [XQuery Appendix A.1 \[58\]]: its signs, then its
    /// `ValueExpr` \[59\].
    fn unary_expr(&mut self) -> Result<Expr> {
        let signs = self.signs();
        let operand = self.value_expr()?;
        Ok(if signs.is_empty() { operand } else { Expr::Unary(signs, Box::new(operand)) })
    }

    /// The signs of a `UnaryExpr` [XQuery Appendix A.1 \[58\]], in the
    /// order written.
    fn signs(&mut self) -> Vec<Sign> {
        let mut signs = Vec::new();
        loop {
            if self.accept_sym(Sym::Minus) {
                signs.push(Sign::Minus);
            } else if self.accept_sym(Sym::Plus) {
                signs.push(Sign::Plus);
            } else {
                return signs;
            }
        }
    }
}

/// `left` and `right` joined by `op`, `left` holding only operators of
/// `op`'s precedence or above: a chain `left` is of `op`'s precedence
/// grows by one. None if `op` is a comparison or `to` and `left` is one
/// already, which the grammar allows only once [XQuery Appendix A.1
/// \[48\], \[49\]].
fn joined(left: Expr, op: Binary, right: Expr) -> Option<Expr> {
    Some(match op {
        Binary::Or => Expr::Or(chain(left, right, |e| if let Expr::Or(v) = e { Ok(v) } else { Err(e) })),
        Binary::And => Expr::And(chain(left, right, |e| if let Expr::And(v) = e { Ok(v) } else { Err(e) })),
        Binary::Union => Expr::Union(chain(left, right, |e| if let Expr::Union(v) = e { Ok(v) } else { Err(e) })),
        Binary::Compare(comparator) => {
            if matches!(left, Expr::Comparison(_)) {
                return None;
            }
            Expr::Comparison(Box::new(Comparison { left, op: comparator, right }))
        }
        Binary::To => {
            if matches!(left, Expr::Range(_)) {
                return None;
            }
            Expr::Range(Box::new([left, right]))
        }
        Binary::Add(add) => {
            let (first, rest) = if let Expr::Additive(first, rest) = left { (first, rest) } else { (Box::new(left), Vec::new()) };
            Expr::Additive(first, appended(rest, (add, right)))
        }
        Binary::Mul(mul) => {
            let (first, rest) = if let Expr::Multiplicative(first, rest) = left { (first, rest) } else { (Box::new(left), Vec::new()) };
            Expr::Multiplicative(first, appended(rest, (mul, right)))
        }
        Binary::Set(set) => {
            let (first, rest) = if let Expr::IntersectExcept(first, rest) = left { (first, rest) } else { (Box::new(left), Vec::new()) };
            Expr::IntersectExcept(first, appended(rest, (set, right)))
        }
    })
}

/// The operands of a chain `left`, as `operands` takes it apart, with
/// `right` after them; `left` and `right` if `left` is no such chain.
fn chain(left: Expr, right: Expr, operands: impl FnOnce(Expr) -> std::result::Result<Vec<Expr>, Expr>) -> Vec<Expr> {
    match operands(left) {
        Ok(chain) => appended(chain, right),
        Err(left) => vec![left, right],
    }
}

/// `items` with `item` after them.
fn appended<T>(mut items: Vec<T>, item: T) -> Vec<T> {
    items.push(item);
    items
}
