//! \[DESIGN\] The logical plan of a plan form a rule gave back (RULES.md
//! D277), checked whole as it is made: each node's inputs as many as its
//! operator takes, each column it names one its input has, each table
//! one the catalog holds, each kept node and opaque expression one the
//! form kept, used once, a kept node's inputs keeping their columns'
//! types, and the plan's output keeping the columns and types it had.
//! What fails is refused (SQLSTATE 38T03), and the rule is skipped.

use std::collections::HashSet;

use super::from::Kept;
use super::typed::{Typed, of_node, parsed, same, text};
use super::{Body, Form, JoinKind, MAX_DEPTH, Node, Op, SetKind};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::logical::{LogicalPlan, OuterJoin, SetOp, SortKey};
use crate::plugin::errors::violation;

/// A rule's plan refused for `why`.
pub(super) fn refused(why: &str) -> SqlError {
    violation(&format!("its plan is not valid: {why}"))
}

/// The logical plan of `form`, which a rule gave for the plan `kept` was
/// kept of, checked.
///
/// # Errors
///
/// SQLSTATE 38T03 if it is not valid, or does not keep the plan's output
/// columns and their types.
pub fn plan_of(form: &Form, kept: &Kept, st: &State) -> Result<LogicalPlan> {
    let mut b = Building { st, kept, nodes: HashSet::new(), exprs: HashSet::new() };
    let (plan, types) = b.node(&form.root, 0)?;
    if types.len() != kept.root.len() {
        return Err(refused(&format!("it gives {} columns, not {}", types.len(), kept.root.len())));
    }
    for (at, (new, old)) in types.iter().zip(&kept.root).enumerate() {
        if old.is_some() && !same(new, old) {
            return Err(refused(&format!("its column {at} is of type {}, not {}", text(new), text(old))));
        }
    }
    Ok(plan)
}

/// A plan being made of a form.
pub(super) struct Building<'a> {
    pub(super) st: &'a State,
    pub(super) kept: &'a Kept,
    /// The kept nodes used.
    nodes: HashSet<u32>,
    /// The opaque expressions used.
    pub(super) exprs: HashSet<u32>,
}

/// That `depth` is within `MAX_DEPTH`.
pub(super) fn within(depth: usize) -> Result<usize> {
    if depth >= MAX_DEPTH { Err(refused(&format!("it nests deeper than {MAX_DEPTH}"))) } else { Ok(depth.saturating_add(1)) }
}

/// That a node of `op` has `n` inputs, `wanted` wanted.
fn inputs(op: Op, n: usize, wanted: usize) -> Result<()> {
    if n == wanted { Ok(()) } else { Err(refused(&format!("a node of operator {} has {n} inputs, not {wanted}", op.code()))) }
}

/// That `column` is one of `width`.
pub(super) fn has(column: u32, width: usize) -> Result<usize> {
    usize::try_from(column).ok().filter(|c| *c < width).ok_or_else(|| refused(&format!("column {column} is not one of {width}")))
}

