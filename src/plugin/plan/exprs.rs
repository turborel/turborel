//! \[DESIGN\] The expressions of a plan form a rule gave back, made the
//! planner's and checked (RULES.md D277): each column one of the row, a
//! literal a value of its type, a function one name finds, an operator of
//! its operands' count, a comparison of comparable types, a condition of
//! BOOLEAN, and an opaque expression one the form kept, its columns named
//! again as other columns of the row.

use super::back::{Building, has, refused, within};
use super::from::Held;
use super::typed::{Typed, of_expr, of_value, parsed, same};
use super::{Expression, Operator};
use crate::error::Result;
use crate::logical::{Cond, Expr, Truth, map_cond_columns, map_expr_columns};
use crate::value::{ArithOp, CompOp, DataType, Value, assign};

/// What a literal's assignment to its type cites.
const LITERAL: &str = "ISO/IEC 9075-2:2023 §9.2";

/// The operator of a comparison, if `op` is one.
fn comparison(op: Operator) -> Option<CompOp> {
    match op {
        Operator::Eq => Some(CompOp::Eq),
        Operator::Ne => Some(CompOp::Ne),
        Operator::Lt => Some(CompOp::Lt),
        Operator::Gt => Some(CompOp::Gt),
        Operator::Le => Some(CompOp::Le),
        Operator::Ge => Some(CompOp::Ge),
        Operator::And
        | Operator::Or
        | Operator::Not
        | Operator::IsNull
        | Operator::IsNotNull
        | Operator::Add
        | Operator::Sub
        | Operator::Mul
        | Operator::Div
        | Operator::Negate => None,
    }
}

/// The arithmetic operator of `op`, if it is one of two operands.
fn arithmetic(op: Operator) -> Option<ArithOp> {
    match op {
        Operator::Add => Some(ArithOp::Add),
        Operator::Sub => Some(ArithOp::Sub),
        Operator::Mul => Some(ArithOp::Mul),
        Operator::Div => Some(ArithOp::Div),
        Operator::And
        | Operator::Or
        | Operator::Not
        | Operator::Eq
        | Operator::Ne
        | Operator::Lt
        | Operator::Gt
        | Operator::Le
        | Operator::Ge
        | Operator::IsNull
        | Operator::IsNotNull
        | Operator::Negate => None,
    }
}

/// Whether `op` makes a truth value.
fn logical(op: Operator) -> bool {
    comparison(op).is_some() || matches!(op, Operator::And | Operator::Or | Operator::Not | Operator::IsNull | Operator::IsNotNull)
}

/// The operands of `op`, which must be `n`.
fn operands(op: Operator, args: &[Expression], n: usize) -> Result<&[Expression]> {
    if args.len() == n { Ok(args) } else { Err(refused(&format!("operator {} has {} operands, not {n}", op.code(), args.len()))) }
}

