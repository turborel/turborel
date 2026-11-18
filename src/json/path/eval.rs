//! An SQL/JSON path expression evaluated [ISO/IEC 9075-2:2023 §9.46 GR]:
//! the SQL/JSON sequence its `<JSON path wff>` gives of a context item and
//! the values of its named variables.
//!
//! An SQL/JSON sequence is a `Seq`, and an exception condition the
//! `SqlError` a wff's evaluation returns \[GR11\]. \[DESIGN\] An item of a
//! sequence that is the context item, a variable's or one within them is
//! borrowed of it, not copied, so that each keeps its place, which
//! `keyvalue()` identifies an object by (`Identity`); an item made by the
//! evaluation is owned. This file evaluates the
//! primaries \[GR11a to GR11f\] and defines Wrap and Unwrap \[GR7, GR8\];
//! the accessor ops are in `eval/accessors.rs` \[GR11g ii 1 to 5\], the item
//! methods in `eval/methods.rs` \[GR11g ii 6\], with the SQL numeric
//! functions they and `%` compute with in `eval/numbers.rs`, the signs and
//! operators in `eval/arithmetic.rs` \[GR11h to GR11j\], and the predicates of
//! filters in `eval/predicate.rs` \[GR12\].
//!
//! Evaluation recurses over the syntax tree, a call for each node on the
//! way down; the parser builds no tree more than the path module's
//! `MAX_PATH_NESTING` nodes deep (`Path`), so the recursion is bounded by it
//! (RULES.md R-CF-6).

mod accessors;
mod arithmetic;
mod methods;
mod numbers;
mod predicate;

use std::borrow::Cow;
use std::cell::{Cell, OnceCell};
use std::collections::HashMap;

use super::ast::{Accessor, Expr, Mode, Path, Predicate};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::{Item, MAX_NESTING, Scalar};
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
    /// predicate \[GR11\]: SQLSTATE 2203A, 2203C, 22039, 22033, 22036, 22031,
    /// 2202V, 2202W, 2202X, 2202Y, 2202Z, 2203B or 22038 as the General
    /// Rules raise them, or the error of a cast or an arithmetic operation
    /// the path does; SQLCODE -900 of a `datetime()` of a string of more
    /// digits of a second's fraction than turborel's maximum.
    pub fn evaluate(&self, context: &Item, variables: &[(String, Vec<Item>)]) -> Result<Vec<Item>> {
        self.check_variables(&variables.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>())?;
        let identity = Identity::new(context, variables);
        let evaluator = Evaluator { mode: self.mode, zone: self.zone, context, variables, identity: &identity, current: None, last: None };
        Ok(evaluator.wff(&self.expr)?.into_iter().map(Cow::into_owned).collect())
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

/// An SQL/JSON sequence evaluated: each item borrowed of the context item
/// or a variable if it is one of theirs, else owned.
type Seq<'a> = Vec<Cow<'a, Item>>;

/// What a wff is evaluated with: the mode, the SQL-session's displacement
/// (`Path::zone`), the context item, the named variables, the identities
/// of their objects, the current item of the innermost filter \[GR11c\],
/// and the last subscript of the current array of the innermost array
/// accessor \[GR11d\].
#[derive(Clone, Copy)]
struct Evaluator<'a> {
    mode: Mode,
    zone: i16,
    context: &'a Item,
    variables: &'a [(String, Vec<Item>)],
    identity: &'a Identity<'a>,
    current: Option<&'a Item>,
    last: Option<i128>,
}

