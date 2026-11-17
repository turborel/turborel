//! An SQL/JSON path expression evaluated [ISO/IEC 9075-2:2023 §9.46 GR]:
//! the SQL/JSON sequence its `<JSON path wff>` gives of a context item and
//! the values of its named variables.
//!
//! An SQL/JSON sequence is a `Vec<Item>`, and an exception condition the
//! `SqlError` a wff's evaluation returns \[GR11\]. This file evaluates the
//! primaries \[GR11a to GR11f\] and defines Wrap and Unwrap \[GR7, GR8\];
//! the accessor ops are in `eval/accessors.rs` \[GR11g ii 1 to 5\], the item
//! methods in `eval/methods.rs` \[GR11g ii 6\], with the SQL numeric
//! functions they and `%` compute with in `eval/numbers.rs`, the signs and
//! operators in `eval/arithmetic.rs` \[GR11h to GR11j\], and the predicates of
//! filters in `eval/predicate.rs` \[GR12\].
//!
//! Evaluation recurses over the syntax tree, a call for each node on the
//! way down; the parser builds no tree more than the path module's
//! `MAX_NESTING` nodes deep (`Path`), so the recursion is bounded by it
//! (RULES.md R-CF-6).

mod accessors;
mod arithmetic;
mod methods;
mod numbers;
mod predicate;

use super::ast::{Accessor, Expr, Mode, Path, Predicate};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::{Item, Scalar};
use crate::value::{DecFloat, Decimal, Value};

impl Path {
    /// [§9.46 GR] The SQL/JSON sequence the path gives of the context item
    /// `context` \[GR11a\], its named variables valued as `variables` says,
    /// each the name it has after its `$` and the SQL/JSON sequence it is
    /// \[GR11b\]: one item, or none, of a `FORMAT JSON` argument that is the
    /// null value \[GR11b i 1 A\]. \[DESIGN\] A name is the variable's if it
    /// is the same string; the first of a name is taken.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `variables` values no variable of a name the path
    /// has \[SR4\], whatever the path would do with it; else the exception
    /// condition of the path's evaluation not caught by a filter's
    /// predicate \[GR11\]: SQLSTATE 2203A, 2203C, 22039, 22033, 22036, 2203B or
    /// 22038 as the General Rules raise them, or the error of a cast or an
    /// arithmetic operation the path does.
    pub fn evaluate(&self, context: &Item, variables: &[(String, Vec<Item>)]) -> Result<Vec<Item>> {
        self.check_variables(&variables.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>())?;
        Evaluator { mode: self.mode, context, variables, current: None, last: None }.wff(&self.expr)
    }

    /// [§9.46 SR4] That each named variable of the path has its name among
    /// `names`, those of the `<JSON argument>`s of the PASSING clause.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one has not.
    pub fn check_variables(&self, names: &[&str]) -> Result<()> {
        named_variables(&self.expr, names)
    }
}

/// What a wff is evaluated with: the mode, the context item, the named
/// variables, the current item of the innermost filter \[GR11c\], and the
/// last subscript of the current array of the innermost array accessor
/// \[GR11d\].
#[derive(Clone, Copy)]
struct Evaluator<'a> {
    mode: Mode,
    context: &'a Item,
    variables: &'a [(String, Vec<Item>)],
    current: Option<&'a Item>,
    last: Option<i128>,
}

