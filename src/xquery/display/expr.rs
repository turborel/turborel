//! Expressions written back as text [XQuery Appendix A.1 \[31\] to
//! \[93\]]. A tree holds every parenthesized expression as written, so no
//! operand needs parentheses its tree does not have.
//!
//! \[DESIGN\] RULES.md R-CF-6: an expression writes the expressions in it
//! by calling their `fmt` itself, and its constant text by `write_str`,
//! so the frames of each level the tree nests hold no formatting
//! arguments, and the deepest tree the parser makes is written within
//! the stack.

use std::fmt::{self, Display, Formatter};

use super::{empty_order, separated, spaced, string_literal, type_declaration};
use crate::xquery::ast::{
    AddOp, AxisStep, Comparator, Comparison, Direction, Expr, Flwor, FlworClause, ForBinding, ForwardAxis, GeneralComp, If, LetBinding,
    Literal, MulOp, NodeComp, OrderBy, Path, Pragma, QName, Quantified, Quantifier, ReverseAxis, Root, Separator, SetOp, Sign, Step,
    StepExpr, Typeswitch, ValidationMode, ValueComp,
};
use crate::xquery::keywords::{
    ANCESTOR, ANCESTOR_OR_SELF, AND, AS, ASCENDING, AT, ATTRIBUTE, BY, CASE, CAST, CASTABLE, CHILD, COLLATION, DEFAULT, DESCENDANT,
    DESCENDANT_OR_SELF, DESCENDING, DIV, ELSE, EMPTY, EQ, EVERY, EXCEPT, FOLLOWING, FOLLOWING_SIBLING, FOR, GE, GT, IDIV, IF, IN, INSTANCE,
    INTERSECT, IS, LAX, LE, LET, LT, MOD, NE, OF, OR, ORDER, ORDERED, PARENT, PRECEDING, PRECEDING_SIBLING, RETURN, SATISFIES, SELF_AXIS,
    SOME, STABLE, STRICT, THEN, TO, TREAT, TYPESWITCH, UNION, UNORDERED, VALIDATE, WHERE,
};

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Sequence(items) => separated(f, items, ", "),
            Expr::Flwor(flwor) => flwor.fmt(f),
            Expr::Quantified(quantified) => quantified.fmt(f),
            Expr::Typeswitch(typeswitch) => typeswitch.fmt(f),
            Expr::If(expr) => if_expr(f, expr),
            Expr::Or(operands) => keyword_separated(f, operands, OR),
            Expr::And(operands) => keyword_separated(f, operands, AND),
            Expr::Comparison(comparison) => comparison_expr(f, comparison),
            Expr::Range(operands) => range(f, operands),
            Expr::Additive(first, rest) => chain(f, first, rest, |op| if *op == AddOp::Plus { "+" } else { "-" }),
            Expr::Multiplicative(first, rest) => chain(f, first, rest, |op| multiplicative(*op)),
            Expr::Union(operands) => keyword_separated(f, operands, UNION),
            Expr::IntersectExcept(first, rest) => chain(f, first, rest, |op| if *op == SetOp::Intersect { INTERSECT } else { EXCEPT }),
            Expr::InstanceOf(operand, t) => postfix(f, operand, [INSTANCE, OF], &**t),
            Expr::Treat(operand, t) => postfix(f, operand, [TREAT, AS], &**t),
            Expr::Castable(operand, t) => postfix(f, operand, [CASTABLE, AS], &**t),
            Expr::Cast(operand, t) => postfix(f, operand, [CAST, AS], &**t),
            Expr::Unary(signs, operand) => unary(f, signs, operand),
            Expr::Validate(mode, operand) => validate(f, *mode, operand),
            Expr::Extension(pragmas, operand) => extension(f, pragmas, operand.as_deref()),
            Expr::Path(path) => path.fmt(f),
            Expr::Filter(primary, predicates) => filter(f, primary, predicates),
            Expr::Literal(literal) => literal.fmt(f),
            Expr::Variable(name) => variable(f, name),
            Expr::Parenthesized(inner) => parenthesized(f, inner.as_deref()),
            Expr::ContextItem => f.write_str("."),
            Expr::FunctionCall(name, arguments) => function_call(f, name, arguments),
            Expr::Ordered(inner) => keyword_braced(f, ORDERED, inner),
            Expr::Unordered(inner) => keyword_braced(f, UNORDERED, inner),
            Expr::Constructor(constructor) => constructor.fmt(f),
        }
    }
}

