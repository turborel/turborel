//! \[DESIGN\] The parts of an expression no node takes, evaluated a row at
//! a time (RULES.md D362): each row of the rows given made of its
//! program's inputs, and evaluated as a row's projection evaluates it
//! (`exec::eval`, `eval_cond`), in a statement that hears its warnings
//! apart (`Quiet`). A row that raises an error, or any warning, gives
//! none, so the batch is evaluated a row at a time whole, its warnings
//! raised in the order a row at a time raises them.

use std::sync::Arc;

use super::Cx;
use super::vector::{Vector, values};
use crate::catalog::SequenceDesc;
use crate::db::State;
use crate::error::{Result, Warnings};
use crate::exec::{Subqueries, SubqueryRows, eval, eval_cond};
use crate::fulltext::rank::Statistics;
use crate::logical::{Cond, Expr, Invocation, Subquery, Truth};
use crate::storage::{Bools, Column, Mask};
use crate::value::Value;

/// \[DESIGN\] A statement's evaluation whose warnings are heard apart:
/// everything else its own.
struct Quiet<'s> {
    statement: &'s dyn Subqueries,
    heard: Warnings,
}

impl Subqueries for Quiet<'_> {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        self.statement.rows(subquery, outer, limit)
    }

    fn slot(&self, index: usize) -> Option<&Value> {
        self.statement.slot(index)
    }

    fn outer(&self, index: usize) -> Option<&Value> {
        self.statement.outer(index)
    }

    fn warnings(&self) -> Option<&Warnings> {
        Some(&self.heard)
    }
    fn vector_kernels(&self) -> &dyn crate::kernels::Kernels {
        self.statement.vector_kernels()
    }

    fn like(&self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        self.statement.like(value, pattern, escape)
    }

    fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        self.statement.next_value(seq)
    }

    fn interrupted(&self) -> Result<()> {
        self.statement.interrupted()
    }

    fn debugged(&self) -> Option<Arc<crate::debugger::Link>> {
        self.statement.debugged()
    }

    fn state(&self) -> Option<&Arc<State>> {
        self.statement.state()
    }

    fn ranking_statistics(&self, index: &str) -> Result<Option<Arc<dyn Statistics>>> {
        self.statement.ranking_statistics(index)
    }

    fn temp_lobs(&self) -> Option<&dyn crate::value::lob::TempLobs> {
        self.statement.temp_lobs()
    }

    fn invoke_hosted(&self, i: &Invocation, values: Vec<Value>) -> Option<Result<Option<Value>>> {
        self.statement.invoke_hosted(i, values)
    }
}

/// Each row of `cx`, a row of its inputs' values, given to `f` in
/// `statement`, in order: none if one fails or raises a warning.
fn each<T>(cx: &Cx<'_>, f: impl Fn(&Vec<Value>, &dyn Subqueries) -> Result<T>) -> Option<Vec<T>> {
    let quiet = Quiet { statement: cx.statement, heard: Warnings::default() };
    let inputs: Vec<&[Value]> = cx.inputs.iter().map(|c| c.values()).collect();
    let mut row = vec![Value::Null; inputs.len()];
    let mut made = Vec::with_capacity(cx.count);
    for n in 0..cx.count {
        for (slot, values) in row.iter_mut().zip(&inputs) {
            slot.clone_from(values.get(n).unwrap_or(&Value::Null));
        }
        made.push(f(&row, &quiet).ok()?);
    }
    (!quiet.heard.raised()).then_some(made)
}

/// The values of `e` of each row of `cx`, as `each` gives them.
pub(super) fn values_of(e: &Expr, cx: &Cx<'_>) -> Option<Vector<'static>> {
    each(cx, |row, statement| eval(e, row, statement)).map(values)
}

/// The truth values of `c` of each row of `cx`, as `each` gives them.
pub(super) fn truths_of(c: &Cond, cx: &Cx<'_>) -> Option<Vector<'static>> {
    let made = each(cx, |row, statement| eval_cond(c, row, statement))?;
    let (mut bits, mut unknown) = (Mask::none(cx.count), Mask::none(cx.count));
    for (n, t) in made.iter().enumerate() {
        match t {
            Truth::True => bits.set(n),
            Truth::Unknown => unknown.set(n),
            Truth::False => {}
        }
    }
    Some(Vector::Owned(Column::Bools(Bools::new(bits, unknown.any().then_some(unknown)))))
}
