//! \[EXTENSION\] What a GRAPH_TABLE's search shares with its prefilters
//! and costs, which path search may call from its workers: the graph, its
//! operands' values for the input row, and how an element variable's
//! columns of the match row are made of the element bound to it.

use std::sync::Arc;

use super::store::Store;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::{Runner, eval, eval_cond};
use crate::graph::binding::PathBinding;
use crate::graph::program::{Bound, CostFn, Filter, Predicate, VarId};
use crate::graph::view::{ElementId, GraphView};
use crate::logical::{EDGE_COLUMNS, GraphMatch, GraphValue, Truth, element_text};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// The hidden column of an edge's source, after its identity and table.
const SOURCE_AT: usize = 2;
/// The hidden column of an edge's destination.
const DESTINATION_AT: usize = 3;

/// \[EXTENSION\] A GRAPH_TABLE's search of one input row.
pub(super) struct Shared {
    pub(super) db: Db,
    pub(super) tx: TxId,
    pub(super) graph: Arc<GraphMatch>,
    pub(super) store: Arc<Store>,
    /// The operands' values.
    pub(super) operands: Vec<Value>,
    /// Of each path pattern another's prefilters read, its binding they
    /// are searched with (`graph_table/correlated.rs`).
    pub(super) fixed: Vec<Option<PathBinding>>,
}

impl Shared {
    /// It with `fixed` the bindings of other path patterns read.
    pub(super) fn with_fixed(&self, fixed: Vec<Option<PathBinding>>) -> Shared {
        let (db, graph, store) = (self.db.clone(), Arc::clone(&self.graph), Arc::clone(&self.store));
        Shared { db, tx: self.tx, graph, store, operands: self.operands.clone(), fixed }
    }

    /// The fixed binding of a path pattern element variable `var` is of,
    /// and its number there.
    fn fixed_of(&self, var: usize) -> Option<(&PathBinding, VarId)> {
        let shape = self.graph.vars.get(var)?;
        shape.bound.iter().find_map(|&(m, local)| self.fixed.get(m).and_then(Option::as_ref).map(|b| (b, local)))
    }

    /// What runs the subqueries of an expression.
    pub(super) fn runner(&self) -> Runner {
        Runner::new(&self.db, self.tx)
    }

    /// A match row of no element bound: every variable's columns and slot
    /// null, then the operands.
    pub(super) fn blank(&self) -> Row {
        let mut row = vec![Value::Null; self.graph.width.saturating_add(self.graph.slots.len())];
        row.extend(self.operands.iter().cloned());
        row
    }

    /// The identity of `element` [GS §5 R5.1]; null if its row is gone.
    ///
    /// # Errors
    ///
    /// Any error reading its row.
    pub(super) fn identity(&self, element: ElementId) -> Result<Value> {
        let Some(source) = self.graph.tables.get(usize::try_from(element.table).unwrap_or(usize::MAX)) else { return Ok(Value::Null) };
        let Some(row) = self.store.row(element)? else { return Ok(Value::Null) };
        let key: Vec<Value> = source.key.iter().map(|&k| row.get(k).cloned().unwrap_or(Value::Null)).collect();
        Ok(Value::Char(element_text(&source.alias, source.native, &key)?))
    }

    /// Fills the columns of element variable `var` in `row` with those of
    /// `element`, nulls if it is of no table the variable may bind.
    ///
    /// # Errors
    ///
    /// Any error reading the element or evaluating a property.
    pub(super) fn fill(&self, row: &mut Row, var: usize, element: ElementId) -> Result<()> {
        let Some(shape) = self.graph.vars.get(var) else { return Ok(()) };
        let table = usize::try_from(element.table).unwrap_or(usize::MAX);
        let (Some(projection), Some(stored)) = (shape.projection(table), self.store.row(element)?) else { return Ok(()) };
        let runner = self.runner();
        for (k, e) in projection.iter().enumerate() {
            let v = eval(e, &stored, &runner)?;
            if let Some(cell) = row.get_mut(shape.offset.saturating_add(k)) {
                *cell = v;
            }
        }
        if shape.edge {
            let ends = self.store.endpoints(element)?;
            let hidden = shape.offset.saturating_add(shape.width).saturating_sub(EDGE_COLUMNS);
            for (at, end) in [(SOURCE_AT, ends.source), (DESTINATION_AT, ends.destination)] {
                let id = self.identity(end)?;
                if let Some(cell) = row.get_mut(hidden.saturating_add(at)) {
                    *cell = id;
                }
            }
        }
        Ok(())
    }