impl Building<'_> {
    /// The plan of `n`, and its output types.
    fn node(&mut self, n: &Node, depth: usize) -> Result<(LogicalPlan, Vec<Typed>)> {
        let deeper = within(depth)?;
        let mut built = Vec::with_capacity(n.inputs.len());
        for input in &n.inputs {
            built.push(self.node(input, deeper)?);
        }
        if n.kept {
            return self.kept_node(n, built);
        }
        let inner: Vec<Vec<Typed>> = built.iter().map(|(_, t)| t.clone()).collect();
        let mut plans = built.into_iter().map(|(p, _)| Box::new(p));
        let count = n.inputs.len();
        let plan = match (&n.body, n.op) {
            (Body::Scan { table, .. }, Op::Scan) => {
                inputs(n.op, count, 0)?;
                self.scan(table)?
            }
            (Body::Join { kind, condition }, Op::Join) => {
                inputs(n.op, count, 2)?;
                let (left, right) = (plans.next(), plans.next());
                self.join((*kind, condition.as_ref()), &inner.concat(), (left, right), deeper)?
            }
            (Body::Set { op, all }, Op::Set) => {
                inputs(n.op, count, 2)?;
                set_types(&inner)?;
                let op = match op {
                    SetKind::Union => SetOp::Union,
                    SetKind::Except => SetOp::Except,
                    SetKind::Intersect => SetOp::Intersect,
                };
                let (left, right) = (plans.next().ok_or_else(|| refused("no left"))?, plans.next().ok_or_else(|| refused("no right"))?);
                LogicalPlan::SetOp { op, all: *all, compared: None, left, right }
            }
            (Body::Values(rows), Op::Values) => {
                inputs(n.op, count, 0)?;
                self.values(n, rows, deeper)?
            }
            (body, op) => {
                inputs(op, count, 1)?;
                let input = plans.next().ok_or_else(|| refused("no input"))?;
                self.over((body, op), input, &inner.concat(), deeper)?
            }
        };
        let types = of_node(&plan, &inner, self.st);
        Ok((plan, types))
    }

    /// The plan of a node of one input, `input`, of `types`.
    fn over(&mut self, (body, op): (&Body, Op), input: Box<LogicalPlan>, types: &[Typed], depth: usize) -> Result<LogicalPlan> {
        Ok(match (body, op) {
            (Body::Filter(c), Op::Filter) => LogicalPlan::Filter { cond: self.cond_of(c, types, depth)?, input },
            (Body::Project(all), Op::Project) => {
                let exprs = all.iter().map(|e| self.expr_of(e, types, depth)).collect::<Result<_>>()?;
                LogicalPlan::Project { exprs, input }
            }
            (Body::Sort(keys), Op::Sort) => {
                let keys = keys
                    .iter()
                    .map(|k| Ok(SortKey { column: has(k.column, types.len())?, descending: k.descending, nulls_first: k.nulls_first }))
                    .collect::<Result<_>>()?;
                LogicalPlan::Sort { keys, input }
            }
            (Body::Distinct, Op::Distinct) => LogicalPlan::Distinct { input },
            (_, op) => return Err(refused(&format!("a node of operator {} has a body of another", op.code()))),
        })
    }

    /// The node kept as `n` names, over `built`, its inputs.
    fn kept_node(&mut self, n: &Node, built: Vec<(LogicalPlan, Vec<Typed>)>) -> Result<(LogicalPlan, Vec<Typed>)> {
        let k = self.kept.nodes.get(&n.id).ok_or_else(|| refused(&format!("it keeps node {}, which it was not given", n.id)))?;
        if !self.nodes.insert(n.id) {
            return Err(refused(&format!("it keeps node {} twice", n.id)));
        }
        inputs(n.op, built.len(), k.inputs.len())?;
        for (at, ((_, got), had)) in built.iter().zip(&k.inputs).enumerate() {
            let alike = got.len() == had.len() && got.iter().zip(had).all(|(a, b)| same(a, b));
            if !alike {
                return Err(refused(&format!("the input {at} of node {} has columns of other types", n.id)));
            }
        }
        let mut plan = k.node.clone();
        if !k.inputs.is_empty() {
            let mut given = built.into_iter().map(|(p, _)| p);
            plan.for_each_input_mut(&mut |slot| {
                if let Some(p) = given.next() {
                    *slot = p;
                }
            });
        }
        Ok((plan, k.types.clone()))
    }

    /// A scan of `table`, "SCHEMA.TABLE", every column needed.
    fn scan(&self, table: &str) -> Result<LogicalPlan> {
        let (auth, ident) = table.split_once('.').ok_or_else(|| refused(&format!("{table} names no table")))?;
        let name = QualifiedName { auth: auth.to_owned(), ident: ident.to_owned() };
        let desc = self.st.catalog.table(&name).ok_or_else(|| refused(&format!("there is no table {table}")))?;
        let names: Vec<String> = desc.columns.iter().map(|c| c.name.clone()).collect();
        Ok(LogicalPlan::Scan { needed: (0..names.len()).collect(), names, table: name, located: false })
    }

    /// A join of `kind`, an outer one on `condition`, over `types`.
    fn join(
        &mut self,
        (kind, condition): (JoinKind, Option<&super::Expression>),
        types: &[Typed],
        (left, right): (Option<Box<LogicalPlan>>, Option<Box<LogicalPlan>>),
        depth: usize,
    ) -> Result<LogicalPlan> {
        let (left, right) = (left.ok_or_else(|| refused("no left"))?, right.ok_or_else(|| refused("no right"))?);
        let kind = match (kind, condition) {
            (JoinKind::Cross, None) => return Ok(LogicalPlan::Product { left, right }),
            (JoinKind::Left, Some(_)) => OuterJoin::Left,
            (JoinKind::Right, Some(_)) => OuterJoin::Right,
            (JoinKind::Full, Some(_)) => OuterJoin::Full,
            (JoinKind::Cross, Some(_)) | (JoinKind::Left | JoinKind::Right | JoinKind::Full, None) => {
                return Err(refused("a cross join has no condition, and an outer join one"));
            }
        };
        let cond = self.cond_of(condition.ok_or_else(|| refused("no condition"))?, types, depth)?;
        Ok(LogicalPlan::OuterJoin { kind, cond, left, right })
    }

    /// The rows of a VALUES node `n`, of the types its columns say.
    fn values(&mut self, n: &Node, rows: &[Vec<super::Expression>], depth: usize) -> Result<LogicalPlan> {
        let types = n
            .columns
            .iter()
            .map(|c| parsed(&c.data_type)?.ok_or_else(|| refused("a column of VALUES has no type")))
            .collect::<Result<Vec<_>>>()?;
        let mut made = Vec::with_capacity(rows.len());
        for row in rows {
            if row.len() != types.len() {
                return Err(refused(&format!("a row of VALUES has {} values, not {}", row.len(), types.len())));
            }
            made.push(row.iter().map(|e| self.expr_of(e, &[], depth)).collect::<Result<Vec<_>>>()?);
        }
        Ok(LogicalPlan::Values { rows: made, types })
    }
}

/// That a set operator's operands are of one degree and comparable types.
fn set_types(inner: &[Vec<Typed>]) -> Result<()> {
    let (left, right) = (inner.first().cloned().unwrap_or_default(), inner.get(1).cloned().unwrap_or_default());
    if left.len() != right.len() {
        return Err(refused("the operands of a set operator are of other degrees"));
    }
    let clash = left.iter().zip(&right).any(|(a, b)| a.as_ref().zip(b.as_ref()).is_some_and(|(a, b)| !a.comparable(b)));
    if clash { Err(refused("the operands of a set operator are of types not comparable")) } else { Ok(()) }
}
