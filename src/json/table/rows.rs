//! The rows of a `<JSON table>` [ISO/IEC 9075-2:2023 §7.11], made one at
//! a time as they are read: the rows of each `<JSON table primitive>`
//! \[GR1\] joined laterally as the plan tree says [SR1k iii 2 to 6].
//!
//! Each node of the plan is a `State` opened over the row its rows extend,
//! `base`, which holds the null value in every column the node does not
//! give, so a row of one operand of UNION is null extended [SR1i ii NOTE
//! 244] and a child's row is its parent's row with the child's columns. A
//! primitive's row pattern is evaluated as its node is opened; its rows'
//! columns as each row is read. An operand of UNION is opened when the
//! one before it is read to its end, an operand of CROSS after the first
//! for each row of those before it, so no rows are held but the SQL/JSON
//! sequences of the row patterns opened.
//!
//! The plan tree is no deeper than the statement's syntax tree, which the
//! parser bounds by `parser::MAX_NESTING_DEPTH`, so the recursion over its
//! states is bounded (RULES.md R-CF-6).

use std::sync::Arc;

use super::{JsonTable, PlanNode, TableErrorBehavior, column, has_text};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::query::json_input;
use crate::json::{Item, Keys, is_exception};
use crate::value::Value;

/// A row of a JSON table: its values, and the warning a cast of one of
/// them raised, if any.
pub type TableRow = (Vec<Value>, Option<SqlState>);

/// The named variables of the PASSING clause, each its name and its
/// SQL/JSON sequence [§9.46 GR11b].
type Variables = Vec<(String, Vec<Item>)>;

/// The Syntax Rule a context item's type breaks if it is no string.
const CONTEXT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.14 SR1] the context item of JSON_TABLE is a string";
/// The Syntax Rule a FORMAT JSON argument's type breaks if it is no
/// string.
const ARGUMENT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.12 SR6] an argument FORMAT JSON is a string";

/// The rows of a JSON table of one context item, read one at a time
/// (`Cursor::next`), of the `JsonTable` it was opened of.
pub struct Cursor {
    /// The PASSING arguments' sequences.
    variables: Variables,
    /// The rows left.
    state: State,
}

impl JsonTable {
    /// [§7.11 GR1] The rows of the JSON table of the context item
    /// `context` and the values `passing` of its PASSING arguments, in
    /// order: none if `context` is the null value \[GR1a\]; else the rows
    /// of its plan [§9.47] \[GR1d\], none in place of an exception
    /// condition of a row pattern if its error behavior is EMPTY \[GR1e i
    /// 2\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `context`, or a FORMAT JSON argument, is no string
    /// [§10.14 SR1, §10.12 SR6]; -900 of a binary string; the exception
    /// condition of the context item's or an argument's text, or of the
    /// outermost row pattern, if its error behavior is ERROR \[GR1e i 1\].
    pub fn open(&self, context: &Value, passing: &[Value]) -> Result<Cursor> {
        let started = match self.start(context, passing) {
            Ok(Some(started)) => started,
            Ok(None) => return Ok(Cursor { variables: Vec::new(), state: State::Empty }),
            Err(e) => {
                if self.swallows(&e) {
                    return Ok(Cursor { variables: Vec::new(), state: State::Empty });
                }
                return Err(e);
            }
        };
        let (item, variables) = started;
        let base = (vec![Value::Null; self.width], None);
        let state = Opener { table: self, variables: &variables }.open(self.top, Some(Arc::new(item)), base)?;
        Ok(Cursor { variables, state })
    }

    /// [§9.47] The context item parsed and the PASSING arguments made
    /// sequences; none if the context item is the null value.
    fn start(&self, context: &Value, passing: &[Value]) -> Result<Option<(Item, Variables)>> {
        let Some(item) = json_input(context, CONTEXT_RULE, Keys::Any)? else { return Ok(None) };
        let mut variables = Vec::with_capacity(passing.len());
        for ((name, format_json), value) in self.passing.iter().zip(passing) {
            variables.push((name.clone(), passed(value, *format_json)?));
        }
        Ok(Some((item, variables)))
    }

    /// [§7.11 GR1e i 2] Whether `error` is an exception condition the
    /// error behavior EMPTY replaces by an empty table.
    fn swallows(&self, error: &SqlError) -> bool {
        self.on_error == TableErrorBehavior::Empty && is_exception(error)
    }
}

/// [§9.46 GR11b] The SQL/JSON sequence of a PASSING argument's `value`:
/// of FORMAT JSON or the JSON type, the item it is, none of the null
/// value; else the item its value is.
fn passed(value: &Value, format_json: bool) -> Result<Vec<Item>> {
    if !format_json && !matches!(value, Value::Json(_)) {
        return Ok(vec![Item::of_value(value)?]);
    }
    Ok(json_input(value, ARGUMENT_RULE, Keys::Any)?.into_iter().collect())
}

