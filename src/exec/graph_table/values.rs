//! \[EXTENSION\] The rows a GRAPH_TABLE's match gives [GS §8]: its match
//! row, each singleton's columns of the element bound to it; its slots,
//! each set function over a group variable's elements in the order they
//! are bound [P1 §4.4] and each path function of a path [GS §6.9]; then,
//! if its postfilter is true [R6.4], its COLUMNS of the match, of each
//! vertex of its paths, or of each step.

use std::cmp::Ordering;

use super::shared::Shared;
use crate::error::Result;
use crate::error::SqlState;
use crate::exec::{Runner, eval, eval_cond, warn};
use crate::functions::{picked, picked_value};
use crate::graph::binding::PathBinding;
use crate::graph::view::ElementId;
use crate::json::Item;
use crate::logical::{AggCall, Distribution, GraphValue, PathFunction, RowsShape, Truth, identity_item};
use crate::storage::Row;
use crate::value::json::Json;
use crate::value::{DataType, Decimal, Value, compare};

/// The bindings of a match: one of each path pattern.
pub(super) type Chosen<'b> = Vec<&'b PathBinding>;

impl Shared {
    /// The match row of `chosen`: its singletons and slots.
    ///
    /// # Errors
    ///
    /// Any error reading an element or computing a slot.
    pub(super) fn match_row(&self, chosen: &Chosen<'_>, runner: &Runner) -> Result<Row> {
        let mut row = self.blank();
        for (v, shape) in self.graph.vars.iter().enumerate() {
            let bound = shape.bound.iter().find_map(|&(path, id)| chosen.get(path).and_then(|b| b.singleton(id)));
            if let Some(element) = bound {
                self.fill(&mut row, v, element)?;
            }
        }
        for (k, slot) in self.graph.slots.iter().enumerate() {
            if self.graph.prefilters.iter().any(|p| p.slots.iter().any(|s| s.0 == k)) {
                continue;
            }
            let value = match slot {
                GraphValue::Aggregate { call, var } => {
                    let shape = self.graph.vars.get(*var);
                    let bound = shape.into_iter().flat_map(|s| &s.bound);
                    let elements: Vec<ElementId> =
                        bound.flat_map(|&(path, id)| chosen.get(path).map(|b| b.group(id)).unwrap_or_default()).collect();
                    self.aggregate(call, (*var, &elements), &row, runner)?
                }
                GraphValue::Path { path, function, span } => {
                    let whole = chosen.get(*path).copied();
                    let part = span.and_then(|(start, end)| whole.and_then(|b| b.span(start, end)));
                    match (span, whole, &part) {
                        // EXISTS IN PATH's prefilters read the match's other bindings.
                        (None, Some(b), _) | (Some(_), _, Some(b)) => match function {
                            PathFunction::Within(_) => {
                                self.with_fixed(chosen.iter().map(|c| Some((*c).clone())).collect()).path_value(b, *function)?
                            }
                            PathFunction::Length
                            | PathFunction::Vertices
                            | PathFunction::Edges
                            | PathFunction::Elements
                            | PathFunction::Acyclic
                            | PathFunction::Trail
                            | PathFunction::Simple
                            | PathFunction::Json => self.path_value(b, *function)?,
                        },
                        (None | Some(_), _, _) => Value::Null,
                    }
                }
                GraphValue::Iteration { .. } => Value::Null,
            };
            if let Some(cell) = row.get_mut(self.graph.width.saturating_add(k)) {
                *cell = value;
            }
        }
        Ok(row)
    }

    /// Set function `call` over `elements`, those group variable `var` is
    /// bound to, each in the columns of `row`.
    pub(super) fn aggregate(&self, call: &AggCall, (var, elements): (usize, &[ElementId]), row: &Row, runner: &Runner) -> Result<Value> {
        let mut acc = call.func.0.accumulator(call.result.clone());
        let mut seen: Vec<String> = Vec::new();
        let mut sorted: Vec<Value> = Vec::new();
        for &element in elements {
            let mut each = row.clone();
            self.fill(&mut each, var, element)?;
            if let Some(c) = &call.filter
                && eval_cond(c, &each, runner)? != Truth::True
            {
                continue;
            }
            let Some(e) = &call.arg else {
                acc.update(&Value::Null)?;
                continue;
            };
            let v = eval(e, &each, runner)?;
            if v.is_null() {
                warn(runner, SqlState::NULL_VALUE_ELIMINATED);
                continue;
            }
            if call.distinct {
                let text = super::store::key_text(std::slice::from_ref(&v)).unwrap_or_default();
                if seen.contains(&text) {
                    continue;
                }
                seen.push(text);
            }
            if call.distribution.is_some() {
                sorted.push(v);
                continue;
            }
            acc.update(&v)?;
        }
        let Some(d) = &call.distribution else { return acc.finish() };
        distributed(d, (sorted, &call.result), row, runner)
    }