/// Writes `operands` with `keyword` between each two, spaced.
fn keyword_separated(f: &mut Formatter<'_>, operands: &[Expr], keyword: &str) -> fmt::Result {
    for (index, operand) in operands.iter().enumerate() {
        if index > 0 {
            spaced(f, keyword)?;
        }
        operand.fmt(f)?;
    }
    Ok(())
}

/// Writes `first`, then each operator `op` writes and the operand after
/// it, the operator spaced.
fn chain<T>(f: &mut Formatter<'_>, first: &Expr, rest: &[(T, Expr)], op: impl Fn(&T) -> &'static str) -> fmt::Result {
    first.fmt(f)?;
    for (operator, operand) in rest {
        spaced(f, op(operator))?;
        operand.fmt(f)?;
    }
    Ok(())
}

/// Writes an `IfExpr` [XQuery Appendix A.1 \[45\]].
fn if_expr(f: &mut Formatter<'_>, expr: &If) -> fmt::Result {
    f.write_str(IF)?;
    f.write_str(" (")?;
    expr.test.fmt(f)?;
    f.write_str(")")?;
    spaced(f, THEN)?;
    expr.then.fmt(f)?;
    spaced(f, ELSE)?;
    expr.otherwise.fmt(f)
}

/// Writes a `ComparisonExpr` [XQuery Appendix A.1 \[48\]].
fn comparison_expr(f: &mut Formatter<'_>, comparison: &Comparison) -> fmt::Result {
    comparison.left.fmt(f)?;
    spaced(f, comparator(comparison.op))?;
    comparison.right.fmt(f)
}

/// Writes a `RangeExpr` [XQuery Appendix A.1 \[49\]].
fn range(f: &mut Formatter<'_>, operands: &[Expr; 2]) -> fmt::Result {
    let [from, to] = operands;
    from.fmt(f)?;
    spaced(f, TO)?;
    to.fmt(f)
}

/// Writes `operand`, the two keywords of a postfix operator spaced, and
/// its type [XQuery Appendix A.1 \[54\] to \[57\]].
fn postfix(f: &mut Formatter<'_>, operand: &Expr, keywords: [&str; 2], t: &impl Display) -> fmt::Result {
    let [first, second] = keywords;
    operand.fmt(f)?;
    spaced(f, first)?;
    f.write_str(second)?;
    f.write_str(" ")?;
    t.fmt(f)
}

/// Writes a `UnaryExpr` [XQuery Appendix A.1 \[58\]].
fn unary(f: &mut Formatter<'_>, signs: &[Sign], operand: &Expr) -> fmt::Result {
    for sign in signs {
        f.write_str(if *sign == Sign::Minus { "-" } else { "+" })?;
    }
    operand.fmt(f)
}

/// Writes a `VarRef` [XQuery Appendix A.1 \[87\]].
fn variable(f: &mut Formatter<'_>, name: &QName) -> fmt::Result {
    f.write_str("$")?;
    name.fmt(f)
}

/// Writes a `ParenthesizedExpr` [XQuery Appendix A.1 \[89\]].
fn parenthesized(f: &mut Formatter<'_>, inner: Option<&Expr>) -> fmt::Result {
    f.write_str("(")?;
    if let Some(inner) = inner {
        inner.fmt(f)?;
    }
    f.write_str(")")
}

/// Writes a `FunctionCall` [XQuery Appendix A.1 \[93\]].
fn function_call(f: &mut Formatter<'_>, name: &QName, arguments: &[Expr]) -> fmt::Result {
    name.fmt(f)?;
    f.write_str("(")?;
    separated(f, arguments, ", ")?;
    f.write_str(")")
}

/// Writes `keyword { inner }` [XQuery Appendix A.1 \[91\], \[92\]].
pub(super) fn keyword_braced(f: &mut Formatter<'_>, keyword: &str, inner: &Expr) -> fmt::Result {
    f.write_str(keyword)?;
    braced(f, inner)
}

/// Writes ` { inner }`.
pub(super) fn braced(f: &mut Formatter<'_>, inner: &Expr) -> fmt::Result {
    f.write_str(" { ")?;
    inner.fmt(f)?;
    f.write_str(" }")
}

/// How `op` is written [XQuery Appendix A.1 \[51\]].
fn multiplicative(op: MulOp) -> &'static str {
    match op {
        MulOp::Times => "*",
        MulOp::Div => DIV,
        MulOp::Idiv => IDIV,
        MulOp::Mod => MOD,
    }
}