impl<'a> Evaluator<'a> {
    /// The evaluator of a filter's predicate, of `item` the current item.
    fn with_current<'b>(&self, item: &'b Item) -> Evaluator<'b>
    where
        'a: 'b,
    {
        Evaluator { mode: self.mode, context: self.context, variables: self.variables, current: Some(item), last: self.last }
    }

    /// The evaluator of a subscript, of an array of `len` elements the
    /// current array: `last` is `len` − 1 \[GR11d, NOTE 494\].
    fn with_array(&self, len: usize) -> Evaluator<'a> {
        Evaluator { last: Some(i128::try_from(len).map_or(i128::MAX, |n| n.saturating_sub(1))), ..*self }
    }

    /// [§9.46 GR11] The SQL/JSON sequence `expr` gives.
    fn wff(&self, expr: &Expr) -> Result<Vec<Item>> {
        match expr {
            // [GR11e] The literal's value.
            Expr::Literal(item) => Ok(vec![item.clone()]),
            // [GR11a ii] The context item.
            Expr::Context => Ok(vec![self.context.clone()]),
            // [GR11b ii 3] The item the variable is passed.
            Expr::Variable(name) => lookup(self.variables, name).cloned(),
            // [GR11c] The current item; [SR5] `@` is within a filter's
            // predicate, so one is current.
            Expr::Current => Ok(self.current.into_iter().cloned().collect()),
            // [GR11d] The current array's last subscript; [SR6] `last` is
            // within a subscript, so an array is current.
            Expr::Last => Ok(self.last.map(|last| Item::Scalar(Decimal::new(last, 0).into())).into_iter().collect()),
            // [GR11g i] An exception of the base is the result.
            Expr::Access(base, op) => {
                let base = self.wff(base)?;
                self.access(base, op)
            }
            Expr::Signed(sign, operand) => self.signed(*sign, operand),
            Expr::Arithmetic(left, op, right) => self.arithmetic(left, *op, right),
        }
    }

    /// [§9.46 GR11g ii] The accessor op `op` applied to `base`.
    fn access(&self, base: Vec<Item>, op: &Accessor) -> Result<Vec<Item>> {
        match op {
            Accessor::Member(key) => self.member(base, key),
            Accessor::AnyMember => self.any_member(base),
            Accessor::Subscripts(list) => self.subscripts(base, list),
            Accessor::AnyElement => self.any_element(base),
            Accessor::Filter(predicate) => Ok(self.filter(base, predicate)),
            Accessor::Method(method) => self.method(base, *method),
        }
    }

    /// `seq`, Unwrap(`seq`) if the mode is lax.
    fn lax_unwrap(&self, seq: Vec<Item>) -> Vec<Item> {
        match self.mode {
            Mode::Lax => unwrap(seq),
            Mode::Strict => seq,
        }
    }

    /// `seq`, Wrap(`seq`) if the mode is lax.
    fn lax_wrap(&self, seq: Vec<Item>) -> Vec<Item> {
        match self.mode {
            Mode::Lax => wrap(seq),
            Mode::Strict => seq,
        }
    }

    /// Whether the mode is strict.
    fn strict(&self) -> bool {
        self.mode == Mode::Strict
    }
}

/// [§9.46 GR7] Wrap(`seq`): each item that is no array, an array of it
/// alone.
fn wrap(seq: Vec<Item>) -> Vec<Item> {
    seq.into_iter()
        .map(|item| match item {
            Item::Array(_) => item,
            Item::Null | Item::Scalar(_) | Item::Object(_) => Item::Array(vec![item]),
        })
        .collect()
}

/// [§9.46 GR8] Unwrap(`seq`): each item that is an array, its elements in
/// order; one level only.
fn unwrap(seq: Vec<Item>) -> Vec<Item> {
    let mut unwrapped = Vec::with_capacity(seq.len());
    for item in seq {
        match item {
            Item::Array(elements) => unwrapped.extend(elements),
            Item::Null | Item::Scalar(_) | Item::Object(_) => unwrapped.push(item),
        }
    }
    unwrapped
}

/// [§9.46 GR11b] The sequence the variable `name` is passed.
///
/// # Errors
///
/// SQLCODE -103 if none is \[SR4\].
fn lookup<'v>(variables: &'v [(String, Vec<Item>)], name: &str) -> Result<&'v Vec<Item>> {
    variables.iter().find(|(passed, _)| passed == name).map(|(_, seq)| seq).ok_or_else(|| not_passed(name))
}

/// [§9.46 SR4] That `name` is among `names`.
///
/// # Errors
///
/// SQLCODE -103 if it is not.
fn declared(names: &[&str], name: &str) -> Result<()> {
    if names.contains(&name) { Ok(()) } else { Err(not_passed(name)) }
}

/// The error of the named variable `name`, which no `<JSON argument>`
/// names \[SR4\].
fn not_passed(name: &str) -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        format!("[ISO/IEC 9075-2:2023 §9.46 SR4] the named variable ${name} of the SQL/JSON path expression is passed no value"),
    )
}

