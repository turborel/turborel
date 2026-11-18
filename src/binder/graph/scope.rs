//! \[EXTENSION\] The scope a GRAPH_TABLE's conditions and COLUMNS are
//! bound in [GS §6.6, §6.8, §8]: each element variable a FROM item of its
//! properties and hidden columns (`logical/graph.rs`), whose columns a
//! reference reads; a set function's argument of a group variable's
//! columns, its value a slot of the match row computed of the group's
//! elements [P1 §4.4]; and the graph functions and predicates, each an
//! expression over the hidden columns or a slot.
//!
//! The binder consults it where a column, a set function or a graph
//! function is bound (`Binder::graph_column`, `graph_registered`,
//! `graph_count`, `graph_function`, `graph_predicate`).

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use super::super::{Binder, Grouping, Scope, rule};
use crate::ast::{ColumnSpecification, GraphFunction, GraphFunctionName, GraphPredicate, PathEnd, SetFunctionSpecification};
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::logical::{AggCall, Cond, Expr, GraphValue, PathFunction, or_all};
use crate::value::{CompOp, DataType, Value};

/// The hidden columns after a variable's properties, in order.
pub(super) const ID: usize = 0;
/// See `ID`.
pub(super) const TABLE: usize = 1;
/// See `ID`; of an edge.
pub(super) const SOURCE: usize = 2;
/// See `ID`; of an edge.
pub(super) const DESTINATION: usize = 3;
/// See `ID`; of an edge.
pub(super) const DIRECTED: usize = 4;

/// An element variable as the scope sees it.
pub(super) struct ScopeVar {
    /// Its name.
    pub(super) name: String,
    /// Its first column in the binder's row.
    pub(super) offset: usize,
    /// Its properties, before its hidden columns.
    pub(super) properties: usize,
    /// Each property's name and type.
    pub(super) columns: Vec<(String, DataType)>,
    /// Whether it binds edges.
    pub(super) edge: bool,
    /// Each element table it may bind: its number, labels and properties.
    pub(super) tables: Vec<(usize, Vec<String>, Vec<String>)>,
}

impl ScopeVar {
    /// The binder's row position of its hidden column `k`.
    pub(super) fn hidden(&self, k: usize) -> usize {
        self.offset.saturating_add(self.properties).saturating_add(k)
    }

    /// The width of its columns.
    pub(super) fn width(&self) -> usize {
        self.properties.saturating_add(if self.edge { crate::logical::EDGE_COLUMNS } else { crate::logical::VERTEX_COLUMNS })
    }
}

/// \[EXTENSION\] The scope of a GRAPH_TABLE's expressions.
pub(in crate::binder) struct Graphing {
    /// The element variables.
    pub(super) vars: Vec<ScopeVar>,
    /// The path variables, by path pattern.
    pub(super) paths: Vec<Option<String>>,
    /// The groups its expressions are bound in, whose set functions are
    /// slots.
    pub(super) measure: Grouping,
    /// The binder's row position of the first slot.
    pub(super) slot_base: usize,
    /// The slots made so far.
    pub(super) slots: RefCell<Vec<GraphValue>>,
    /// The variable a set function's argument read, if one did.
    found: Cell<Option<usize>>,
    /// The variables the expression bound now reads.
    reads: RefCell<BTreeSet<usize>>,
}

impl Graphing {
    /// A scope of `vars` and the path variables `paths`, its slots from
    /// `slot_base`.
    pub(super) fn new(vars: Vec<ScopeVar>, paths: Vec<Option<String>>, slot_base: usize) -> Graphing {
        Graphing {
            vars,
            paths,
            measure: Grouping::empty(),
            slot_base,
            slots: RefCell::new(Vec::new()),
            found: Cell::new(None),
            reads: RefCell::new(BTreeSet::new()),
        }
    }

    /// The variables read since the last call, taken.
    pub(super) fn take_reads(&self) -> BTreeSet<usize> {
        self.reads.take()
    }

    /// The variable whose columns hold row position `pos`.
    fn var_at(&self, pos: usize) -> Option<usize> {
        self.vars.iter().position(|v| pos >= v.offset && pos < v.offset.saturating_add(v.width()))
    }

    /// The element variable named `name`.
    fn var_named(&self, name: &str) -> Result<usize> {
        let found = self.vars.iter().position(|v| v.name == name);
        found.ok_or_else(|| rule(format!("[EXTENSION] {name} is no element variable of the graph pattern")))
    }

    /// Notes that variable `v` is read, and its column read there.
    fn read(&self, v: usize, pos: usize) -> Expr {
        self.reads.borrow_mut().insert(v);
        Expr::Column(pos)
    }

    /// A new slot of `value`: its column.
    fn slot(&self, value: GraphValue) -> Expr {
        let mut slots = self.slots.borrow_mut();
        slots.push(value);
        Expr::Column(self.slot_base.saturating_add(slots.len().saturating_sub(1)))
    }

