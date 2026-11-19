//! `PrimaryExpr` [XQuery Appendix A.1 \[84\] to \[93\]] [XQuery §3.1], and
//! the computed constructors [Appendix A.1 \[109\] to \[116\]] [XQuery
//! §3.7.3]. The direct constructors are read in `parser/direct.rs`.

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{ComputedName, Constructor, Expr, Literal, PiName};
use crate::xquery::keywords::{
    ATTRIBUTE, COMMENT, DOCUMENT, DOCUMENT_NODE, ELEMENT, EMPTY_SEQUENCE, IF, ITEM, NODE, ORDERED, PROCESSING_INSTRUCTION,
    SCHEMA_ATTRIBUTE, SCHEMA_ELEMENT, TEXT, TYPESWITCH, UNORDERED,
};
use crate::xquery::lexer::{Sym, Tok};

/// [XQuery Appendix A.3] The names no function call may have unprefixed,
/// as an expression's syntax takes them first [Appendix A.1.2,
/// reserved-function-names].
const RESERVED_FUNCTION_NAMES: [&str; 13] = [
    ATTRIBUTE,
    COMMENT,
    DOCUMENT_NODE,
    ELEMENT,
    EMPTY_SEQUENCE,
    IF,
    ITEM,
    NODE,
    PROCESSING_INSTRUCTION,
    SCHEMA_ATTRIBUTE,
    SCHEMA_ELEMENT,
    TEXT,
    TYPESWITCH,
];

impl Parser {
    /// `PrimaryExpr` [XQuery Appendix A.1 \[84\]].
    pub(super) fn primary_expr(&mut self) -> Result<Expr> {
        if self.accept_sym(Sym::LParen) {
            return self.parenthesized_expr();
        }
        if self.is_sym(0, Sym::Lt) || self.is_sym(0, Sym::CommentStart) || self.is_sym(0, Sym::PiStart) {
            return self.direct_constructor();
        }
        if self.constructor_follows() {
            return self.computed_constructor();
        }
        if self.is_name(0) && self.is_sym(1, Sym::LParen) {
            return self.function_call();
        }
        self.simple_primary()
    }

    /// A literal, a variable reference or the context item [XQuery
    /// Appendix A.1 \[85\], \[87\], \[90\]], the primary expressions that
    /// hold no other expression.
    fn simple_primary(&mut self) -> Result<Expr> {
        if let Some(literal) = self.literal() {
            return Ok(Expr::Literal(literal));
        }
        if self.is_sym(0, Sym::Dollar) {
            let name = self.var_name("a variable reference is '$' and a QName [XQuery Appendix A.1 [87]]")?;
            return Ok(Expr::Variable(Box::new(name)));
        }
        if self.accept_sym(Sym::Dot) {
            return Ok(Expr::ContextItem);
        }
        Err(self.unexpected("an operand is a literal, a variable, a step, a parenthesized expression, a function call or a constructor [XQuery Appendix A.1 [84]]"))
    }

    /// `Literal` [XQuery Appendix A.1 \[85\]], if one is next.
    fn literal(&mut self) -> Option<Literal> {
        let literal = match self.peek(0)? {
            Tok::Integer(text) => Literal::Integer(text.clone()),
            Tok::Decimal(text) => Literal::Decimal(text.clone()),
            Tok::Double(text) => Literal::Double(text.clone()),
            Tok::String(value) => Literal::String(value.clone()),
            Tok::Name(_) | Tok::PrefixWildcard(_) | Tok::LocalWildcard(_) | Tok::Pragma(_) | Tok::Sym(_) | Tok::End => return None,
        };
        self.ahead.pop_front();
        Some(literal)
    }

    /// `ParenthesizedExpr` [XQuery Appendix A.1 \[89\]], after its `(`.
    fn parenthesized_expr(&mut self) -> Result<Expr> {
        if self.accept_sym(Sym::RParen) {
            return Ok(Expr::Parenthesized(None));
        }
        let inner = self.expr()?;
        self.close_parenthesized(inner)
    }

    /// The parenthesized expression of `inner`, its `)` read.
    fn close_parenthesized(&mut self, inner: Expr) -> Result<Expr> {
        self.expect_sym(Sym::RParen, "a parenthesized expression closes by ')' [XQuery Appendix A.1 [89]]")?;
        Ok(Expr::Parenthesized(Some(Box::new(inner))))
    }

