//! \[EXTENSION\] [GS §8] A GRAPH_TABLE, bound: its graph's element tables
//! (`graph/elements.rs`), its element variables, each a FROM item of its
//! properties in the scope its expressions are bound in
//! (`graph/variables.rs`, `graph/scope.rs`), its path patterns and their
//! prefilters (`graph/pattern.rs`), its postfilter [GS §6.8 R6.4], its
//! rows clause and COLUMNS (`graph/columns.rs`). It is planned as a
//! lateral derived table is, of what it reads of the tables before it and
//! of enclosing queries (`graph/remap.rs`), and run by path search
//! (`exec/graph_table.rs`).

mod columns;
mod elements;
mod functions;
mod pattern;
mod remap;
mod scope;
mod seam;
mod variables;

use std::collections::BTreeSet;
use std::rc::Rc;

pub(super) use scope::Graphing;

use super::from::{Bounds, From, FromItem};
use super::unnest::{Unnested, lateral_binder, lateral_item};
use super::{Binder, Context, qualify};
use crate::ast::{GraphReference, GraphTable};
use crate::catalog::GraphDesc;
use crate::error::{Code, Result, SqlError};
use crate::graph::program::MatchMode;
use crate::logical::{GraphMatch, VarShape};
use crate::parser::{exposure, normalize};
use pattern::{ELEMENT_SCOPE, Filtered};
use remap::Operands;

/// The rule a set function where a GRAPH_TABLE's expressions do not
/// aggregate breaks.
const MEASURE: &str = "[GS §6.6] a GRAPH_TABLE's column or WHERE reads a group variable's properties in a set function alone";

/// The Syntax Rule error of graph pattern matching, the condition of
/// §22 it is.
pub(super) fn grf(condition: &str, msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {condition} {}", msg.into()))
}

/// The graph a GRAPH_TABLE reads, which `cx` may query.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0001) if there is none, (GRF-0005) if it is INVALID;
/// -104 if SELECT on it is not held; -900 of LINE_GRAPH.
fn graph_desc(cx: Context<'_>, reference: &GraphReference) -> Result<GraphDesc> {
    let name = match reference {
        GraphReference::Named(name) => name,
        GraphReference::LineGraph { .. } => return Err(scope::unsupported("LINE_GRAPH [GS §12]")),
    };
    let qn = qualify(name, cx.schema);
    let desc = cx.db.catalog.graph(&qn).ok_or_else(|| grf("GRF-0001", format!("there is no property graph {qn}")).or_at(name.at))?;
    desc.queryable(&*cx.db.catalog, cx.who()).map_err(|e| e.or_at(name.at))?;
    Ok(desc.clone())
}

/// The FROM item of GRAPH_TABLE `g`, its first column at `offset` of the
/// FROM row `from` has so far.
///
/// # Errors
///
/// SQLCODE -103 if a Syntax Rule is broken, each condition of §22 it is;
/// -104 if a privilege is not held; -900 of what this release does not
/// bind.
/// The variables, their FROM items and shapes, and the column past them.
type DeclaredVars = (Vec<scope::ScopeVar>, Vec<FromItem>, Vec<VarShape>, usize);

/// The variables of `declared`, their FROM items and shapes, from column
/// `offset`, and the column past them.
fn declared_vars(declared: &[variables::Declared], infos: &[elements::TableInfo], offset: usize) -> Result<DeclaredVars> {
    let (mut vars, mut items, mut shapes, mut start) = (Vec::new(), Vec::new(), Vec::new(), offset);
    for d in declared {
        let candidates = variables::candidates(d, infos);
        let (var, item) = variables::var_item(d, (&candidates, infos), start)?;
        start = start.saturating_add(var.width());
        let (shape_offset, width) = (var.offset.saturating_sub(offset), var.width());
        shapes.push(VarShape { name: d.name.clone(), edge: d.edge, offset: shape_offset, width, tables: Vec::new(), bound: Vec::new() });
        vars.push(var);
        items.push(item);
    }
    Ok((vars, items, shapes, start))
}

pub(in crate::binder) fn graph_table_item(b: Bounds<'_>, g: &GraphTable, offset: usize, from: &mut From) -> Result<FromItem> {
    let lateral = lateral_binder(b, from);
    let desc = graph_desc(b.cx, &g.graph)?;
    if let Some(s) = g.pattern.segments.first() {
        return Err(scope::unsupported("FOR EACH SEGMENT [GS §11.3]").or_at(s.first.at));
    }
    let pattern = normalize(&g.pattern);
    let exposed = exposure(&pattern)?;
    let (tables, infos, labels) = elements::element_tables(b, &desc, g.system_time.as_deref())?;
    let extra = columns::row_variables(&g.rows);
    let declared = variables::declared(&pattern, &extra)?;
    for d in &declared {
        d.labels.iter().flatten().try_for_each(|l| variables::labels_known(l, &labels))?;
    }
    let (vars, items, mut shapes, start) = declared_vars(&declared, &infos, offset)?;
    let width = start.saturating_sub(offset);
    let paths = pattern.paths.iter().map(|p| p.variable.as_ref().map(|v| v.name.clone())).collect();
    let graphing = Rc::new(Graphing::new(vars, paths, start));
    let names: Vec<String> = declared.iter().map(|d| d.name.clone()).collect();
    let built = Built { lateral: &lateral, items, graphing: Rc::clone(&graphing) };
    let bind = |f: &Filtered<'_>| built.filter(f, &exposed);
    let (mut path_shapes, prefilters) = pattern::paths(&pattern, (&names, &labels), &bind)?;
    let measure = built.binder(None);
    let scope = super::Scope::Groups(&graphing.measure, MEASURE);
    let postfilter = match &pattern.where_clause {
        Some(c) => Some(measure.search_condition(c, scope)?),
        None => None,
    };
    let (columns, out) = columns::columns(&measure, g, scope)?;
    let costs = columns::costs(&measure, &pattern, &graphing)?;
    let rows = columns::rows(&g.rows, &names)?;
    graphing.take_reads();
    columns::fill_shapes(&mut shapes, &mut path_shapes, (&tables, &infos, &declared));
    let slots = graphing.slots.take();
    let mut operands = Operands::new(offset, width.saturating_add(slots.len()));
    let assembled = Assembled { tables, labels, paths: path_shapes, prefilters, vars: shapes, slots, postfilter, columns, costs, rows };
    let graph = assembled.remapped(&mut operands, (&desc, &pattern, width), g)?;
    let reads: BTreeSet<usize> = operands.reads.clone();
    from.used.extend(reads.iter().copied());
    let correlation = g.correlation.clone().unwrap_or_else(|| format!("$GRAPH_TABLE{offset}"));
    Ok(lateral_item(&correlation, g.at, out, offset, Unnested::graph(graph, operands.exprs, reads)))
}