/// [§9.46 SR4] That `variables` names every named variable of `expr`:
/// checked of the whole path before it is evaluated, so a filter, which
/// takes an exception for Unknown, does not hide it.
fn named_variables(expr: &Expr, variables: &[&str]) -> Result<()> {
    match expr {
        Expr::Literal(_) | Expr::Context | Expr::Current | Expr::Last => Ok(()),
        Expr::Variable(name) => declared(variables, name),
        Expr::Access(base, op) => {
            named_variables(base, variables)?;
            accessor_variables(op, variables)
        }
        Expr::Signed(_, operand) => named_variables(operand, variables),
        Expr::Arithmetic(left, _, right) => {
            named_variables(left, variables)?;
            named_variables(right, variables)
        }
    }
}

/// [§9.46 SR4] `named_variables` of the wffs of an accessor op.
fn accessor_variables(op: &Accessor, variables: &[&str]) -> Result<()> {
    match op {
        Accessor::Member(_) | Accessor::AnyMember | Accessor::AnyElement | Accessor::Method(_) => Ok(()),
        Accessor::Subscripts(list) => {
            for subscript in list {
                named_variables(&subscript.from, variables)?;
                subscript.to.as_ref().map_or(Ok(()), |to| named_variables(to, variables))?;
            }
            Ok(())
        }
        Accessor::Filter(predicate) => predicate_variables(predicate, variables),
    }
}

/// [§9.46 SR4] `named_variables` of the wffs of a predicate.
fn predicate_variables(predicate: &Predicate, variables: &[&str]) -> Result<()> {
    match predicate {
        Predicate::Exists(expr) | Predicate::LikeRegex(expr, _) => named_variables(expr, variables),
        Predicate::Compare(left, _, right) | Predicate::StartsWith(left, right) => {
            named_variables(left, variables)?;
            named_variables(right, variables)
        }
        Predicate::IsUnknown(operand) | Predicate::Not(operand) => predicate_variables(operand, variables),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_variables(left, variables)?;
            predicate_variables(right, variables)
        }
    }
}

/// The data exception `state` of a General Rule, its message `cited`
/// beginning with the rule.
fn exception(state: SqlState, cited: &str) -> SqlError {
    SqlError::new(Code::Data, cited).with_state(state)
}

/// An SQL/JSON scalar that is a number [§4.48.3]: exact, approximate or
/// decimal floating-point.
#[derive(Debug, Clone, Copy)]
enum Number {
    /// An exact number.
    Exact(Decimal),
    /// An approximate number.
    Approx(f64),
    /// A decimal floating-point number.
    DecFloat(DecFloat),
}

impl Number {
    /// The SQL value it is.
    fn value(self) -> Value {
        match self {
            Number::Exact(d) => Value::Exact(d),
            Number::Approx(x) => Value::Approx(x),
            Number::DecFloat(d) => Value::DecFloat(d),
        }
    }
}

/// The number `item` is, if it is one.
fn number(item: &Item) -> Option<Number> {
    match item.as_scalar()?.value() {
        Value::Exact(d) => Some(Number::Exact(*d)),
        Value::Approx(x) => Some(Number::Approx(*x)),
        Value::DecFloat(d) => Some(Number::DecFloat(*d)),
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Extension(_) => None,
    }
}

/// The number the SQL/JSON sequence `seq` is, if it is a singleton
/// numeric value: one item, a number.
fn singleton_number(seq: &[Item]) -> Option<Number> {
    let [only] = seq else { return None };
    number(only)
}

/// The character string `item` is, if it is one.
fn string(item: &Item) -> Option<&str> {
    if let Value::Char(s) = item.as_scalar()?.value() { Some(s) } else { None }
}

/// The SQL value `value` as an SQL/JSON item: the null value as the
/// SQL/JSON null, as of a variable \[GR11b ii 2\].
fn item(value: Value) -> Item {
    Scalar::new(value).map_or(Item::Null, Item::Scalar)
}

/// The exact number `n`, as `size()` and `last` give one.
fn count(n: usize) -> Item {
    Item::Scalar(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0).into())
}

#[cfg(test)]
mod tests;
