//! Expressions compiled [XQuery §3]: primary expressions, function calls,
//! and the operators; paths are compiled in `analyze/path.rs`, the
//! expressions that bind variables in `analyze/flwor.rs` and constructors
//! in `analyze/construct.rs`.

use super::super::ast::{AddOp, Comparison, Expr, Literal, MulOp, Pragma, QName as Lexical, Sign, ValidationMode};
use super::super::error::{ErrorCode, error};
use super::super::plan::{Arith, CallOp, Callee, CompareOp, Op};
use super::Analysis;
use super::scope::{Unprefixed, Var, display};
use super::types::is_name;
use crate::error::Result;
use crate::xml::schema::Mode;
use crate::xml::{Atomic, AtomicType, cast};

impl Analysis<'_> {
    /// `e` compiled [XQuery §3].
    pub(super) fn expr(&mut self, e: &Expr) -> Result<Op> {
        // [DESIGN] Each kind is compiled by a function of its own, so the
        // frame the nesting of expressions recurs through holds none of
        // their locals (RULES.md R-CF-6).
        match e {
            Expr::Sequence(items) => self.all(items).map(Op::Sequence),
            Expr::Flwor(f) => self.flwor(f),
            Expr::Quantified(q) => self.quantified(q),
            Expr::Typeswitch(t) => self.typeswitch(t),
            Expr::If(i) => Ok(Op::If(Box::new([self.expr(&i.test)?, self.expr(&i.then)?, self.expr(&i.otherwise)?]))),
            Expr::Or(operands) => self.all(operands).map(Op::Or),
            Expr::And(operands) => self.all(operands).map(Op::And),
            Expr::Comparison(c) => self.comparison(c),
            Expr::Range(operands) => {
                let [from, to] = &**operands;
                Ok(Op::Range(Box::new([self.expr(from)?, self.expr(to)?])))
            }
            Expr::Additive(first, rest) => self.additive(first, rest),
            Expr::Multiplicative(first, rest) => self.multiplicative(first, rest),
            Expr::Union(operands) => self.all(operands).map(Op::Union),
            Expr::IntersectExcept(first, rest) => self.intersect_except(first, rest),
            Expr::InstanceOf(operand, t) => Ok(Op::InstanceOf(Box::new(self.expr(operand)?), self.sequence_type(t)?)),
            Expr::Treat(operand, t) => Ok(Op::Treat(Box::new(self.expr(operand)?), self.sequence_type(t)?)),
            Expr::Castable(operand, t) => self.cast_expr(operand, t, true),
            Expr::Cast(operand, t) => self.cast_expr(operand, t, false),
            Expr::Unary(signs, operand) => self.unary(signs, operand),
            Expr::Validate(mode, operand) => self.validate(*mode, operand),
            Expr::Extension(pragmas, operand) => self.extension(pragmas, operand.as_deref()),
            Expr::Path(path) => self.path(path),
            Expr::Filter(primary, predicates) => Ok(Op::Filter(Box::new(self.expr(primary)?), self.all(predicates)?)),
            Expr::Literal(literal) => literal_value(literal).map(Op::Atomic),
            Expr::Variable(name) => self.variable_reference(name),
            Expr::Parenthesized(None) => Ok(Op::Empty),
            Expr::Parenthesized(Some(inner)) | Expr::Ordered(inner) | Expr::Unordered(inner) => self.expr(inner),
            Expr::ContextItem => Ok(Op::ContextItem),
            Expr::FunctionCall(name, arguments) => self.call(name, arguments),
            Expr::Constructor(c) => self.constructor_expr(c),
        }
    }

    /// As `expr`, of an `ExprSingle`.
    pub(super) fn expr_single(&mut self, e: &Expr) -> Result<Op> {
        self.expr(e)
    }

    /// Each of `exprs` compiled, in order.
    pub(super) fn all(&mut self, exprs: &[Expr]) -> Result<Vec<Op>> {
        exprs.iter().map(|e| self.expr(e)).collect()
    }

    /// A comparison [XQuery §3.5], its strings compared by the default
    /// collation.
    fn comparison(&mut self, c: &Comparison) -> Result<Op> {
        let (left, right) = (self.expr(&c.left)?, self.expr(&c.right)?);
        Ok(Op::Compare(Box::new(CompareOp { left, op: c.op, right, collation: self.settings.collation.clone() })))
    }

    /// An additive chain [XQuery §3.4].
    fn additive(&mut self, first: &Expr, rest: &[(AddOp, Expr)]) -> Result<Op> {
        let first = Box::new(self.expr(first)?);
        let rest = rest
            .iter()
            .map(|(op, e)| Ok((if *op == AddOp::Plus { Arith::Add } else { Arith::Subtract }, self.expr(e)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Op::Arithmetic(first, rest))
    }

    /// A multiplicative chain [XQuery §3.4].
    fn multiplicative(&mut self, first: &Expr, rest: &[(MulOp, Expr)]) -> Result<Op> {
        let first = Box::new(self.expr(first)?);
        let mut ops = Vec::with_capacity(rest.len());
        for (op, e) in rest {
            let arith = match op {
                MulOp::Times => Arith::Multiply,
                MulOp::Div => Arith::Divide,
                MulOp::Idiv => Arith::IntegerDivide,
                MulOp::Mod => Arith::Modulo,
            };
            ops.push((arith, self.expr(e)?));
        }
        Ok(Op::Arithmetic(first, ops))
    }

    /// `intersect` and `except` [XQuery §3.3.3].
    fn intersect_except(&mut self, first: &Expr, rest: &[(super::super::ast::SetOp, Expr)]) -> Result<Op> {
        let first = Box::new(self.expr(first)?);
        let rest = rest.iter().map(|(op, e)| Ok((*op, self.expr(e)?))).collect::<Result<Vec<_>>>()?;
        Ok(Op::IntersectExcept(first, rest))
    }

    /// A unary expression [XQuery §3.4]: its signs one negation if an odd
    /// number of them are `-`, else a unary plus.
    fn unary(&mut self, signs: &[Sign], operand: &Expr) -> Result<Op> {
        let minus = signs.iter().filter(|s| **s == Sign::Minus).count();
        Ok(Op::Unary(!minus.is_multiple_of(2), Box::new(self.expr(operand)?)))
    }

    /// A validate expression [XQuery §3.13]: of its mode, strict of none.
    fn validate(&mut self, mode: Option<ValidationMode>, operand: &Expr) -> Result<Op> {
        self.validates = true;
        let mode = match mode {
            Some(ValidationMode::Lax) => Mode::Lax,
            Some(ValidationMode::Strict) | None => Mode::Strict,
        };
        Ok(Op::Validate(mode, Box::new(self.expr(operand)?)))
    }

    /// An extension expression [XQuery §3.14]: each pragma's QName
    /// resolved \[err:XPST0081\]; none recognized, so its expression in
    /// braces, which it must have \[err:XQST0079\].
    fn extension(&mut self, pragmas: &[Pragma], operand: Option<&Expr>) -> Result<Op> {
        for pragma in pragmas {
            self.scope.name(&pragma.name, Unprefixed::None)?;
        }
        let Some(operand) = operand else {
            return Err(error(
                ErrorCode::XQST0079,
                "turborel recognizes no pragma, and the extension expression has no expression in braces [XQuery §3.14]",
            ));
        };
        self.expr(operand)
    }

    /// A variable reference [XQuery §3.1.2]: an unprefixed name in no
    /// namespace, of a variable in scope \[err:XPST0008\].
    fn variable_reference(&mut self, name: &Lexical) -> Result<Op> {
        let expanded = self.scope.name(name, Unprefixed::None)?;
        Ok(match self.scope.variable(&expanded)? {
            Var::Local(slot) => Op::Local(slot),
            Var::Global(at) => Op::Global(at),
        })
    }

    /// A function call [XQuery §3.1.5]: an unprefixed name in the default
    /// function namespace; a function the prolog declares, a constructor
    /// function [XQuery §3.12.5], or one of the library, of its name and
    /// arity \[err:XPST0017\].
    fn call(&mut self, name: &Lexical, arguments: &[Expr]) -> Result<Op> {
        let expanded = self.scope.name(name, Unprefixed::Function)?;
        let arity = arguments.len();
        let mut compiled = self.all(arguments)?;
        if let Some(at) = self.declared.iter().position(|(n, a)| n.same(&expanded) && *a == arity) {
            self.scope.functions_called.insert(at);
            return Ok(Op::Call(Box::new(CallOp { callee: Callee::Declared(at), arguments: compiled })));
        }
        if self.constructor(&expanded, arity) {
            // [XQuery §3.12.5] T($arg) is ($arg cast as T?), of a QName
            // ($arg cast as T) [XQueryFO §5.1], of a string literal alone of
            // a type derived from xs:QName or xs:NOTATION.
            let target = self.atomic_target(&expanded, true)?;
            if let Some(literal) = arguments.first().filter(|_| is_name(target.to)) {
                return self.qname_cast(literal, &target, false);
            }
            let argument = compiled.pop().unwrap_or(Op::Empty);
            return Ok(Op::Cast(Box::new(argument), target));
        }
        let Some(found) = self.env.library.find(&expanded, arity) else {
            let why = format!("no function {} of {arity} arguments is in the static context [XQuery §3.1.5]", display(&expanded));
            return Err(error(ErrorCode::XPST0017, &why));
        };
        Ok(Op::Call(Box::new(CallOp { callee: Callee::Library(found), arguments: compiled })))
    }
}

/// The value of `literal` [XQuery §3.1.1]: a numeric literal cast from
/// `xdt:untypedAtomic` to `xs:integer`, `xs:decimal` or `xs:double` by
/// whether it holds "." or an exponent; a string literal an `xs:string`.
fn literal_value(literal: &Literal) -> Result<Atomic> {
    let (text, to) = match literal {
        Literal::Integer(text) => (text, AtomicType::Integer),
        Literal::Decimal(text) => (text, AtomicType::Decimal),
        Literal::Double(text) => (text, AtomicType::Double),
        Literal::String(text) => return Ok(Atomic::new(AtomicType::String, text.as_str())),
    };
    cast(&Atomic::new(AtomicType::UntypedAtomic, text.as_str()), to)
}
