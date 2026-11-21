//! \[DESIGN\] The plan form of a logical plan (RULES.md D275): each node
//! the form says whole made of its parts, the rest kept, with what the
//! planner knows of each node's columns and size. What the form keeps, of
//! nodes and expressions alike, is held here (`Kept`), so the plan of the
//! form rewritten (`back.rs`) takes it back unopened.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::estimate::{Sized, of_node, scanned};
use super::typed::{Typed, of_node as types_of, of_value, parsed, same, text};
use super::{Body, Column, Expression, Form, JoinKind, Key, MAX_DEPTH, Node, Op, Operator, SetKind};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, SetOp, cond_columns, expr_columns};
use crate::plugin::errors::violation;
use crate::value::{ArithOp, CompOp, DataType, Value};

/// A node of the planner's the form keeps: it with its inputs taken out,
/// its inputs' types and its own.
#[derive(Debug, Clone)]
pub(super) struct KeptNode {
    pub(super) node: LogicalPlan,
    pub(super) inputs: Vec<Vec<Typed>>,
    pub(super) types: Vec<Typed>,
}

/// An expression of the planner's the form shows opaque: a value's or a
/// condition's.
#[derive(Debug, Clone)]
pub(super) enum Held {
    Expr(Expr),
    Cond(Cond),
}

/// What a plan form keeps of the planner's plan.
#[derive(Debug, Clone, Default)]
pub struct Kept {
    /// The nodes kept, by their ids.
    pub(super) nodes: BTreeMap<u32, KeptNode>,
    /// The opaque expressions, each with the columns it reads, by their
    /// ids from 1.
    pub(super) exprs: Vec<(Held, Vec<usize>)>,
    /// The plan's output types.
    pub(super) root: Vec<Typed>,
}

/// The plan form of `plan`, and what it keeps of it.
///
/// # Errors
///
/// SQLSTATE 38T03 if the plan nests past `MAX_DEPTH`.
pub fn form_of(plan: &LogicalPlan, st: &State) -> Result<(Form, Kept)> {
    let mut making = Making { st, kept: Kept::default(), next: 1 };
    let (root, types, _) = making.node(plan, 0)?;
    making.kept.root = types;
    Ok((Form { root }, making.kept))
}

/// A form being made.
struct Making<'a> {
    st: &'a State,
    kept: Kept,
    next: u32,
}

/// The first line of `plan`'s EXPLAIN, its inputs left out.
fn label(plan: &LogicalPlan) -> String {
    plan.explain().lines().next().unwrap_or_default().trim().to_owned()
}

/// `plan`, its inputs taken out.
fn stripped(plan: &LogicalPlan) -> LogicalPlan {
    plan.clone().map_inputs(|_| LogicalPlan::Values { rows: Vec::new(), types: Vec::new() })
}

/// The operator of a node the form keeps.
fn kept_op(plan: &LogicalPlan) -> Op {
    match plan {
        LogicalPlan::Scan { .. } => Op::Scan,
        LogicalPlan::Filter { .. } => Op::Filter,
        LogicalPlan::Product { .. } | LogicalPlan::OuterJoin { .. } | LogicalPlan::SemiJoin { .. } => Op::Join,
        LogicalPlan::Aggregate { .. } => Op::Aggregate,
        LogicalPlan::Project { .. } => Op::Project,
        LogicalPlan::Distinct { .. } => Op::Distinct,
        LogicalPlan::SetOp { .. } => Op::Set,
        LogicalPlan::System { .. } => Op::System,
        LogicalPlan::Values { .. } => Op::Values,
        LogicalPlan::Sort { .. } => Op::Sort,
        LogicalPlan::Limit { .. } => Op::Limit,
        LogicalPlan::Recursive { .. } => Op::Recursive,
        LogicalPlan::WorkTable { .. } => Op::WorkTable,
        LogicalPlan::Transition { .. } => Op::Transition,
        LogicalPlan::Unnest(_) => Op::Unnest,
        LogicalPlan::Window(_) => Op::Window,
        LogicalPlan::Recognize(_) => Op::Recognize,
        LogicalPlan::Polymorphic(_) => Op::Polymorphic,
    }
}

/// Whether a node kept shows its inputs: all but a recursive query's
/// members and a polymorphic table function's tables, whose plans are
/// read by a fixpoint or its components alone.
fn opens_inputs(plan: &LogicalPlan) -> bool {
    !matches!(plan, LogicalPlan::Recursive { .. } | LogicalPlan::Polymorphic(_))
}

/// That `depth` is within `MAX_DEPTH`.
fn within(depth: usize) -> Result<usize> {
    if depth >= MAX_DEPTH { Err(violation(&format!("a plan nested deeper than {MAX_DEPTH}"))) } else { Ok(depth.saturating_add(1)) }
}