    /// \[GS §11.1\] Whether every FOR EACH SEGMENT of the match `chosen`
    /// holds: its condition true of each two edges one after the other
    /// in its path, of the match row `row` and the two bound to its
    /// variables.
    ///
    /// # Errors
    ///
    /// Any error reading an element or evaluating a condition.
    pub(super) fn segments_hold(&self, chosen: &Chosen<'_>, row: &Row, runner: &Runner) -> Result<bool> {
        for s in &self.graph.segments {
            let whole = chosen.get(s.path).copied();
            let part = s.span.and_then(|(start, end)| whole.and_then(|b| b.span(start, end)));
            let path = match s.span {
                Some(_) => part.as_ref(),
                None => whole,
            };
            let edges = path.map(PathBinding::edges).unwrap_or_default();
            for pair in edges.windows(2) {
                let (Some(&first), Some(&second)) = (pair.first(), pair.get(1)) else { continue };
                let mut each = row.clone();
                self.fill(&mut each, s.first, first)?;
                self.fill(&mut each, s.second, second)?;
                if eval_cond(&s.cond, &each, runner)? != Truth::True {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// The identities of `elements`, as an array.
    fn identities(&self, elements: &[ElementId]) -> Result<Value> {
        let ids = elements.iter().map(|&e| self.identity(e)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Array(ids.into()))
    }

    /// \[GS §6.9\] Path function `f` of path `b`.
    pub(super) fn path_value(&self, b: &PathBinding, f: PathFunction) -> Result<Value> {
        Ok(match f {
            PathFunction::Length => Value::Exact(Decimal::new(i128::try_from(b.length()).unwrap_or(i128::MAX), 0)),
            PathFunction::Vertices => self.identities(&b.vertices())?,
            PathFunction::Edges => self.identities(&b.edges())?,
            PathFunction::Elements => self.identities(&b.elements)?,
            PathFunction::Acyclic => Value::Bool(b.is_acyclic()),
            PathFunction::Trail => Value::Bool(b.is_trail()),
            PathFunction::Simple => Value::Bool(b.is_simple()),
            PathFunction::Json => Value::Json(Json::of_item(&self.path_item(b)?)?),
            PathFunction::Within(k) => Value::Bool(self.within(k, b)?),
        })
    }

    /// PATH_TO_JSON of `b`: the array of its elements' identities, each
    /// `{"table":...,"key":[...]}`, in path order.
    fn path_item(&self, b: &PathBinding) -> Result<Item> {
        let mut items = Vec::new();
        for &e in &b.elements {
            let Some(source) = self.graph.tables.get(usize::try_from(e.table).unwrap_or(usize::MAX)) else { continue };
            let Some(row) = self.store.row(e)? else { continue };
            let key: Vec<Value> = source.key.iter().map(|&k| row.get(k).cloned().unwrap_or(Value::Null)).collect();
            items.push(identity_item(&source.alias, &key));
        }
        Ok(Item::Array(items))
    }

    /// The values of COLUMNS of each row match row `row` of `chosen` gives
    /// [GS §8 R8.3], pushed onto `out`.
    ///
    /// # Errors
    ///
    /// Any error evaluating a column.
    pub(super) fn columns_of(&self, chosen: &Chosen<'_>, row: &Row, runner: &Runner, out: &mut Vec<Vec<Value>>) -> Result<()> {
        let columns = |r: &Row| self.graph.columns.iter().map(|e| eval(e, r, runner)).collect::<Result<Vec<_>>>();
        match self.graph.rows {
            RowsShape::Match => out.push(columns(row)?),
            RowsShape::Vertex(v) => {
                for b in chosen {
                    for vertex in b.vertices() {
                        let mut each = row.clone();
                        self.fill(&mut each, v, vertex)?;
                        out.push(columns(&each)?);
                    }
                }
            }
            RowsShape::Step(a, e, z) => {
                for step in chosen.iter().flat_map(|b| steps(b)) {
                    let mut each = row.clone();
                    for (var, element) in [(a, Some(step.0)), (e, step.1), (z, step.2)].into_iter().filter_map(|(v, x)| Some((v, x?))) {
                        self.fill(&mut each, var, element)?;
                    }
                    out.push(columns(&each)?);
                }
            }
        }
        Ok(())
    }
}

/// \[GS §8 R8.3\] The steps of `b`: each edge with the vertices before and
/// after it; of a path of no edge, its vertex alone.
fn steps(b: &PathBinding) -> Vec<(ElementId, Option<ElementId>, Option<ElementId>)> {
    let vertices = b.vertices();
    let edges = b.edges();
    if edges.is_empty() {
        return vertices.first().map(|&v| (v, None, None)).into_iter().collect();
    }
    edges.iter().enumerate().filter_map(|(i, &e)| Some((*vertices.get(i)?, Some(e), vertices.get(i.saturating_add(1)).copied()))).collect()
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10] The value inverse distribution function
/// `d` of result type `result` picks of `values`, its fraction over
/// `row`: null of no value; SQLSTATE 22003 of a fraction not from 0 to 1.
fn distributed(d: &Distribution, (mut values, result): (Vec<Value>, &DataType), row: &Row, runner: &Runner) -> Result<Value> {
    let fraction = eval(&d.fraction, row, runner)?;
    let n = u64::try_from(values.len()).unwrap_or(u64::MAX);
    let Some(chosen) = picked(d.continuous, &fraction, n)? else { return Ok(Value::Null) };
    values.sort_by(|a, b| compare(a, b).unwrap_or(Ordering::Equal));
    if d.order.descending {
        values.reverse();
    }
    let at = |k: u64| usize::try_from(k).ok().and_then(|k| values.get(k)).cloned();
    let (Some(low), Some(high)) = (at(chosen.low), at(chosen.high)) else { return Ok(Value::Null) };
    picked_value(d.continuous, (low, high), chosen.factor, result)
}
