//! `ValueExpr` [XQuery Appendix A.1 \[59\]]: validate and extension
//! expressions [XQuery §3.13, §3.14], and path expressions [XQuery §3.2]
//! with their steps, axes, node tests and predicates [Appendix A.1 \[68\]
//! to \[83\]].

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{
    AxisStep, Expr, ForwardAxis, NameTest, NodeTest, Path, ReverseAxis, Root, Separator, Step, StepExpr, ValidationMode,
};
use crate::xquery::keywords::{
    ANCESTOR, ANCESTOR_OR_SELF, ATTRIBUTE, CHILD, COMMENT, DESCENDANT, DESCENDANT_OR_SELF, DOCUMENT, ELEMENT, FOLLOWING, FOLLOWING_SIBLING,
    LAX, ORDERED, PARENT, PRECEDING, PRECEDING_SIBLING, PROCESSING_INSTRUCTION, SELF_AXIS, STRICT, TEXT, UNORDERED, VALIDATE,
};
use crate::xquery::lexer::{Sym, Tok};

/// [XQuery Appendix A.1 \[73\]] The forward axes, by name.
const FORWARD_AXES: [(&str, ForwardAxis); 7] = [
    (CHILD, ForwardAxis::Child),
    (DESCENDANT, ForwardAxis::Descendant),
    (ATTRIBUTE, ForwardAxis::Attribute),
    (SELF_AXIS, ForwardAxis::SelfAxis),
    (DESCENDANT_OR_SELF, ForwardAxis::DescendantOrSelf),
    (FOLLOWING_SIBLING, ForwardAxis::FollowingSibling),
    (FOLLOWING, ForwardAxis::Following),
];

/// [XQuery Appendix A.1 \[76\]] The reverse axes, by name.
const REVERSE_AXES: [(&str, ReverseAxis); 5] = [
    (PARENT, ReverseAxis::Parent),
    (ANCESTOR, ReverseAxis::Ancestor),
    (PRECEDING_SIBLING, ReverseAxis::PrecedingSibling),
    (PRECEDING, ReverseAxis::Preceding),
    (ANCESTOR_OR_SELF, ReverseAxis::AncestorOrSelf),
];

/// [XQuery Appendix A.1 \[110\], \[114\], \[115\], \[91\], \[92\]] The
/// keywords a `{` after makes a constructor or an ordered or unordered
/// expression.
const BRACED_KEYWORDS: [&str; 5] = [DOCUMENT, TEXT, COMMENT, ORDERED, UNORDERED];

/// [XQuery Appendix A.1 \[70\] to \[90\], \[95\]] The symbols that begin a
/// `StepExpr`, beside `<`.
const STEP_SYMBOLS: [Sym; 8] = [Sym::Star, Sym::At, Sym::Dot, Sym::DotDot, Sym::Dollar, Sym::LParen, Sym::CommentStart, Sym::PiStart];

/// Whether `tok` begins a `StepExpr` [XQuery Appendix A.1 \[70\]]: a name,
/// a wildcard, a literal, or one of `STEP_SYMBOLS`.
fn begins_step(tok: &Tok) -> bool {
    match tok {
        Tok::Name(_)
        | Tok::PrefixWildcard(_)
        | Tok::LocalWildcard(_)
        | Tok::Integer(_)
        | Tok::Decimal(_)
        | Tok::Double(_)
        | Tok::String(_) => true,
        Tok::Sym(sym) => STEP_SYMBOLS.contains(sym),
        Tok::Pragma(_) | Tok::End => false,
    }
}

/// The path `/` alone, the root of the context node's tree [XQuery §3.2].
fn lone_slash() -> Expr {
    Expr::Path(Box::new(Path { root: Root::Slash, first: None, rest: Vec::new() }))
}

impl Parser {
    /// `ValueExpr` [XQuery Appendix A.1 \[59\]].
    pub(super) fn value_expr(&mut self) -> Result<Expr> {
        if self.validate_follows() {
            return self.validate_expr();
        }
        if matches!(self.peek(0), Some(Tok::Pragma(_))) {
            return self.extension_expr();
        }
        self.path_expr()
    }

    /// Whether a `ValidateExpr` is next: `validate`, then `{` or a
    /// validation mode and `{` [XQuery Appendix A.1 \[63\]].
    fn validate_follows(&mut self) -> bool {
        let mode = self.is_keyword(1, LAX) || self.is_keyword(1, STRICT);
        self.is_keyword(0, VALIDATE) && (self.is_sym(1, Sym::LBrace) || (mode && self.is_sym(2, Sym::LBrace)))
    }

    /// `ValidateExpr` [XQuery Appendix A.1 \[63\]], which is next.
    fn validate_expr(&mut self) -> Result<Expr> {
        self.ahead.pop_front();
        let mode = if self.accept_keyword(LAX) {
            Some(ValidationMode::Lax)
        } else if self.accept_keyword(STRICT) {
            Some(ValidationMode::Strict)
        } else {
            None
        };
        let operand = self.enclosed_expr("a validate expression's operand is in braces [XQuery Appendix A.1 [63]]")?;
        Ok(Expr::Validate(mode, Box::new(operand)))
    }

