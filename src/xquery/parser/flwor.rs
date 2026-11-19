//! `Expr` and `ExprSingle` [XQuery Appendix A.1 \[31\], \[32\]], and the
//! expressions `ExprSingle` has beside `OrExpr`: FLWOR [XQuery §3.8],
//! quantified [XQuery §3.11], typeswitch [XQuery §3.12.2] and conditional
//! [XQuery §3.10] expressions [Appendix A.1 \[33\] to \[45\]].

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{
    CaseClause, Direction, EmptyOrder, Expr, Flwor, FlworClause, ForBinding, If, LetBinding, OrderBy, OrderSpec, Quantified,
    QuantifiedBinding, Quantifier, SequenceType, Typeswitch,
};
use crate::xquery::keywords::{
    AS, ASCENDING, AT, BY, CASE, COLLATION, DEFAULT, DESCENDING, ELSE, EMPTY, EVERY, FOR, GREATEST, IF, IN, LEAST, LET, ORDER, RETURN,
    SATISFIES, SOME, STABLE, THEN, TYPESWITCH, WHERE,
};
use crate::xquery::lexer::Sym;

impl Parser {
    /// `Expr` [XQuery Appendix A.1 \[31\]]: one `ExprSingle`, or a sequence
    /// of them separated by commas.
    pub(super) fn expr(&mut self) -> Result<Expr> {
        let first = self.expr_single()?;
        if self.is_sym(0, Sym::Comma) { self.sequence(first) } else { Ok(first) }
    }

    /// The sequence of `first` and the `ExprSingle`s after it, each after
    /// a comma [XQuery Appendix A.1 \[31\]].
    fn sequence(&mut self, first: Expr) -> Result<Expr> {
        let mut items = vec![first];
        while self.accept_sym(Sym::Comma) {
            items.push(self.expr_single()?);
        }
        Ok(Expr::Sequence(items))
    }

    /// `ExprSingle` [XQuery Appendix A.1 \[32\]], one level deeper
    /// (`MAX_NESTING`). Its keywords begin its expressions only before
    /// what the grammar has after them, as none is reserved [Appendix
    /// A.2].
    pub(super) fn expr_single(&mut self) -> Result<Expr> {
        self.nested(|p| {
            let variable = p.is_sym(1, Sym::Dollar);
            if variable && (p.is_keyword(0, FOR) || p.is_keyword(0, LET)) {
                return p.flwor_expr();
            }
            if variable && (p.is_keyword(0, SOME) || p.is_keyword(0, EVERY)) {
                return p.quantified_expr();
            }
            let parenthesized = p.is_sym(1, Sym::LParen);
            if parenthesized && p.is_keyword(0, TYPESWITCH) {
                return p.typeswitch_expr();
            }
            if parenthesized && p.is_keyword(0, IF) {
                return p.if_expr();
            }
            p.or_expr()
        })
    }

    /// `TypeDeclaration` [XQuery Appendix A.1 \[118\]], if `as` is next.
    pub(super) fn type_declaration(&mut self) -> Result<Option<SequenceType>> {
        if self.accept_keyword(AS) { self.sequence_type().map(Some) } else { Ok(None) }
    }

    /// `FLWORExpr` [XQuery Appendix A.1 \[33\]], which `for` or `let` and
    /// `$` begin.
    fn flwor_expr(&mut self) -> Result<Expr> {
        let mut clauses = Vec::new();
        while self.is_sym(1, Sym::Dollar) {
            if self.accept_keyword(FOR) {
                clauses.push(FlworClause::For(self.for_bindings()?));
            } else if self.accept_keyword(LET) {
                clauses.push(FlworClause::Let(self.let_bindings()?));
            } else {
                break;
            }
        }
        let condition = if self.accept_keyword(WHERE) { Some(self.expr_single()?) } else { None };
        let order_by = self.order_by_clause()?;
        self.expect_keyword(RETURN, "a FLWOR expression ends with 'return' and an expression [XQuery Appendix A.1 [33]]")?;
        let result = self.expr_single()?;
        Ok(Expr::Flwor(Box::new(Flwor { clauses, condition, order_by, result })))
    }

    /// The bindings of a `ForClause` [XQuery Appendix A.1 \[34\]], after
    /// its `for`.
    fn for_bindings(&mut self) -> Result<Vec<ForBinding>> {
        let rule = "a for clause binds '$' and a name, a type if declared, a position if 'at', then 'in' and an expression [XQuery Appendix A.1 [34]]";
        let mut bindings = Vec::new();
        loop {
            let var = self.var_name(rule)?;
            let declared = self.type_declaration()?;
            let position = if self.accept_keyword(AT) { Some(self.var_name(rule)?) } else { None };
            self.expect_keyword(IN, rule)?;
            let input = self.expr_single()?;
            bindings.push(ForBinding { var, declared, position, input });
            if !self.accept_sym(Sym::Comma) {
                return Ok(bindings);
            }
        }
    }

    /// The bindings of a `LetClause` [XQuery Appendix A.1 \[36\]], after
    /// its `let`.
    fn let_bindings(&mut self) -> Result<Vec<LetBinding>> {
        let rule = "a let clause binds '$' and a name, a type if declared, then ':=' and an expression [XQuery Appendix A.1 [36]]";
        let mut bindings = Vec::new();
        loop {
            let var = self.var_name(rule)?;
            let declared = self.type_declaration()?;
            self.expect_sym(Sym::Assign, rule)?;
            let value = self.expr_single()?;
            bindings.push(LetBinding { var, declared, value });
            if !self.accept_sym(Sym::Comma) {
                return Ok(bindings);
            }
        }
    }