impl Cursor {
    /// The next row of `table`, the JSON table it was opened of; none
    /// after the last. `defaults` gives the value of the DEFAULT of a
    /// place, as a column's behavior asks for it.
    ///
    /// # Errors
    ///
    /// The exception condition of a column \[GR1e ii 2 D, 3 D\], of a
    /// nested row pattern or of a chaining item if the error behavior is
    /// ERROR \[GR1e i 1, GR1e ii 4 A II 1\], an error not an exception
    /// condition, or the error of a DEFAULT evaluated.
    pub fn next(&mut self, table: &JsonTable, defaults: &mut impl FnMut(usize) -> Result<Value>) -> Option<Result<TableRow>> {
        let mut env = Env { open: Opener { table, variables: &self.variables }, defaults };
        self.state.next(&mut env)
    }
}

/// What opens the states of a JSON table's nodes.
#[derive(Clone, Copy)]
struct Opener<'a> {
    table: &'a JsonTable,
    variables: &'a [(String, Vec<Item>)],
}

/// What a state reads its rows with.
struct Env<'a, F> {
    open: Opener<'a>,
    defaults: &'a mut F,
}

/// The rows left of a node of the plan.
enum State {
    /// No rows.
    Empty,
    /// A primitive's.
    Items(Items),
    /// A parent's and its children's.
    Parent(Box<Parent>),
    /// A product of siblings.
    Cross(Box<Cross>),
    /// A union of siblings.
    Union(Box<Union>),
}

/// [§7.11 GR1e ii] The rows of a primitive: of each item of its row
/// pattern's sequence left, `base` with its columns.
struct Items {
    primitive: usize,
    sequence: std::vec::IntoIter<Item>,
    /// The ordinal of the last row made.
    ordinal: usize,
    base: TableRow,
}

/// [§7.11 SR1k iii 2, 3] A parent's rows, each joined to its child's rows
/// of its item, OUTER keeping it null extended if they are none.
struct Parent {
    left: Items,
    inner: bool,
    child: usize,
    /// The parent's row being joined, its child's rows left, and whether
    /// they gave one.
    current: Option<(TableRow, State, bool)>,
}

/// [§7.11 SR1k iii 5] The product of siblings: each row of the first with
/// each row of the product of the rest.
struct Cross {
    left: State,
    rest: Vec<usize>,
    context: Option<Arc<Item>>,
    current: Option<State>,
}

/// [§7.11 SR1k iii 4] The union join of siblings: the rows of each in
/// turn, null extended.
struct Union {
    current: State,
    rest: std::vec::IntoIter<usize>,
    context: Option<Arc<Item>>,
    base: TableRow,
}