    /// `FunctionCall` [XQuery Appendix A.1 \[93\]], which its name and `(`
    /// begin. Its name, unprefixed, is none of the reserved function
    /// names [Appendix A.1.2, reserved-function-names] [Appendix A.3].
    fn function_call(&mut self) -> Result<Expr> {
        let at = self.here();
        let name = self.expect_name("a function call is named by a QName [XQuery Appendix A.1 [93]]")?;
        if name.prefix.is_none() && RESERVED_FUNCTION_NAMES.contains(&name.local.as_str()) {
            return Err(self.lexer.error(
                at,
                &format!(
                    "'{}' is a reserved function name, so no function call is named by it unprefixed [XQuery Appendix A.1.2, reserved-function-names] [XQuery Appendix A.3]",
                    name.local
                ),
            ));
        }
        self.expect_sym(Sym::LParen, "a function call's arguments are in parentheses [XQuery Appendix A.1 [93]]")?;
        let mut arguments = Vec::new();
        if !self.accept_sym(Sym::RParen) {
            loop {
                arguments.push(self.expr_single()?);
                if !self.accept_sym(Sym::Comma) {
                    break;
                }
            }
            self.expect_sym(Sym::RParen, "a function call's arguments are separated by ',' and closed by ')' [XQuery Appendix A.1 [93]]")?;
        }
        Ok(Expr::FunctionCall(Box::new(name), arguments))
    }

    /// `EnclosedExpr` [XQuery Appendix A.1 \[29\]]: `{`, an `Expr`, `}`;
    /// `rule` the production that has it.
    pub(super) fn enclosed_expr(&mut self, rule: &str) -> Result<Expr> {
        self.expect_sym(Sym::LBrace, rule)?;
        let inner = self.expr()?;
        self.expect_sym(Sym::RBrace, rule)?;
        Ok(inner)
    }

    /// `{`, an `Expr` if one is there, `}`: the content of a computed
    /// constructor that may be empty [XQuery Appendix A.1 \[111\], \[113\],
    /// \[116\]].
    fn optional_enclosed(&mut self, rule: &str) -> Result<Option<Expr>> {
        self.expect_sym(Sym::LBrace, rule)?;
        let inner = if self.is_sym(0, Sym::RBrace) { None } else { Some(self.expr()?) };
        self.expect_sym(Sym::RBrace, rule)?;
        Ok(inner)
    }

    /// A computed constructor [XQuery Appendix A.1 \[109\]], or an
    /// `OrderedExpr` \[91\] or `UnorderedExpr` \[92\], which its keyword
    /// begins.
    fn computed_constructor(&mut self) -> Result<Expr> {
        let keyword = self.expect_name("a constructor begins with its keyword [XQuery Appendix A.1 [109]]")?;
        let constructor = if keyword.is(ORDERED) {
            return Ok(Expr::Ordered(Box::new(self.enclosed_expr("an ordered expression is in braces [XQuery Appendix A.1 [91]]")?)));
        } else if keyword.is(UNORDERED) {
            return Ok(Expr::Unordered(Box::new(self.enclosed_expr("an unordered expression is in braces [XQuery Appendix A.1 [92]]")?)));
        } else if keyword.is(DOCUMENT) {
            Constructor::Document(self.enclosed_expr("a document constructor's content is in braces [XQuery Appendix A.1 [110]]")?)
        } else if keyword.is(TEXT) {
            Constructor::Text(self.enclosed_expr("a text constructor's content is in braces [XQuery Appendix A.1 [114]]")?)
        } else if keyword.is(COMMENT) {
            Constructor::ComputedComment(self.enclosed_expr("a comment constructor's content is in braces [XQuery Appendix A.1 [115]]")?)
        } else if keyword.is(ELEMENT) {
            let rule = "an element constructor is 'element', its name or a name expression, then its content in braces [XQuery Appendix A.1 [111]]";
            let name = self.computed_name(rule)?;
            Constructor::ComputedElement(name, self.optional_enclosed(rule)?)
        } else if keyword.is(ATTRIBUTE) {
            let rule = "an attribute constructor is 'attribute', its name or a name expression, then its value in braces [XQuery Appendix A.1 [113]]";
            let name = self.computed_name(rule)?;
            Constructor::ComputedAttribute(name, self.optional_enclosed(rule)?)
        } else {
            let rule = "a processing instruction constructor is 'processing-instruction', its target or a target expression, then its content in braces [XQuery Appendix A.1 [116]]";
            let target =
                if self.is_sym(0, Sym::LBrace) { PiName::Expr(self.enclosed_expr(rule)?) } else { PiName::Name(self.expect_ncname(rule)?) };
            Constructor::ComputedPi(target, self.optional_enclosed(rule)?)
        };
        Ok(Expr::Constructor(Box::new(constructor)))
    }

    /// The name of a computed element or attribute constructor: a QName,
    /// or an expression in braces [XQuery Appendix A.1 \[111\], \[113\]].
    fn computed_name(&mut self, rule: &str) -> Result<ComputedName> {
        if self.is_sym(0, Sym::LBrace) {
            return Ok(ComputedName::Expr(self.enclosed_expr(rule)?));
        }
        Ok(ComputedName::Name(self.expect_name(rule)?))
    }
}