impl Building<'_> {
    /// The planner's expression of `e`, over a row of `types`.
    pub(super) fn expr_of(&mut self, e: &Expression, types: &[Typed], depth: usize) -> Result<Expr> {
        let deeper = within(depth)?;
        Ok(match e {
            Expression::Column(i) => Expr::Column(has(*i, types.len())?),
            Expression::Literal { data_type, value } => literal(data_type, value)?,
            Expression::Call { name, args } => self.call(name, args, types, deeper)?,
            Expression::Operator { op, args } => {
                if logical(*op) {
                    return Ok(Expr::Truth(Box::new(self.cond_of(e, types, deeper)?)));
                }
                if let Some(a) = arithmetic(*op) {
                    let [x, y] = operands(*op, args, 2)? else { return Err(refused("two operands")) };
                    let (x, y) = (self.expr_of(x, types, deeper)?, self.expr_of(y, types, deeper)?);
                    numbers(&[&x, &y], types)?;
                    return Ok(Expr::Arith(Box::new(x), a, Box::new(y)));
                }
                let [x] = operands(*op, args, 1)? else { return Err(refused("one operand")) };
                let x = self.expr_of(x, types, deeper)?;
                numbers(&[&x], types)?;
                Expr::Neg(Box::new(x))
            }
            Expression::Cast { data_type, arg } => {
                let t = parsed(data_type)?.ok_or_else(|| refused("a CAST to no type"))?;
                Expr::Cast(Box::new(self.expr_of(arg, types, deeper)?), t)
            }
            Expression::Opaque { id, columns } => match self.held(*id, columns, types)? {
                Held::Expr(x) => x,
                Held::Cond(c) => Expr::Truth(Box::new(c)),
            },
        })
    }

    /// The call of the one function named `name`.
    fn call(&mut self, name: &str, args: &[Expression], types: &[Typed], depth: usize) -> Result<Expr> {
        let all = self.st.functions.scalars(name);
        let [f] = all.as_slice() else { return Err(refused(&format!("no one function is named {name}"))) };
        let args = args.iter().map(|a| self.expr_of(a, types, depth)).collect::<Result<Vec<_>>>()?;
        let known: Option<Vec<DataType>> = args.iter().map(|a| of_expr(a, types)).collect();
        if let Some(known) = known {
            f.0.return_type(&known).map_err(|e| refused(&format!("{name} takes no such arguments: {}", e.msg)))?;
        }
        Ok(Expr::Call(f.clone(), args))
    }

    /// The opaque expression `id`, its columns named again `columns` of a
    /// row of `types`, used once.
    fn held(&mut self, id: u32, columns: &[u32], types: &[Typed]) -> Result<Held> {
        let place = usize::try_from(id).ok().and_then(|i| i.checked_sub(1));
        let (held, was) = place.and_then(|p| self.kept.exprs.get(p)).ok_or_else(|| refused(&format!("it was given no expression {id}")))?;
        if !self.exprs.insert(id) {
            return Err(refused(&format!("it uses expression {id} twice")));
        }
        if columns.len() != was.len() {
            return Err(refused(&format!("expression {id} reads {} columns, not {}", was.len(), columns.len())));
        }
        let now = columns.iter().map(|c| has(*c, types.len())).collect::<Result<Vec<_>>>()?;
        let to = |c: usize| was.iter().position(|w| *w == c).and_then(|at| now.get(at)).copied().unwrap_or(c);
        Ok(match held {
            Held::Expr(e) => Held::Expr(map_expr_columns(e.clone(), &to)),
            Held::Cond(c) => Held::Cond(map_cond_columns(c.clone(), &to)),
        })
    }

    /// The planner's condition of `e`, over a row of `types`.
    pub(super) fn cond_of(&mut self, e: &Expression, types: &[Typed], depth: usize) -> Result<Cond> {
        let deeper = within(depth)?;
        match e {
            Expression::Operator { op, args } => {
                if !logical(*op) {
                    return self.valued(e, types, deeper);
                }
                self.logical(*op, args, types, deeper)
            }
            Expression::Literal { data_type, value } => {
                let t = parsed(data_type)?;
                if t.is_some() && !same(&t, &Some(DataType::Boolean)) {
                    return Err(refused("a condition is a literal of another type than BOOLEAN"));
                }
                Ok(Cond::Const(Truth::of(value)))
            }
            Expression::Opaque { id, columns } => Ok(match self.held(*id, columns, types)? {
                Held::Cond(c) => c,
                Held::Expr(x) => Cond::Value(x),
            }),
            Expression::Column(_) | Expression::Call { .. } | Expression::Cast { .. } => self.valued(e, types, deeper),
        }
    }

    /// The condition of a logical operator `op` of `args`.
    fn logical(&mut self, op: Operator, args: &[Expression], types: &[Typed], depth: usize) -> Result<Cond> {
        if let Some(c) = comparison(op) {
            let [x, y] = operands(op, args, 2)? else { return Err(refused("two operands")) };
            let (x, y) = (self.expr_of(x, types, depth)?, self.expr_of(y, types, depth)?);
            let (tx, ty) = (of_expr(&x, types), of_expr(&y, types));
            if tx.zip(ty).is_some_and(|(a, b)| !a.comparable(&b)) {
                return Err(refused("a comparison of types not comparable"));
            }
            return Ok(Cond::Compare(x, c, y));
        }
        let boxed = |c: Cond| Box::new(c);
        Ok(match op {
            Operator::And | Operator::Or => {
                let [x, y] = operands(op, args, 2)? else { return Err(refused("two operands")) };
                let (x, y) = (boxed(self.cond_of(x, types, depth)?), boxed(self.cond_of(y, types, depth)?));
                if op == Operator::And { Cond::And(x, y) } else { Cond::Or(x, y) }
            }
            Operator::Not => {
                let [x] = operands(op, args, 1)? else { return Err(refused("one operand")) };
                Cond::Not(boxed(self.cond_of(x, types, depth)?))
            }
            Operator::IsNull | Operator::IsNotNull => {
                let [x] = operands(op, args, 1)? else { return Err(refused("one operand")) };
                Cond::IsNull(self.expr_of(x, types, depth)?, op == Operator::IsNotNull)
            }
            Operator::Eq
            | Operator::Ne
            | Operator::Lt
            | Operator::Gt
            | Operator::Le
            | Operator::Ge
            | Operator::Add
            | Operator::Sub
            | Operator::Mul
            | Operator::Div
            | Operator::Negate => return Err(refused("an operator that is not logical")),
        })
    }

    /// The truth value of the value of `e`, which must be BOOLEAN where
    /// its type is known [ISO/IEC 9075-2:2023 §6.46].
    fn valued(&mut self, e: &Expression, types: &[Typed], depth: usize) -> Result<Cond> {
        let x = self.expr_of(e, types, depth)?;
        if of_expr(&x, types).is_some_and(|t| t != DataType::Boolean) {
            return Err(refused("a condition is a value of another type than BOOLEAN"));
        }
        Ok(Cond::Value(x))
    }
}

/// That none of `all` is of a type known to be no number's, as no
/// character or binary string is an operand of arithmetic
/// [X3.135-1986 §5.9 SR3; ISO/IEC 9075-2:2023 §6.30 SR3].
fn numbers(all: &[&Expr], types: &[Typed]) -> Result<()> {
    for x in all {
        if of_expr(x, types).is_some_and(|t| t.is_character() || t.is_binary()) {
            return Err(refused("a string is an operand of arithmetic"));
        }
    }
    Ok(())
}

/// The literal `value` of the type `data_type` writes: the value assigned
/// to the type, CAST to it where its own type is another.
fn literal(data_type: &str, value: &Value) -> Result<Expr> {
    let Some(t) = parsed(data_type)? else {
        return if value.is_null() { Ok(Expr::Const(Value::Null)) } else { Err(refused("a literal of no type is not the null value")) };
    };
    let v = assign(value.clone(), &t, LITERAL).map_err(|e| refused(&format!("a literal is no value of {t}: {}", e.msg)))?;
    let typed: Typed = Some(t.clone());
    Ok(if same(&of_value(&v), &typed) { Expr::Const(v) } else { Expr::Cast(Box::new(Expr::Const(v)), t) })
}

#[cfg(test)]
mod tests;