    /// `ExtensionExpr` [XQuery Appendix A.1 \[65\]], which a pragma begins.
    fn extension_expr(&mut self) -> Result<Expr> {
        let mut pragmas = Vec::new();
        while let Some(Tok::Pragma(pragma)) = self.peek(0) {
            pragmas.push(pragma.clone());
            self.ahead.pop_front();
        }
        self.expect_sym(Sym::LBrace, "an extension expression's pragmas are followed by '{' [XQuery Appendix A.1 [65]]")?;
        let operand = if self.is_sym(0, Sym::RBrace) { None } else { Some(Box::new(self.expr()?)) };
        self.expect_sym(Sym::RBrace, "an extension expression's braces close by '}' [XQuery Appendix A.1 [65]]")?;
        Ok(Expr::Extension(pragmas, operand))
    }

    /// `PathExpr` [XQuery Appendix A.1 \[68\]]. A path of one step that is
    /// a `FilterExpr` is that expression; a `FilterExpr` of no predicate
    /// is its primary expression.
    fn path_expr(&mut self) -> Result<Expr> {
        let Some(root) = self.path_root()? else {
            return Ok(lone_slash());
        };
        let first = self.step_expr()?;
        self.path_rest(root, first)
    }

    /// What a path starts with, read: none for a `/` alone [XQuery
    /// Appendix A.1 \[68\]].
    fn path_root(&mut self) -> Result<Option<Root>> {
        if self.accept_sym(Sym::Slash) {
            return Ok(self.step_follows().then_some(Root::Slash));
        }
        if self.accept_sym(Sym::SlashSlash) {
            if !self.step_follows() {
                return Err(self.unexpected("'//' begins a path and is no path alone [XQuery §3.2]"));
            }
            return Ok(Some(Root::DoubleSlash));
        }
        Ok(Some(Root::Relative))
    }

    /// The path from `root` whose first step is `first`, its later steps
    /// read. Apart from `path_expr`, so the frame of each level a query
    /// nests holds only what its recursion needs (RULES.md R-CF-6).
    fn path_rest(&mut self, root: Root, first: StepExpr) -> Result<Expr> {
        let rest = self.later_steps()?;
        Ok(match (root, first, rest.is_empty()) {
            (Root::Relative, StepExpr::Filter(primary, predicates), true) => {
                if predicates.is_empty() {
                    primary
                } else {
                    Expr::Filter(Box::new(primary), predicates)
                }
            }
            (root, first, _) => Expr::Path(Box::new(Path { root, first: Some(first), rest })),
        })
    }

    /// The steps after a path's first, each after `/` or `//` [XQuery
    /// Appendix A.1 \[69\]].
    fn later_steps(&mut self) -> Result<Vec<(Separator, StepExpr)>> {
        let mut steps = Vec::new();
        loop {
            let separator = if self.accept_sym(Sym::Slash) {
                Separator::Slash
            } else if self.accept_sym(Sym::SlashSlash) {
                Separator::DoubleSlash
            } else {
                return Ok(steps);
            };
            steps.push((separator, self.step_expr()?));
        }
    }

    /// Whether a step follows a leading `/` [XQuery Appendix A.1.2,
    /// leading-lone-slash]: a token that can begin a `StepExpr`, so a
    /// `*` or a keyword after the slash is a name test, never an
    /// operator. \[DESIGN\] A `<` begins one only if a name is right
    /// after it, the start of a direct element constructor: else it is
    /// the comparison, as `/ < 3` intends.
    fn step_follows(&mut self) -> bool {
        if self.is_sym(0, Sym::Lt) {
            let after = self.peek_token(0).map_or(0, |t| t.end);
            return self.lexer.char_at_index(after).is_some_and(|c| self.lexer.is_name_start(c));
        }
        self.peek(0).is_some_and(begins_step)
    }

    /// `StepExpr` [XQuery Appendix A.1 \[70\]]: an `AxisStep` \[71\] or a
    /// `FilterExpr` \[81\], each with its predicates.
    fn step_expr(&mut self) -> Result<StepExpr> {
        if self.axis_step_follows() {
            return self.axis_step();
        }
        let primary = self.primary_expr()?;
        self.filter_predicates(primary)
    }

    /// The `FilterExpr` of `primary` and the predicates after it.
    fn filter_predicates(&mut self, primary: Expr) -> Result<StepExpr> {
        let predicates = self.predicates()?;
        Ok(StepExpr::Filter(primary, predicates))
    }

    /// Whether an `AxisStep` [XQuery Appendix A.1 \[71\]] is next, rather
    /// than a primary expression: `@`, `..`, an axis and `::`, a wildcard,
    /// a kind test, or a name that begins no function call or computed
    /// constructor.
    fn axis_step_follows(&mut self) -> bool {
        let symbol = self.is_sym(0, Sym::At) || self.is_sym(0, Sym::DotDot) || self.is_sym(1, Sym::Axis);
        let name_test = matches!(self.peek(0), Some(Tok::Sym(Sym::Star) | Tok::PrefixWildcard(_) | Tok::LocalWildcard(_)));
        let name = self.is_name(0) && !self.is_sym(1, Sym::LParen) && !self.constructor_follows();
        symbol || name_test || name || self.kind_test_follows()
    }