/// The scope a GRAPH_TABLE's expressions are bound in.
struct Built<'b> {
    /// The binder of the FROM row before it.
    lateral: &'b Binder<'b>,
    /// The element variables' FROM items.
    items: Vec<FromItem>,
    graphing: Rc<Graphing>,
}

impl Built<'_> {
    /// A binder of the FROM row before it and the variables, which sees
    /// unqualified what the FROM row does, and the properties of element
    /// variable `own` instead if given [GS §6.8 R6.2].
    fn binder(&self, own: Option<usize>) -> Binder<'_> {
        let mut items = self.lateral.items.clone();
        let first = items.len();
        items.extend(self.items.iter().cloned());
        let names = items.iter().flat_map(FromItem::row_names).collect();
        let visible = match own.and_then(|v| self.graphing.vars.get(v)) {
            Some(var) => (0..var.properties).map(|k| (first.saturating_add(own.unwrap_or_default()), k)).collect(),
            None => self.lateral.visible.clone(),
        };
        let cx = Context { draws: false, ..self.lateral.context() };
        let mut binder = Binder::new(cx, self.lateral.outer, (items, names, visible), BTreeSet::new());
        binder.graphing = Some(Rc::clone(&self.graphing));
        binder
    }

    /// Prefilter `f` bound: its condition and the variables it reads.
    fn filter(&self, f: &Filtered<'_>, exposed: &crate::parser::Exposure) -> Result<(crate::logical::Cond, BTreeSet<usize>)> {
        self.graphing.take_reads();
        let made = self.graphing.slots.borrow().len();
        let cond = match f {
            Filtered::Element(p, own) => columns::element_condition(&self.binder(Some(*own)), p, (*own, &self.graphing), ELEMENT_SCOPE)?,
            Filtered::Path(c) => {
                let scope = super::Scope::Groups(&self.graphing.measure, MEASURE);
                self.binder(None).search_condition(c, scope)?
            }
        };
        columns::slots_in_prefilter(&self.graphing, made, exposed)?;
        Ok((cond, self.graphing.take_reads()))
    }
}

/// What a GRAPH_TABLE is bound to, over the binder's row.
struct Assembled {
    tables: Vec<crate::logical::ElementSource>,
    labels: Vec<String>,
    paths: Vec<crate::logical::PathShape>,
    prefilters: Vec<crate::logical::Prefilter>,
    vars: Vec<VarShape>,
    slots: Vec<crate::logical::GraphValue>,
    postfilter: Option<crate::logical::Cond>,
    columns: Vec<crate::logical::Expr>,
    costs: Vec<Option<crate::logical::CostShape>>,
    rows: crate::logical::RowsShape,
}

impl Assembled {
    /// It over the match row, its operands found in `operands`.
    fn remapped(
        self,
        operands: &mut Operands,
        (desc, pattern, width): (&GraphDesc, &crate::ast::GraphPattern, usize),
        g: &GraphTable,
    ) -> Result<GraphMatch> {
        let prefilters = self.prefilters.into_iter().map(|p| crate::logical::Prefilter { cond: operands.cond(p.cond), ..p }).collect();
        let slots = self.slots.into_iter().map(|s| columns::slot_remapped(s, operands)).collect::<Result<Vec<_>>>()?;
        let postfilter = self.postfilter.map(|c| operands.cond(c));
        let columns = self.columns.into_iter().map(|e| operands.expr(e)).collect();
        let mut paths = self.paths;
        for (path, cost) in paths.iter_mut().zip(self.costs) {
            path.cost = cost.map(|c| crate::logical::CostShape { expr: operands.expr(c.expr), ..c });
        }
        let mode = match pattern.mode {
            // [GS §7.3] REPEATABLE ELEMENTS if none is written.
            Some(crate::ast::MatchMode::RepeatableElements) | None => MatchMode::RepeatableElements,
            Some(crate::ast::MatchMode::DifferentEdges) => MatchMode::DifferentEdges,
            Some(crate::ast::MatchMode::DifferentVertices) => MatchMode::DifferentVertices,
        };
        let operand_count = operands.exprs.len();
        let options = columns::options(g);
        Ok(GraphMatch {
            graph: desc.name.clone(),
            tables: self.tables,
            labels: self.labels,
            paths,
            mode,
            vars: self.vars,
            prefilters,
            slots,
            width,
            operands: operand_count,
            postfilter,
            rows: self.rows,
            columns,
            options,
        })
    }
}