    /// Whether prefilter `k` is true of the bindings `bound` [GS §6.8].
    ///
    /// # Errors
    ///
    /// Any error evaluating it.
    pub(super) fn prefilter(&self, k: usize, bound: &Bound<'_>) -> Result<bool> {
        let Some(p) = self.graph.prefilters.get(k) else { return Ok(true) };
        let mut row = self.blank();
        if let (Some(own), Some(element)) = (p.own, bound.element()) {
            self.fill(&mut row, own, element)?;
        }
        for &(var, id) in &p.reads {
            if let Some(element) = bound.singleton(id) {
                self.fill(&mut row, var, element)?;
            }
        }
        for &var in &p.across {
            if let Some(element) = self.fixed_of(var).and_then(|(b, local)| b.singleton(local)) {
                self.fill(&mut row, var, element)?;
            }
        }
        let runner = self.runner();
        for &(slot, id) in &p.slots {
            let value = match self.graph.slots.get(slot) {
                Some(GraphValue::Aggregate { call, var }) => {
                    let across = self.fixed_of(*var).filter(|_| id == VarId::MAX).map(|(b, local)| b.group(local));
                    let elements = across.unwrap_or_else(|| bound.group(id).to_vec());
                    self.aggregate(call, (*var, &elements), &row, &runner)?
                }
                Some(GraphValue::Iteration { var, first, property }) => {
                    let element = if *first { bound.first(id) } else { bound.previous(id) };
                    self.iteration(element, *var, *property)?
                }
                Some(GraphValue::Path { path, function, span }) => {
                    let fixed = || self.fixed.get(*path).cloned().flatten();
                    let whole = if *path == p.path { bound.partial(*span) } else { fixed().and_then(|b| part(b, *span)) };
                    whole.map_or(Ok(Value::Null), |b| self.path_value(&b, *function))?
                }
                None => Value::Null,
            };
            if let Some(cell) = row.get_mut(self.graph.width.saturating_add(slot)) {
                *cell = value;
            }
        }
        Ok(eval_cond(&p.cond, &row, &runner)? == Truth::True)
    }

    /// \[GS §11.1\] PREV or FIRST of element variable `var`, its binding
    /// then `element`: its identity, or its property numbered `property`;
    /// null of none.
    ///
    /// # Errors
    ///
    /// Any error reading the element or evaluating the property.
    fn iteration(&self, element: Option<ElementId>, var: usize, property: Option<usize>) -> Result<Value> {
        let Some(element) = element else { return Ok(Value::Null) };
        let Some(k) = property else { return self.identity(element) };
        let mut row = self.blank();
        self.fill(&mut row, var, element)?;
        let at = self.graph.vars.get(var).map_or(usize::MAX, |v| v.offset.saturating_add(k));
        Ok(row.get(at).cloned().unwrap_or(Value::Null))
    }

    /// \[GS §7.2\] The COST of path pattern `path` of following `edge`.
    ///
    /// # Errors
    ///
    /// Any error evaluating it; SQLSTATE 22W34 of a cost not a number.
    pub(super) fn cost(&self, path: usize, edge: ElementId) -> Result<f64> {
        let Some(cost) = self.graph.paths.get(path).and_then(|p| p.cost.as_ref()) else { return Ok(1.0) };
        let mut row = self.blank();
        if let Some(var) = cost.var {
            self.fill(&mut row, var, edge)?;
        }
        let v = eval(&cost.expr, &row, &self.runner())?;
        Ok(v.as_f64().unwrap_or(f64::NAN))
    }
}

impl Shared {
    /// \[GS §11.2\] Whether EXISTS IN PATH's pattern `k` matches path `b`
    /// in path order, its prefilters reading the fixed bindings.
    ///
    /// # Errors
    ///
    /// Any error of the pattern or its prefilters.
    pub(super) fn within(&self, k: usize, b: &PathBinding) -> Result<bool> {
        let Some(shape) = self.graph.within.get(k) else { return Ok(false) };
        let held = Arc::new(self.with_fixed(self.fixed.clone()));
        let pattern = shape.instantiate(&mut |i| filter(&held, i));
        crate::graph::within::within(&pattern, &*self.store, &b.elements)
    }
}

/// Of `whole`, the subpath between the marks `span`, or all of it.
fn part(whole: PathBinding, span: Option<(VarId, VarId)>) -> Option<PathBinding> {
    match span {
        Some((start, end)) => whole.span(start, end),
        None => Some(whole),
    }
}

/// Prefilter `k` of `shared`, as path search calls it.
pub(super) fn filter(shared: &Arc<Shared>, k: usize) -> Filter {
    let held = Arc::clone(shared);
    let test: Predicate = Arc::new(move |bound| held.prefilter(k, bound));
    Filter { reads: shared.graph.reads(k), test }
}

/// The COST of path pattern `path` of `shared`, as path search calls it.
pub(super) fn cost(shared: &Arc<Shared>, path: usize) -> CostFn {
    let held = Arc::clone(shared);
    Arc::new(move |edge| held.cost(path, edge))
}
