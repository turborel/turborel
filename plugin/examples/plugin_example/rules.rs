//! The example's optimizer rules (kind 37): CONTRADICTION, which finds a
//! filter no row passes, as `C > 5 AND C < 3`, and makes it and what it
//! reads an empty VALUES of its columns; BROKEN, which gives a plan that
//! loses every column, and ERRING, which fails: the two the planner skips.
//! SKEW, a procedure, writes what the database keeps of a table's column
//! into a table, as a program asks for statistics (STATISTICS).

use std::cmp::Ordering;

use turborel::value::{Value, compare};
use turborel_plugin::plan::{Body, Expression, Form, MAX_DEPTH, Node, Op, Operator};
use turborel_plugin::{Answer, Cx, PluginError, Procedure, Rule};

/// CONTRADICTION.
pub struct Contradiction;

/// BROKEN, or ERRING if it fails.
pub struct Broken {
    /// Whether it fails rather than give a plan not valid.
    pub fails: bool,
}

/// SKEW.
pub struct Skew;

/// The column and value of `e`, a comparison of a column with a literal
/// by `op`, and whether `op` is strict.
fn bound(e: &Expression) -> Option<(Operator, u32, &Value)> {
    let Expression::Operator { op, args } = e else { return None };
    let [Expression::Column(c), Expression::Literal { value, .. }] = args.as_slice() else { return None };
    Some((*op, *c, value))
}

/// Whether `low` and `high`, a lower and an upper bound of one column, no
/// value is between.
fn empty((low_op, low): (Operator, &Value), (high_op, high): (Operator, &Value)) -> bool {
    let strict = low_op == Operator::Gt || high_op == Operator::Lt;
    let lows = [Operator::Gt, Operator::Ge].contains(&low_op);
    let highs = [Operator::Lt, Operator::Le].contains(&high_op);
    let order = compare(low, high);
    lows && highs && order.is_some_and(|o| o == Ordering::Greater || (strict && o == Ordering::Equal))
}

/// Whether `e` is a condition no row is true of: a column above one
/// literal and below another no value is between.
fn contradicts(e: &Expression) -> bool {
    let Expression::Operator { op: Operator::And, args } = e else { return false };
    let [a, b] = args.as_slice() else { return false };
    let (Some((op_a, col_a, a)), Some((op_b, col_b, b))) = (bound(a), bound(b)) else { return false };
    col_a == col_b && (empty((op_a, a), (op_b, b)) || empty((op_b, b), (op_a, a)))
}

/// `n` with each filter no row passes made an empty VALUES; whether one
/// was, at `depth`.
fn emptied(n: &mut Node, depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    if let (Op::Filter, Body::Filter(c)) = (n.op, &n.body)
        && contradicts(c)
    {
        *n = Node { op: Op::Values, id: 0, kept: false, rows: 0.0, body: Body::Values(Vec::new()), inputs: Vec::new(), ..n.clone() };
        return true;
    }
    let mut any = false;
    for input in &mut n.inputs {
        any |= emptied(input, depth.saturating_add(1));
    }
    any
}

impl Rule for Contradiction {
    fn rewrite(&mut self, _cx: &mut Cx<'_>, form: &Form) -> Answer<Option<Form>> {
        let mut rewritten = form.clone();
        Ok(emptied(&mut rewritten.root, 0).then_some(rewritten))
    }
}

impl Rule for Broken {
    fn rewrite(&mut self, _cx: &mut Cx<'_>, form: &Form) -> Answer<Option<Form>> {
        if self.fails {
            return Err(PluginError::new("38000", "the rule fails"));
        }
        let none = Node {
            op: Op::Project,
            id: 0,
            kept: false,
            columns: Vec::new(),
            body: Body::Project(Vec::new()),
            inputs: vec![form.root.clone()],
            ..form.root.clone()
        };
        Ok(Some(Form { root: none }))
    }
}

impl Procedure for Skew {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()> {
        let [Value::Char(table), Value::Char(column)] = args else { return Err(PluginError::external("SKEW takes a table and a column")) };
        let known = cx.column_statistics(table, column)?;
        let first = known.as_ref().and_then(|t| t.columns.first());
        let distinct = first.map_or(-1.0_f64, |c| c.distinct);
        let rows = known.as_ref().map_or(-1.0_f64, |t| t.rows);
        cx.run("INSERT INTO SEEN VALUES (?, ?)", &[Value::Approx(rows), Value::Approx(distinct)])
    }
}