    /// `AxisStep` [XQuery Appendix A.1 \[71\]], which is next: its step
    /// \[72\] to \[77\], then its predicates.
    fn axis_step(&mut self) -> Result<StepExpr> {
        let mut axis_step = self.step()?;
        axis_step.predicates = self.predicates()?;
        Ok(StepExpr::Axis(axis_step))
    }

    /// The step of an `AxisStep` [XQuery Appendix A.1 \[72\] to \[77\]],
    /// which is next, its predicates not yet read: boxed, as the frame
    /// that reads its predicates holds it (RULES.md R-CF-6).
    fn step(&mut self) -> Result<Box<AxisStep>> {
        let step = if self.accept_sym(Sym::At) {
            Step::Attribute(self.node_test()?)
        } else if self.accept_sym(Sym::DotDot) {
            Step::Parent
        } else if self.is_sym(1, Sym::Axis) {
            self.axis()?
        } else {
            Step::Abbreviated(self.node_test()?)
        };
        Ok(Box::new(AxisStep { step, predicates: Vec::new() }))
    }

    /// An axis, its `::`, and its node test [XQuery Appendix A.1 \[73\],
    /// \[76\]], which are next.
    fn axis(&mut self) -> Result<Step> {
        let at = self.here();
        let name = self.expect_name("an axis is named [XQuery Appendix A.1 [73], [76]]")?;
        let forward = FORWARD_AXES.iter().find(|(text, _)| name.is(text)).map(|&(_, axis)| axis);
        let reverse = REVERSE_AXES.iter().find(|(text, _)| name.is(text)).map(|&(_, axis)| axis);
        self.expect_sym(Sym::Axis, "an axis is followed by '::' [XQuery Appendix A.1 [73], [76]]")?;
        match (forward, reverse) {
            (Some(axis), _) => Ok(Step::Forward(axis, self.node_test()?)),
            (None, Some(axis)) => Ok(Step::Reverse(axis, self.node_test()?)),
            (None, None) => Err(self.lexer.error(at, "'::' follows the name of an axis [XQuery Appendix A.1 [73], [76]]")),
        }
    }

    /// `NodeTest` [XQuery Appendix A.1 \[78\]]: a kind test, or a
    /// `NameTest` \[79\] with its wildcards \[80\].
    fn node_test(&mut self) -> Result<NodeTest> {
        if self.kind_test_follows() {
            return Ok(NodeTest::Kind(self.kind_test()?));
        }
        let test = match self.peek(0) {
            Some(Tok::Sym(Sym::Star)) => NameTest::Any,
            Some(Tok::PrefixWildcard(prefix)) => NameTest::Prefix(prefix.clone()),
            Some(Tok::LocalWildcard(local)) => NameTest::Local(local.clone()),
            Some(Tok::Name(name)) => NameTest::Name(name.clone()),
            Some(Tok::Sym(_) | Tok::Integer(_) | Tok::Decimal(_) | Tok::Double(_) | Tok::String(_) | Tok::Pragma(_) | Tok::End) | None => {
                return Err(self.unexpected("a step tests nodes by a name, a wildcard or a kind test [XQuery Appendix A.1 [78]]"));
            }
        };
        self.ahead.pop_front();
        Ok(NodeTest::Name(test))
    }

    /// Whether a computed constructor, or an ordered or unordered
    /// expression, is next [XQuery Appendix A.1 \[91\], \[92\], \[109\]]:
    /// its keyword, its name if it has one, then `{`.
    pub(super) fn constructor_follows(&mut self) -> bool {
        if BRACED_KEYWORDS.iter().any(|keyword| self.is_keyword(0, keyword)) {
            return self.is_sym(1, Sym::LBrace);
        }
        let named = self.is_keyword(0, ELEMENT) || self.is_keyword(0, ATTRIBUTE);
        let target = self.is_keyword(0, PROCESSING_INSTRUCTION);
        let braced_name = (self.is_name(1) && named) || (self.is_ncname(1) && target);
        (named || target) && (self.is_sym(1, Sym::LBrace) || (braced_name && self.is_sym(2, Sym::LBrace)))
    }

    /// `PredicateList` [XQuery Appendix A.1 \[82\]]: each `Predicate` \[83\]'s
    /// expression.
    fn predicates(&mut self) -> Result<Vec<Expr>> {
        let mut predicates = Vec::new();
        while self.accept_sym(Sym::LBracket) {
            predicates.push(self.expr()?);
            self.close_predicate()?;
        }
        Ok(predicates)
    }

    /// Reads the `]` that closes a predicate [XQuery Appendix A.1 \[83\]].
    fn close_predicate(&mut self) -> Result<()> {
        self.expect_sym(Sym::RBracket, "a predicate closes by ']' [XQuery Appendix A.1 [83]]").map(drop)
    }
}