impl<'a> Evaluator<'a> {
    /// The evaluator of a filter's predicate, of `item` the current item.
    fn with_current<'b>(&self, item: &'b Item) -> Evaluator<'b>
    where
        'a: 'b,
    {
        Evaluator {
            mode: self.mode,
            zone: self.zone,
            context: self.context,
            variables: self.variables,
            identity: self.identity,
            current: Some(item),
            last: self.last,
        }
    }

    /// The evaluator of a subscript, of an array of `len` elements the
    /// current array: `last` is `len` − 1 \[GR11d, NOTE 494\].
    fn with_array(&self, len: usize) -> Evaluator<'a> {
        Evaluator { last: Some(i128::try_from(len).map_or(i128::MAX, |n| n.saturating_sub(1))), ..*self }
    }

    /// [§9.46 GR11] The SQL/JSON sequence `expr` gives.
    fn wff(&self, expr: &Expr) -> Result<Seq<'a>> {
        match expr {
            // [GR11e] The literal's value.
            Expr::Literal(item) => Ok(vec![Cow::Owned(item.clone())]),
            // [GR11a ii] The context item.
            Expr::Context => Ok(vec![Cow::Borrowed(self.context)]),
            // [GR11b ii 3] The item the variable is passed.
            Expr::Variable(name) => Ok(lookup(self.variables, name)?.iter().map(Cow::Borrowed).collect()),
            // [GR11c] The current item; [SR5] `@` is within a filter's
            // predicate, so one is current.
            Expr::Current => Ok(self.current.map(Cow::Borrowed).into_iter().collect()),
            // [GR11d] The current array's last subscript; [SR6] `last` is
            // within a subscript, so an array is current.
            Expr::Last => Ok(self.last.map(|last| Cow::Owned(Item::Scalar(Decimal::new(last, 0).into()))).into_iter().collect()),
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
    fn access(&self, base: Seq<'a>, op: &Accessor) -> Result<Seq<'a>> {
        match op {
            Accessor::Member(key) => self.member(base, key),
            Accessor::AnyMember => self.any_member(base),
            Accessor::Subscripts(list) => self.subscripts(base, list),
            Accessor::AnyElement => self.any_element(base),
            Accessor::Filter(predicate) => Ok(self.filter(base, predicate)),
            Accessor::Method(method) => self.method(base, method),
        }
    }

    /// `seq`, Unwrap(`seq`) if the mode is lax.
    fn lax_unwrap(&self, seq: Seq<'a>) -> Seq<'a> {
        match self.mode {
            Mode::Lax => unwrap(seq),
            Mode::Strict => seq,
        }
    }

    /// Whether the mode is strict.
    fn strict(&self) -> bool {
        self.mode == Mode::Strict
    }
}

/// The elements of `item` if it is an array, each borrowed if it is; of
/// another item none.
fn elements(item: Cow<'_, Item>) -> std::result::Result<Seq<'_>, Cow<'_, Item>> {
    match item {
        Cow::Borrowed(Item::Array(elements)) => Ok(elements.iter().map(Cow::Borrowed).collect()),
        Cow::Owned(Item::Array(elements)) => Ok(elements.into_iter().map(Cow::Owned).collect()),
        Cow::Borrowed(_) | Cow::Owned(_) => Err(item),
    }
}

/// [§9.46 GR8] Unwrap(`seq`): each item that is an array, its elements in
/// order; one level only.
fn unwrap(seq: Seq<'_>) -> Seq<'_> {
    let mut unwrapped = Vec::with_capacity(seq.len());
    for item in seq {
        match elements(item) {
            Ok(elements) => unwrapped.extend(elements),
            Err(item) => unwrapped.push(item),
        }
    }
    unwrapped
}

/// [§9.46 GR11g ii 6 C VIII 1 c i] What identifies each object `keyvalue()`
/// is given: \[DESIGN\] its place in a walk of the context item and
/// then of each variable's items in turn, each item counted before those
/// within it, from 0, if it is one of theirs; else, an object the
/// evaluation made, a number past them all, a new one each time it is
/// asked. The places are counted the first time one is asked for.
struct Identity<'a> {
    /// The context item.
    context: &'a Item,
    /// The named variables.
    variables: &'a [(String, Vec<Item>)],
    /// Each item's place, by its address, and how many there are.
    places: OnceCell<(HashMap<usize, u64>, u64)>,
    /// How many identities past the places have been given.
    made: Cell<u64>,
}

impl<'a> Identity<'a> {
    /// The identities of the objects of `context` and `variables`.
    fn new(context: &'a Item, variables: &'a [(String, Vec<Item>)]) -> Identity<'a> {
        Identity { context, variables, places: OnceCell::new(), made: Cell::new(0) }
    }

    /// The identity of an item: its place, if it is `borrowed` of the
    /// context item or a variable, else, made by the path, a new one.
    fn of(&self, borrowed: Option<&Item>) -> u64 {
        let (places, count) = self.places.get_or_init(|| {
            let mut places = HashMap::new();
            let mut count = 0_u64;
            let sources = std::iter::once(self.context).chain(self.variables.iter().flat_map(|(_, seq)| seq.iter()));
            for source in sources {
                place(source, &mut places, &mut count, 0);
            }
            (places, count)
        });
        if let Some(borrowed) = borrowed
            && let Some(found) = places.get(&address(borrowed))
        {
            return *found;
        }
        let made = self.made.get();
        self.made.set(made.saturating_add(1));
        count.saturating_add(made)
    }
}

/// Where `item` is in memory, which is what tells it from an equal item.
fn address(item: &Item) -> usize {
    std::ptr::from_ref(item).addr()
}

/// [§9.46 GR11g ii 6 C VIII 1 c i] `item`, within `depth` arrays and
/// objects, and the items within it, each given the next place of `count`
/// in `places`. An item nests at most `MAX_NESTING` deep, as it was parsed,
/// so the walk stops there (RULES.md R-CF-6).
fn place(item: &Item, places: &mut HashMap<usize, u64>, count: &mut u64, depth: usize) {
    places.insert(address(item), *count);
    *count = count.saturating_add(1);
    if depth > MAX_NESTING {
        return;
    }
    let deeper = depth.saturating_add(1);
    match item {
        Item::Null | Item::Scalar(_) => {}
        Item::Array(elements) => elements.iter().for_each(|element| place(element, places, count, deeper)),
        Item::Object(members) => members.iter().for_each(|member| place(&member.value, places, count, deeper)),
    }
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
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => None,
    }
}

/// The number the SQL/JSON sequence `seq` is, if it is a singleton
/// numeric value: one item, a number.
fn singleton_number(seq: &[Cow<'_, Item>]) -> Option<Number> {
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
