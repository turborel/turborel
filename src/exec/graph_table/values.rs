//! \[EXTENSION\] The rows a GRAPH_TABLE's match gives [GS §8]: its match
//! row, each singleton's columns of the element bound to it; its slots,
//! each set function over a group variable's elements in the order they
//! are bound [P1 §4.4] and each path function of a path [GS §6.9]; then,
//! if its postfilter is true [R6.4], its COLUMNS of the match, of each
//! vertex of its paths, or of each step.

use super::shared::Shared;
use crate::error::Result;
use crate::error::SqlState;
use crate::exec::{Runner, eval, eval_cond, warn};
use crate::graph::binding::PathBinding;
use crate::graph::view::ElementId;
use crate::json::Item;
use crate::logical::{AggCall, GraphValue, PathFunction, RowsShape, Truth, identity_item};
use crate::storage::Row;
use crate::value::json::Json;
use crate::value::{Decimal, Value};

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
            let value = match slot {
                GraphValue::Aggregate { call, var } => self.aggregate(call, *var, (chosen, &row), runner)?,
                GraphValue::Path { path, function } => match chosen.get(*path) {
                    Some(b) => self.path_value(b, *function)?,
                    None => Value::Null,
                },
            };
            if let Some(cell) = row.get_mut(self.graph.width.saturating_add(k)) {
                *cell = value;
            }
        }
        Ok(row)
    }

    /// Set function `call` over the elements of group variable `var` in
    /// `chosen`, each in the columns of `row`.
    fn aggregate(&self, call: &AggCall, var: usize, (chosen, row): (&Chosen<'_>, &Row), runner: &Runner) -> Result<Value> {
        let mut acc = call.func.0.accumulator(call.result.clone());
        let mut seen: Vec<String> = Vec::new();
        let shape = self.graph.vars.get(var);
        let elements: Vec<ElementId> = shape
            .into_iter()
            .flat_map(|s| &s.bound)
            .flat_map(|&(path, id)| chosen.get(path).map(|b| b.group(id)).unwrap_or_default())
            .collect();
        for element in elements {
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
            acc.update(&v)?;
        }
        acc.finish()
    }

    /// The identities of `elements`, as an array.
    fn identities(&self, elements: &[ElementId]) -> Result<Value> {
        let ids = elements.iter().map(|&e| self.identity(e)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Array(ids.into()))
    }

    /// \[GS §6.9\] Path function `f` of path `b`.
    fn path_value(&self, b: &PathBinding, f: PathFunction) -> Result<Value> {
        Ok(match f {
            PathFunction::Length => Value::Exact(Decimal::new(i128::try_from(b.length()).unwrap_or(i128::MAX), 0)),
            PathFunction::Vertices => self.identities(&b.vertices())?,
            PathFunction::Edges => self.identities(&b.edges())?,
            PathFunction::Elements => self.identities(&b.elements)?,
            PathFunction::Acyclic => Value::Bool(b.is_acyclic()),
            PathFunction::Trail => Value::Bool(b.is_trail()),
            PathFunction::Simple => Value::Bool(b.is_simple()),
            PathFunction::Json => Value::Json(Json::of_item(&self.path_item(b)?)?),
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