    /// The condition that variable `v` is bound to an element of one of
    /// the tables `holds` is true of.
    fn of_tables(&self, v: usize, holds: &dyn Fn(&[String], &[String]) -> bool) -> Cond {
        let Some(var) = self.vars.get(v) else { return Cond::Const(crate::logical::Truth::False) };
        let column = self.read(v, var.hidden(TABLE));
        let each = var.tables.iter().filter(|(_, labels, props)| holds(labels, props)).map(|(t, _, _)| {
            Cond::Compare(
                column.clone(),
                CompOp::Eq,
                Expr::Const(Value::Exact(crate::value::Decimal::new(i128::from(table_number(*t)), 0))),
            )
        });
        or_all(each.collect()).unwrap_or(Cond::Const(crate::logical::Truth::False))
    }
}

/// The number of element table `t` as its hidden column holds it.
pub(super) fn table_number(t: usize) -> u32 {
    u32::try_from(t).unwrap_or(u32::MAX)
}

/// The refusal of a feature of the graph standard this release reads but
/// does not bind.
pub(super) fn unsupported(what: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] {what} is not supported"))
}

impl Binder<'_> {
    /// \[GS §6.6\] Of a GRAPH_TABLE's binder, the column at `pos`: a
    /// variable's, read; in a set function's argument, of the group
    /// variable it aggregates; or one of a table before it. None of
    /// another binder.
    pub(in crate::binder) fn graph_column(&self, pos: usize, scope: Scope<'_>) -> Option<Expr> {
        let g = self.graphing.as_deref()?;
        // A column of a table before it, read laterally, is one value.
        let Some(v) = g.var_at(pos) else { return Some(Expr::Column(pos)) };
        if let Scope::Rows(_) = scope {
            g.found.set(Some(v));
        }
        Some(g.read(v, pos))
    }

    /// \[GS §6.6\] Of a GRAPH_TABLE's binder, a bare element variable
    /// name `cs`, as COUNT(e) writes it [P1 §4.4]: its identity.
    pub(in crate::binder) fn graph_variable(&self, cs: &ColumnSpecification, scope: Scope<'_>) -> Option<(Expr, DataType)> {
        let g = self.graphing.as_deref()?;
        if cs.qualifier.is_some() {
            return None;
        }
        let v = g.vars.iter().position(|v| v.name == cs.column && !v.name.starts_with('$'))?;
        let pos = g.vars.get(v)?.hidden(ID);
        Some((self.graph_column(pos, scope)?, crate::logical::identity_type()))
    }

    /// Of a GRAPH_TABLE's groups `g`, the set function `call`: a slot of
    /// it over the group variable its argument read [P1 §4.4]. None of
    /// other groups.
    pub(in crate::binder) fn graph_registered(&self, g: &Grouping, call: &AggCall) -> Option<Result<(Expr, DataType)>> {
        let graphing = self.graphing.as_deref().filter(|r| std::ptr::eq(g, std::ptr::from_ref(&r.measure)))?;
        let found = graphing.found.take();
        let Some(var) = found.or_else(|| call.arg.is_none().then_some(usize::MAX)) else {
            return Some(Err(rule(format!(
                "[GS §6.6] the argument of {} in a GRAPH_TABLE reads a group variable's properties",
                call.label
            ))));
        };
        if call.distribution.is_some() {
            return Some(Err(unsupported("an inverse distribution function of a group variable")));
        }
        let result = call.result.clone();
        Some(Ok((graphing.slot(GraphValue::Aggregate { call: Box::new(call.clone()), var }), result)))
    }

    /// \[P1 §4.4\] Of a GRAPH_TABLE's binder, COUNT (`name`.*): the
    /// number of elements bound to group variable `name`.
    pub(in crate::binder) fn graph_count(
        &self,
        sf: &SetFunctionSpecification,
        name: &str,
        scope: Scope<'_>,
    ) -> Option<Result<(Expr, DataType)>> {
        let g = self.graphing.as_deref()?;
        let v = match g.var_named(name) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let Some(count) = self.db.functions.aggregate(kw::COUNT) else {
            return Some(Err(SqlError::new(Code::Api, "[DESIGN] COUNT is not registered")));
        };
        let counted = self.aggregate(count, (None, false), sf.filter.as_deref(), scope);
        // COUNT(*) bound, its slot is of the variable named.
        if let Ok((Expr::Column(c), _)) = &counted
            && let Some(GraphValue::Aggregate { var, .. }) = g.slots.borrow_mut().get_mut(c.saturating_sub(g.slot_base))
        {
            *var = v;
        }
        Some(counted)
    }

    /// \[GS §5, §6.9\] A graph function `f` of a GRAPH_TABLE's binder.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 outside a GRAPH_TABLE, or of no variable of its kind;
    /// -900 of PREV or FIRST.
    pub(in crate::binder) fn graph_function(&self, f: &GraphFunction) -> Result<(Expr, DataType)> {
        let Some(g) = self.graphing.as_deref() else { return Err(super::seam::outside_graph_table()) };
        let name = &f.operand.name;
        let path = g.paths.iter().position(|p| p.as_deref() == Some(name.as_str()));
        let function = match f.name {
            GraphFunctionName::PathLength => PathFunction::Length,
            GraphFunctionName::Vertices => PathFunction::Vertices,
            GraphFunctionName::Edges => PathFunction::Edges,
            GraphFunctionName::IsAcyclic => PathFunction::Acyclic,
            GraphFunctionName::IsTrail => PathFunction::Trail,
            GraphFunctionName::IsSimple => PathFunction::Simple,
            GraphFunctionName::PathToJson => PathFunction::Json,
            GraphFunctionName::ElementId if path.is_some() => PathFunction::Elements,
            GraphFunctionName::ElementId | GraphFunctionName::VertexId | GraphFunctionName::EdgeId | GraphFunctionName::Labels => {
                return super::functions::element_function(self, g, (f.name, g.var_named(name)?));
            }
            GraphFunctionName::Prev | GraphFunctionName::First => return Err(unsupported("PREV and FIRST [GS §11.1]")),
        };
        let path = path.ok_or_else(|| rule(format!("[GS §6.9] {name} is no path variable of the graph pattern")))?;
        let t = super::functions::path_type(function);
        Ok((g.slot(GraphValue::Path { path, function }), t))
    }

    /// \[GS §5 R5.2 to R5.5\] A graph predicate `p` of a GRAPH_TABLE's
    /// binder.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 outside a GRAPH_TABLE or of an operand of no variable;
    /// -900 of EXISTS IN PATH.
    pub(in crate::binder) fn graph_predicate(&self, p: &GraphPredicate) -> Result<Cond> {
        let Some(g) = self.graphing.as_deref() else { return Err(super::seam::outside_graph_table()) };
        let id = |name: &str| -> Result<Expr> {
            let v = g.var_named(name)?;
            let var = g.vars.get(v).ok_or_else(|| rule("[DESIGN] a variable is gone"))?;
            Ok(g.read(v, var.hidden(ID)))
        };
        Ok(match p {
            GraphPredicate::Same { elements, .. } => {
                let ids = elements.iter().map(|e| id(&e.name)).collect::<Result<Vec<_>>>()?;
                let pairs = ids.windows(2).filter_map(|w| Some(Cond::Compare(w.first()?.clone(), CompOp::Eq, w.get(1)?.clone())));
                crate::logical::and_all(pairs.collect()).unwrap_or(Cond::Const(crate::logical::Truth::True))
            }
            GraphPredicate::AllDifferent(elements) => {
                let ids = elements.iter().map(|e| id(&e.name)).collect::<Result<Vec<_>>>()?;
                let mut pairs = Vec::new();
                for (i, a) in ids.iter().enumerate() {
                    pairs.extend(ids.iter().skip(i.saturating_add(1)).map(|b| Cond::Compare(a.clone(), CompOp::Ne, b.clone())));
                }
                crate::logical::and_all(pairs).unwrap_or(Cond::Const(crate::logical::Truth::True))
            }
            GraphPredicate::PropertyExists { element, property } => {
                g.of_tables(g.var_named(&element.name)?, &|_, props| props.iter().any(|name| name == property))
            }
            GraphPredicate::Labeled { element, label, not } => {
                let held = g.of_tables(g.var_named(&element.name)?, &|labels, _| super::functions::label_holds(label, labels));
                negated(held, *not)
            }
            GraphPredicate::Endpoint { node, edge, end, not } => {
                let e = g.var_named(&edge.name)?;
                let var = g.vars.get(e).ok_or_else(|| rule("[DESIGN] a variable is gone"))?;
                let column = if *end == PathEnd::Source { SOURCE } else { DESTINATION };
                let (directed, at_end) = (g.read(e, var.hidden(DIRECTED)), g.read(e, var.hidden(column)));
                let held = Cond::And(Box::new(is_true(directed)), Box::new(Cond::Compare(id(&node.name)?, CompOp::Eq, at_end)));
                negated(held, *not)
            }
            GraphPredicate::Directed { edge, not } => {
                let e = g.var_named(&edge.name)?;
                let var = g.vars.get(e).ok_or_else(|| rule("[DESIGN] a variable is gone"))?;
                negated(is_true(g.read(e, var.hidden(DIRECTED))), *not)
            }
            GraphPredicate::InPath { .. } => return Err(unsupported("EXISTS IN PATH [GS §11.2]")),
        })
    }
}

/// Whether boolean `e` is true.
fn is_true(e: Expr) -> Cond {
    Cond::Compare(e, CompOp::Eq, Expr::Const(Value::Bool(true)))
}

/// `c`, or NOT `c` if `not`.
fn negated(c: Cond, not: bool) -> Cond {
    if not { Cond::Not(Box::new(c)) } else { c }
}