impl Opener<'_> {
    /// The state of `node`, of the context item `context`, none being the
    /// null value, its rows extending `base`.
    fn open(self, node: usize, context: Option<Arc<Item>>, base: TableRow) -> Result<State> {
        match self.table.nodes.get(node) {
            None => Err(broken()),
            Some(PlanNode::Primitive(p)) => Ok(self.items(*p, context.as_deref(), base)?.map_or(State::Empty, State::Items)),
            Some(PlanNode::Parent { primitive, inner, child }) => Ok(match self.items(*primitive, context.as_deref(), base)? {
                Some(left) => State::Parent(Box::new(Parent { left, inner: *inner, child: *child, current: None })),
                None => State::Empty,
            }),
            Some(PlanNode::Siblings { cross, operands }) => self.siblings(operands, *cross, context, base),
        }
    }

    /// The state of the siblings `operands`, one or more, of CROSS or
    /// UNION.
    fn siblings(self, operands: &[usize], cross: bool, context: Option<Arc<Item>>, base: TableRow) -> Result<State> {
        let Some((&first, rest)) = operands.split_first() else { return Err(broken()) };
        if rest.is_empty() {
            return self.open(first, context, base);
        }
        if cross {
            let left = self.open(first, context.clone(), base)?;
            return Ok(State::Cross(Box::new(Cross { left, rest: rest.to_vec(), context, current: None })));
        }
        let current = self.open(first, context.clone(), base.clone())?;
        Ok(State::Union(Box::new(Union { current, rest: Vec::from(rest).into_iter(), context, base })))
    }

    /// [§7.11 GR1] The rows of primitive `p` of the context item
    /// `context`: none of the null value \[GR1a\], nor in place of an
    /// exception condition of its row pattern if the error behavior is
    /// EMPTY \[GR1e i 2\].
    fn items(self, p: usize, context: Option<&Item>, base: TableRow) -> Result<Option<Items>> {
        let primitive = self.table.primitives.get(p).ok_or_else(broken)?;
        let Some(context) = context else { return Ok(None) };
        match primitive.path.evaluate(context, self.variables) {
            Ok(seq) => Ok(Some(Items { primitive: p, sequence: seq.into_iter(), ordinal: 0, base })),
            Err(e) => {
                if self.table.swallows(&e) {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// [§7.11 GR1e ii 4] The child `child` of a row of its parent made of
    /// `item`, the chaining column's value: the item, or, if it has no
    /// JSON text \[§9.43 GR2a ii\], the null value, of which the child has
    /// no rows, or its exception condition if the error behavior is ERROR.
    fn chained(self, child: usize, item: Item, base: TableRow) -> Result<State> {
        if has_text(&item) {
            return self.open(child, Some(Arc::new(item)), base);
        }
        if self.table.on_error == TableErrorBehavior::Error {
            return Err(SqlError::new(
                Code::Data,
                "[ISO/IEC 9075-2:2023 §7.11 GR1e ii 4 A II 1] the SQL/JSON item of a row with NESTED COLUMNS has no JSON text",
            )
            .with_state(SqlState::INVALID_JSON_TEXT));
        }
        self.open(child, None, base)
    }
}

impl State {
    /// Its next row.
    fn next<F: FnMut(usize) -> Result<Value>>(&mut self, env: &mut Env<'_, F>) -> Option<Result<TableRow>> {
        match self {
            State::Empty => None,
            State::Items(items) => items.next(env).map(|made| made.map(|(row, _)| row)),
            State::Parent(parent) => parent.next(env),
            State::Cross(cross) => cross.next(env),
            State::Union(union) => union.next(env),
        }
    }
}

impl Items {
    /// The next row and the item it is made of.
    fn next<F: FnMut(usize) -> Result<Value>>(&mut self, env: &mut Env<'_, F>) -> Option<Result<(TableRow, Item)>> {
        let item = self.sequence.next()?;
        self.ordinal = self.ordinal.saturating_add(1);
        let Some(primitive) = env.open.table.primitives.get(self.primitive) else { return Some(Err(broken())) };
        let (mut values, mut warned) = self.base.clone();
        for (place, column) in &primitive.columns {
            let (value, warning) = match column::value(column, &item, self.ordinal, env.defaults) {
                Ok(made) => made,
                Err(e) => return Some(Err(e)),
            };
            warned = warned.or(warning);
            if let Some(slot) = values.get_mut(*place) {
                *slot = value;
            }
        }
        Some(Ok(((values, warned), item)))
    }
}

impl Parent {
    /// Its next row.
    fn next<F: FnMut(usize) -> Result<Value>>(&mut self, env: &mut Env<'_, F>) -> Option<Result<TableRow>> {
        loop {
            if let Some((_, child, gave)) = &mut self.current {
                match child.next(env) {
                    Some(Ok(row)) => {
                        *gave = true;
                        return Some(Ok(row));
                    }
                    Some(Err(e)) => return Some(Err(e)),
                    None => {}
                }
                // [SR1k iii 2] LEFT OUTER JOIN ON 0=0: the parent's row,
                // null extended, if its child gave none.
                if let Some((row, _, false)) = self.current.take()
                    && !self.inner
                {
                    return Some(Ok(row));
                }
            }
            let (row, item) = match self.left.next(env)? {
                Ok(made) => made,
                Err(e) => return Some(Err(e)),
            };
            match env.open.chained(self.child, item, row.clone()) {
                Ok(child) => self.current = Some((row, child, false)),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Cross {
    /// Its next row.
    fn next<F: FnMut(usize) -> Result<Value>>(&mut self, env: &mut Env<'_, F>) -> Option<Result<TableRow>> {
        loop {
            if let Some(current) = &mut self.current {
                if let Some(made) = current.next(env) {
                    return Some(made);
                }
                self.current = None;
            }
            let row = match self.left.next(env)? {
                Ok(row) => row,
                Err(e) => return Some(Err(e)),
            };
            match env.open.siblings(&self.rest, true, self.context.clone(), row) {
                Ok(rest) => self.current = Some(rest),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Union {
    /// Its next row.
    fn next<F: FnMut(usize) -> Result<Value>>(&mut self, env: &mut Env<'_, F>) -> Option<Result<TableRow>> {
        loop {
            if let Some(made) = self.current.next(env) {
                return Some(made);
            }
            let next = self.rest.next()?;
            match env.open.open(next, self.context.clone(), self.base.clone()) {
                Ok(state) => self.current = state,
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// \[DESIGN\] A broken invariant of a JSON table the binder made.
fn broken() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a JSON table's plan names a node or a primitive it does not have")
}