impl Making<'_> {
    /// The node of `plan`, its output types and its estimate.
    fn node(&mut self, plan: &LogicalPlan, depth: usize) -> Result<(Node, Vec<Typed>, Sized)> {
        let deeper = within(depth)?;
        let id = self.next;
        self.next = self.next.saturating_add(1);
        let body = self.body(plan, deeper)?;
        let (mut inputs, mut inner, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
        if body.is_some() || opens_inputs(plan) {
            for input in plan.inputs() {
                let (n, t, s) = self.node(input, deeper)?;
                inputs.push(n);
                inner.push(t);
                sizes.push(s);
            }
        }
        let types = types_of(plan, &inner, self.st);
        let size = if let LogicalPlan::Scan { table, names, .. } = plan {
            scanned(self.st, table, names.len()).0
        } else {
            of_node(self.st, plan, &sizes)
        };
        let kept = body.is_none();
        if kept {
            let node = if opens_inputs(plan) { stripped(plan) } else { plan.clone() };
            self.kept.nodes.insert(id, KeptNode { node, inputs: inner, types: types.clone() });
        }
        let names = plan.names();
        let columns = names.into_iter().zip(&types).map(|(name, t)| Column { name, data_type: text(t) }).collect();
        let (op, body) = match body {
            Some(made) => made,
            None => (kept_op(plan), Body::Kept(label(plan))),
        };
        Ok((Node { op, id, kept, columns, rows: size.rows, width: size.width, body, inputs }, types, size))
    }

    /// The operator and body of `plan`, if the form says it whole.
    fn body(&mut self, plan: &LogicalPlan, depth: usize) -> Result<Option<(Op, Body)>> {
        Ok(Some(match plan {
            LogicalPlan::Scan { table, names, located: false, .. } => {
                let statistics = scanned(self.st, table, names.len()).1;
                (Op::Scan, Body::Scan { table: table.to_string(), statistics })
            }
            LogicalPlan::Filter { cond, .. } => (Op::Filter, Body::Filter(self.cond(cond, depth)?)),
            LogicalPlan::Project { exprs, .. } => (Op::Project, Body::Project(self.exprs(exprs, depth)?)),
            LogicalPlan::Product { .. } => (Op::Join, Body::Join { kind: JoinKind::Cross, condition: None }),
            LogicalPlan::OuterJoin { kind, cond, .. } => {
                let kind = match kind {
                    OuterJoin::Left => JoinKind::Left,
                    OuterJoin::Right => JoinKind::Right,
                    OuterJoin::Full => JoinKind::Full,
                };
                (Op::Join, Body::Join { kind, condition: Some(self.cond(cond, depth)?) })
            }
            LogicalPlan::SemiJoin { anti, cond, .. } => {
                let kind = if *anti { JoinKind::Anti } else { JoinKind::Semi };
                (Op::Join, Body::Join { kind, condition: Some(self.cond(cond, depth)?) })
            }
            LogicalPlan::Sort { keys, .. } => {
                let keys = keys.iter().map(|k| Key { column: column(k.column), descending: k.descending, nulls_first: k.nulls_first });
                (Op::Sort, Body::Sort(keys.collect()))
            }
            LogicalPlan::Distinct { .. } => (Op::Distinct, Body::Distinct),
            LogicalPlan::SetOp { op, all, compared: None, .. } => (Op::Set, Body::Set { op: set_kind(*op), all: *all }),
            LogicalPlan::Values { rows, .. } => {
                (Op::Values, Body::Values(rows.iter().map(|row| self.exprs(row, depth)).collect::<Result<_>>()?))
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(_)
            | LogicalPlan::Window(_)
            | LogicalPlan::Recognize(_)
            | LogicalPlan::Polymorphic(_) => return Ok(None),
        }))
    }

    /// The forms of `all`.
    fn exprs(&mut self, all: &[Expr], depth: usize) -> Result<Vec<Expression>> {
        all.iter().map(|e| self.expr(e, depth)).collect()
    }

    /// `held` shown opaque, over the columns it reads.
    fn opaque(&mut self, held: Held) -> Expression {
        let mut columns = Vec::new();
        match &held {
            Held::Expr(e) => expr_columns(e, &mut columns),
            Held::Cond(c) => cond_columns(c, &mut columns),
        }
        columns.sort_unstable();
        columns.dedup();
        let shown = columns.iter().map(|c| column(*c)).collect();
        self.kept.exprs.push((held, columns));
        Expression::Opaque { id: column(self.kept.exprs.len()), columns: shown }
    }

    /// The form of `e`.
    fn expr(&mut self, e: &Expr, depth: usize) -> Result<Expression> {
        let deeper = within(depth)?;
        let operator = |op, args| Expression::Operator { op, args };
        Ok(match e {
            Expr::Column(i) => Expression::Column(column(*i)),
            Expr::Const(v) => {
                if !literal(v) {
                    return Ok(self.opaque(Held::Expr(e.clone())));
                }
                Expression::Literal { data_type: text(&of_value(v)), value: v.clone() }
            }
            Expr::Neg(x) => operator(Operator::Negate, vec![self.expr(x, deeper)?]),
            Expr::Arith(a, op, b) => operator(arith(*op), vec![self.expr(a, deeper)?, self.expr(b, deeper)?]),
            Expr::Call(f, args) => {
                if !self.named(f) {
                    return Ok(self.opaque(Held::Expr(e.clone())));
                }
                Expression::Call { name: f.0.name().to_owned(), args: self.exprs(args, deeper)? }
            }
            Expr::Cast(x, t) => {
                if !says(t) {
                    return Ok(self.opaque(Held::Expr(e.clone())));
                }
                Expression::Cast { data_type: t.to_string(), arg: Box::new(self.expr(x, deeper)?) }
            }
            Expr::Truth(c) => self.cond(c, deeper)?,
            Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Subquery(..)
            | Expr::NextValue(_)
            | Expr::Invoked(_)
            | Expr::Warned(..) => self.opaque(Held::Expr(e.clone())),
        })
    }

    /// Whether `f` is the one function of its name, so a CALL of its name
    /// finds it again.
    fn named(&self, f: &crate::functions::ScalarRef) -> bool {
        let all = self.st.functions.scalars(f.0.name());
        if let [one] = all.as_slice() { std::ptr::addr_eq(Arc::as_ptr(&one.0), Arc::as_ptr(&f.0)) } else { false }
    }

    /// The form of `c`.
    fn cond(&mut self, c: &Cond, depth: usize) -> Result<Expression> {
        let deeper = within(depth)?;
        let operator = |op, args| Expression::Operator { op, args };
        Ok(match c {
            Cond::Const(t) => Expression::Literal { data_type: DataType::Boolean.to_string(), value: t.value() },
            Cond::And(a, b) => operator(Operator::And, vec![self.cond(a, deeper)?, self.cond(b, deeper)?]),
            Cond::Or(a, b) => operator(Operator::Or, vec![self.cond(a, deeper)?, self.cond(b, deeper)?]),
            Cond::Not(a) => operator(Operator::Not, vec![self.cond(a, deeper)?]),
            Cond::Compare(a, op, b) => operator(compared(*op), vec![self.expr(a, deeper)?, self.expr(b, deeper)?]),
            Cond::IsNull(e, not) => operator(if *not { Operator::IsNotNull } else { Operator::IsNull }, vec![self.expr(e, deeper)?]),
            Cond::Value(e) => self.expr(e, deeper)?,
            Cond::Distinct(..)
            | Cond::Like(..)
            | Cond::Exists(_)
            | Cond::Quantified(..)
            | Cond::Is(..)
            | Cond::Unique(..)
            | Cond::Match { .. }
            | Cond::In(..) => self.opaque(Held::Cond(c.clone())),
        })
    }
}

/// `i`, as the form numbers columns and ids.
fn column(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// Whether `v` is a literal the form says: the null value, or a value of
/// a type it writes, which protocol 1 carries as itself.
fn literal(v: &Value) -> bool {
    v.is_null() || of_value(v).is_some()
}

/// Whether the form says CAST to `t`: its text read again is it.
fn says(t: &DataType) -> bool {
    let typed: Typed = Some(t.clone());
    parsed(&t.to_string()).is_ok_and(|back| same(&back, &typed))
}

/// The operator of `op`.
fn arith(op: ArithOp) -> Operator {
    match op {
        ArithOp::Add => Operator::Add,
        ArithOp::Sub => Operator::Sub,
        ArithOp::Mul => Operator::Mul,
        ArithOp::Div => Operator::Div,
    }
}

/// The operator of `op`.
fn compared(op: CompOp) -> Operator {
    match op {
        CompOp::Eq => Operator::Eq,
        CompOp::Ne => Operator::Ne,
        CompOp::Lt => Operator::Lt,
        CompOp::Gt => Operator::Gt,
        CompOp::Le => Operator::Le,
        CompOp::Ge => Operator::Ge,
    }
}

/// The set operator of `op`.
fn set_kind(op: SetOp) -> SetKind {
    match op {
        SetOp::Union => SetKind::Union,
        SetOp::Except => SetKind::Except,
        SetOp::Intersect => SetKind::Intersect,
    }
}