/// How `op` is written [XQuery Appendix A.1 \[60\] to \[62\]].
fn comparator(op: Comparator) -> &'static str {
    match op {
        Comparator::General(GeneralComp::Eq) => "=",
        Comparator::General(GeneralComp::Ne) => "!=",
        Comparator::General(GeneralComp::Lt) => "<",
        Comparator::General(GeneralComp::Le) => "<=",
        Comparator::General(GeneralComp::Gt) => ">",
        Comparator::General(GeneralComp::Ge) => ">=",
        Comparator::Value(ValueComp::Eq) => EQ,
        Comparator::Value(ValueComp::Ne) => NE,
        Comparator::Value(ValueComp::Lt) => LT,
        Comparator::Value(ValueComp::Le) => LE,
        Comparator::Value(ValueComp::Gt) => GT,
        Comparator::Value(ValueComp::Ge) => GE,
        Comparator::Node(NodeComp::Is) => IS,
        Comparator::Node(NodeComp::Precedes) => "<<",
        Comparator::Node(NodeComp::Follows) => ">>",
    }
}

/// Writes a `ValidateExpr` [XQuery Appendix A.1 \[63\]].
fn validate(f: &mut Formatter<'_>, mode: Option<ValidationMode>, operand: &Expr) -> fmt::Result {
    f.write_str(VALIDATE)?;
    match mode {
        Some(ValidationMode::Lax) => write!(f, " {LAX}")?,
        Some(ValidationMode::Strict) => write!(f, " {STRICT}")?,
        None => {}
    }
    braced(f, operand)
}

/// Writes an `ExtensionExpr` [XQuery Appendix A.1 \[65\]].
fn extension(f: &mut Formatter<'_>, pragmas: &[Pragma], operand: Option<&Expr>) -> fmt::Result {
    for pragma in pragmas {
        write!(f, "(#{}{}#) ", pragma.name, pragma.contents)?;
    }
    match operand {
        Some(operand) => {
            f.write_str("{ ")?;
            operand.fmt(f)?;
            f.write_str(" }")
        }
        None => f.write_str("{ }"),
    }
}

/// Writes a `FilterExpr` [XQuery Appendix A.1 \[81\]].
fn filter(f: &mut Formatter<'_>, primary: &Expr, predicates: &[Expr]) -> fmt::Result {
    primary.fmt(f)?;
    predicate_list(f, predicates)
}

/// Writes a `PredicateList` [XQuery Appendix A.1 \[82\]].
fn predicate_list(f: &mut Formatter<'_>, predicates: &[Expr]) -> fmt::Result {
    for predicate in predicates {
        f.write_str("[")?;
        predicate.fmt(f)?;
        f.write_str("]")?;
    }
    Ok(())
}

impl Display for Literal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Integer(text) | Literal::Decimal(text) | Literal::Double(text) => f.write_str(text),
            Literal::String(value) => string_literal(f, value),
        }
    }
}

impl Display for Flwor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for clause in &self.clauses {
            match clause {
                FlworClause::For(bindings) => for_clause(f, bindings)?,
                FlworClause::Let(bindings) => let_clause(f, bindings)?,
            }
        }
        if let Some(condition) = &self.condition {
            f.write_str(WHERE)?;
            f.write_str(" ")?;
            condition.fmt(f)?;
            f.write_str(" ")?;
        }
        if let Some(order_by) = &self.order_by {
            order_by.fmt(f)?;
            f.write_str(" ")?;
        }
        f.write_str(RETURN)?;
        f.write_str(" ")?;
        self.result.fmt(f)
    }
}

/// Writes `keyword` and a space before the first of a clause's bindings,
/// and a comma before each later one, then `$` and the variable `var`
/// and its type, if declared.
fn binding_start(f: &mut Formatter<'_>, index: usize, keyword: &str, var: &QName) -> fmt::Result {
    if index == 0 {
        f.write_str(keyword)?;
        f.write_str(" ")?;
    } else {
        f.write_str(", ")?;
    }
    variable(f, var)
}

/// Writes a `ForClause` [XQuery Appendix A.1 \[34\]] and a space.
fn for_clause(f: &mut Formatter<'_>, bindings: &[ForBinding]) -> fmt::Result {
    for (index, binding) in bindings.iter().enumerate() {
        binding_start(f, index, FOR, &binding.var)?;
        type_declaration(f, binding.declared.as_ref())?;
        if let Some(position) = &binding.position {
            spaced(f, AT)?;
            variable(f, position)?;
        }
        spaced(f, IN)?;
        binding.input.fmt(f)?;
        f.write_str(" ")?;
    }
    Ok(())
}