    /// `OrderByClause` [XQuery Appendix A.1 \[38\]], if one is next.
    fn order_by_clause(&mut self) -> Result<Option<OrderBy>> {
        let rule = "an order by clause is 'order by' or 'stable order by', then its order specifications [XQuery Appendix A.1 [38]]";
        let stable = self.accept_keyword(STABLE);
        if !stable && !self.is_keyword(0, ORDER) {
            return Ok(None);
        }
        self.expect_keyword(ORDER, rule)?;
        self.expect_keyword(BY, rule)?;
        let mut specs = Vec::new();
        loop {
            specs.push(self.order_spec()?);
            if !self.accept_sym(Sym::Comma) {
                return Ok(Some(OrderBy { stable, specs }));
            }
        }
    }

    /// `OrderSpec` [XQuery Appendix A.1 \[40\]] and its `OrderModifier`
    /// \[41\].
    fn order_spec(&mut self) -> Result<OrderSpec> {
        let key = self.expr_single()?;
        let direction = if self.accept_keyword(ASCENDING) {
            Some(Direction::Ascending)
        } else if self.accept_keyword(DESCENDING) {
            Some(Direction::Descending)
        } else {
            None
        };
        let empty = if self.accept_keyword(EMPTY) { Some(self.empty_order()?) } else { None };
        let collation = if self.accept_keyword(COLLATION) {
            Some(self.expect_string("'collation' is followed by a URI literal [XQuery Appendix A.1 [41]]")?)
        } else {
            None
        };
        Ok(OrderSpec { key, direction, empty, collation })
    }

    /// `greatest` or `least`, after `empty` [XQuery Appendix A.1 \[15\],
    /// \[41\]].
    pub(super) fn empty_order(&mut self) -> Result<EmptyOrder> {
        if self.accept_keyword(GREATEST) {
            Ok(EmptyOrder::Greatest)
        } else if self.accept_keyword(LEAST) {
            Ok(EmptyOrder::Least)
        } else {
            Err(self.unexpected("'empty' is followed by 'greatest' or 'least' [XQuery Appendix A.1 [15], [41]]"))
        }
    }

    /// `QuantifiedExpr` [XQuery Appendix A.1 \[42\]], which `some` or
    /// `every` and `$` begin.
    fn quantified_expr(&mut self) -> Result<Expr> {
        let rule = "a quantified expression binds '$' and a name, a type if declared, then 'in' and an expression, and ends with 'satisfies' [XQuery Appendix A.1 [42]]";
        let quantifier = if self.is_keyword(0, SOME) { Quantifier::Some } else { Quantifier::Every };
        self.ahead.pop_front();
        let mut bindings = Vec::new();
        loop {
            let var = self.var_name(rule)?;
            let declared = self.type_declaration()?;
            self.expect_keyword(IN, rule)?;
            let input = self.expr_single()?;
            bindings.push(QuantifiedBinding { var, declared, input });
            if !self.accept_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_keyword(SATISFIES, rule)?;
        let test = self.expr_single()?;
        Ok(Expr::Quantified(Box::new(Quantified { quantifier, bindings, test })))
    }

    /// `TypeswitchExpr` [XQuery Appendix A.1 \[43\]], which `typeswitch` and
    /// `(` begin.
    fn typeswitch_expr(&mut self) -> Result<Expr> {
        let rule = "a typeswitch is 'typeswitch', its operand in parentheses, case clauses, then 'default' [XQuery Appendix A.1 [43]]";
        self.ahead.pop_front();
        self.expect_sym(Sym::LParen, rule)?;
        let operand = self.expr()?;
        self.expect_sym(Sym::RParen, rule)?;
        let mut cases = Vec::new();
        while self.accept_keyword(CASE) {
            cases.push(self.case_clause()?);
        }
        if cases.is_empty() {
            return Err(self.unexpected("a typeswitch has one or more case clauses [XQuery Appendix A.1 [43]]"));
        }
        self.expect_keyword(DEFAULT, rule)?;
        let default_var = if self.is_sym(0, Sym::Dollar) { Some(self.var_name(rule)?) } else { None };
        self.expect_keyword(RETURN, rule)?;
        let default = self.expr_single()?;
        Ok(Expr::Typeswitch(Box::new(Typeswitch { operand, cases, default_var, default })))
    }

    /// `CaseClause` [XQuery Appendix A.1 \[44\]], after its `case`.
    fn case_clause(&mut self) -> Result<CaseClause> {
        let rule = "a case clause is 'case', a variable and 'as' if bound, a sequence type, then 'return' and an expression [XQuery Appendix A.1 [44]]";
        let var = if self.is_sym(0, Sym::Dollar) {
            let var = self.var_name(rule)?;
            self.expect_keyword(AS, rule)?;
            Some(var)
        } else {
            None
        };
        let matches = self.sequence_type()?;
        self.expect_keyword(RETURN, rule)?;
        let result = self.expr_single()?;
        Ok(CaseClause { var, matches, result })
    }

    /// `IfExpr` [XQuery Appendix A.1 \[45\]], which `if` and `(` begin.
    fn if_expr(&mut self) -> Result<Expr> {
        let rule = "a conditional expression is 'if', its test in parentheses, 'then' and an expression, 'else' and an expression [XQuery Appendix A.1 [45]]";
        self.ahead.pop_front();
        self.expect_sym(Sym::LParen, rule)?;
        let test = self.expr()?;
        self.expect_sym(Sym::RParen, rule)?;
        self.expect_keyword(THEN, rule)?;
        let then = self.expr_single()?;
        self.expect_keyword(ELSE, rule)?;
        let otherwise = self.expr_single()?;
        Ok(Expr::If(Box::new(If { test, then, otherwise })))
    }
}