/// Writes a `LetClause` [XQuery Appendix A.1 \[36\]] and a space.
fn let_clause(f: &mut Formatter<'_>, bindings: &[LetBinding]) -> fmt::Result {
    for (index, binding) in bindings.iter().enumerate() {
        binding_start(f, index, LET, &binding.var)?;
        type_declaration(f, binding.declared.as_ref())?;
        f.write_str(" := ")?;
        binding.value.fmt(f)?;
        f.write_str(" ")?;
    }
    Ok(())
}

impl Display for OrderBy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.stable {
            write!(f, "{STABLE} ")?;
        }
        write!(f, "{ORDER} {BY} ")?;
        for (index, spec) in self.specs.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            spec.key.fmt(f)?;
            match spec.direction {
                Some(Direction::Ascending) => write!(f, " {ASCENDING}")?,
                Some(Direction::Descending) => write!(f, " {DESCENDING}")?,
                None => {}
            }
            if let Some(order) = spec.empty {
                write!(f, " {EMPTY} {}", empty_order(order))?;
            }
            if let Some(collation) = &spec.collation {
                write!(f, " {COLLATION} ")?;
                string_literal(f, collation)?;
            }
        }
        Ok(())
    }
}

impl Display for Quantified {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let keyword = if self.quantifier == Quantifier::Some { SOME } else { EVERY };
        for (index, binding) in self.bindings.iter().enumerate() {
            binding_start(f, index, keyword, &binding.var)?;
            type_declaration(f, binding.declared.as_ref())?;
            spaced(f, IN)?;
            binding.input.fmt(f)?;
        }
        spaced(f, SATISFIES)?;
        self.test.fmt(f)
    }
}

impl Display for Typeswitch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(TYPESWITCH)?;
        f.write_str(" (")?;
        self.operand.fmt(f)?;
        f.write_str(")")?;
        for case in &self.cases {
            spaced(f, CASE)?;
            if let Some(var) = &case.var {
                variable(f, var)?;
                spaced(f, AS)?;
            }
            case.matches.fmt(f)?;
            spaced(f, RETURN)?;
            case.result.fmt(f)?;
        }
        spaced(f, DEFAULT)?;
        if let Some(var) = &self.default_var {
            variable(f, var)?;
            f.write_str(" ")?;
        }
        f.write_str(RETURN)?;
        f.write_str(" ")?;
        self.default.fmt(f)
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.root {
            Root::Relative => {}
            Root::Slash => f.write_str("/")?,
            Root::DoubleSlash => f.write_str("//")?,
        }
        if let Some(first) = &self.first {
            first.fmt(f)?;
        }
        for (separator, step) in &self.rest {
            f.write_str(if *separator == Separator::Slash { "/" } else { "//" })?;
            step.fmt(f)?;
        }
        Ok(())
    }
}

impl Display for StepExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            StepExpr::Axis(step) => step.fmt(f),
            StepExpr::Filter(primary, predicates) => filter(f, primary, predicates),
        }
    }
}

impl Display for AxisStep {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.step {
            Step::Forward(axis, test) => write!(f, "{}::{test}", forward_axis(*axis))?,
            Step::Reverse(axis, test) => write!(f, "{}::{test}", reverse_axis(*axis))?,
            Step::Abbreviated(test) => write!(f, "{test}")?,
            Step::Attribute(test) => write!(f, "@{test}")?,
            Step::Parent => f.write_str("..")?,
        }
        predicate_list(f, &self.predicates)
    }
}

/// How `axis` is written [XQuery Appendix A.1 \[73\]].
fn forward_axis(axis: ForwardAxis) -> &'static str {
    match axis {
        ForwardAxis::Child => CHILD,
        ForwardAxis::Descendant => DESCENDANT,
        ForwardAxis::Attribute => ATTRIBUTE,
        ForwardAxis::SelfAxis => SELF_AXIS,
        ForwardAxis::DescendantOrSelf => DESCENDANT_OR_SELF,
        ForwardAxis::FollowingSibling => FOLLOWING_SIBLING,
        ForwardAxis::Following => FOLLOWING,
    }
}

/// How `axis` is written [XQuery Appendix A.1 \[76\]].
fn reverse_axis(axis: ReverseAxis) -> &'static str {
    match axis {
        ReverseAxis::Parent => PARENT,
        ReverseAxis::Ancestor => ANCESTOR,
        ReverseAxis::PrecedingSibling => PRECEDING_SIBLING,
        ReverseAxis::Preceding => PRECEDING,
        ReverseAxis::AncestorOrSelf => ANCESTOR_OR_SELF,
    }
}
