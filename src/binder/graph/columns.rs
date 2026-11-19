//! \[EXTENSION\] What a GRAPH_TABLE gives of each match [GS §8]: its rows
//! clause, its COLUMNS, each named [R8.4], `v.*` every property of `v`;
//! its element patterns' conditions, the WHERE and the property
//! specifications; COST of a CHEAPEST selector [GS §7.2]; and how each
//! element variable's columns are made of a row of each element table it
//! may bind.

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use super::grf;
use super::remap::Operands;
use super::scope::Graphing;
use super::variables::{Declared, candidates};
use crate::ast::{ElementPattern, GraphColumn, GraphPattern, GraphRows, GraphTable, Selector, ValueExpression};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::graph::limits::OnLimit;
use crate::logical::{
    AggCall, Cond, CostShape, Distribution, ElementIdentity, ElementSource, Expr, GraphOptions, GraphValue, PathShape, PatternShape,
    RowsShape, SUBPATH_MARK, VarShape, and_all,
};
use crate::parser::{Degree, Exposure};
use crate::value::{CompOp, DataType, Decimal, Value};

/// The rule a set function in COST breaks.
const ONE_EDGE: &str = "[GS §7.2] COST is of one edge, so it holds no set function";

/// The variables the rows clause `rows` declares: name, whether an edge's,
/// and where.
pub(super) fn row_variables(rows: &GraphRows) -> Vec<(String, bool, usize)> {
    match rows {
        GraphRows::Match => Vec::new(),
        GraphRows::Vertex(v) => vec![(v.name.clone(), false, v.at)],
        GraphRows::Step(a, e, b) => vec![(a.name.clone(), false, a.at), (e.name.clone(), true, e.at), (b.name.clone(), false, b.at)],
    }
}

/// \[GS §8 R8.3\] The rows clause, its variables numbered of `names`.
pub(super) fn rows(rows: &GraphRows, names: &[String]) -> Result<RowsShape> {
    let of = |v: &crate::ast::GraphVariable| {
        names.iter().position(|n| n == &v.name).ok_or_else(|| grf("GRF-0024", format!("{} is not declared", v.name)))
    };
    Ok(match rows {
        GraphRows::Match => RowsShape::Match,
        GraphRows::Vertex(v) => RowsShape::Vertex(of(v)?),
        GraphRows::Step(a, e, b) => RowsShape::Step(of(a)?, of(e)?, of(b)?),
    })
}

/// \[GS §6.8 R6.2\] The condition of element pattern `p` of variable
/// `own`: its WHERE, and each property specification an equality.
pub(super) fn element_condition(binder: &Binder<'_>, p: &ElementPattern, (own, g): (usize, &Graphing), scope: Scope<'_>) -> Result<Cond> {
    let mut conds = Vec::new();
    if let Some(w) = &p.where_clause {
        conds.push(binder.search_condition(w, scope)?);
    }
    let var = g.vars.get(own).ok_or_else(|| grf("GRF-0024", "a variable is lost"))?;
    for spec in &p.properties {
        let k = var.columns.iter().position(|(n, _)| n == &spec.name);
        let k = k.ok_or_else(|| grf("GRF-0013", format!("{} has no property {}", var.name, spec.name)).or_at(spec.at))?;
        let column = binder.graph_column(var.offset.saturating_add(k), scope).unwrap_or(Expr::Column(var.offset.saturating_add(k)));
        let (value, _) = binder.value_expression(&spec.value, scope).map_err(|e| e.or_at(spec.at))?;
        conds.push(Cond::Compare(column, CompOp::Eq, value));
    }
    Ok(and_all(conds).unwrap_or(Cond::Const(crate::logical::Truth::True)))
}

/// The slots a prefilter made, each with its element variable, and the
/// variables its set functions aggregate.
type Made = (Vec<(usize, usize)>, Vec<usize>);

/// \[GS §7.4 R7.4\] The slots a prefilter made past `made`, each with its
/// element variable, and the variables its set functions aggregate, each
/// over the elements bound to it within its parenthesized path pattern:
/// one of a variable unbounded where it reads it, seen first as `local`
/// exposes it, is GRF-0032.
pub(super) fn slots_in_prefilter(g: &Graphing, made: usize, (local, exposed): (Option<&Exposure>, &Exposure)) -> Result<Made> {
    let slots = g.slots.borrow();
    let (mut made_here, mut vars) = (Vec::new(), Vec::new());
    for (k, slot) in slots.iter().enumerate().skip(made) {
        let var = match slot {
            GraphValue::Iteration { var, .. } | GraphValue::Aggregate { var, .. } => *var,
            GraphValue::Path { .. } => {
                // [GS §6.9] Of the path so far, which path search keeps.
                made_here.push((k, usize::MAX));
                continue;
            }
        };
        let name = g.vars.get(var).map_or("", |v| v.name.as_str());
        let seen = local.and_then(|l| l.get(name)).or_else(|| exposed.get(name));
        let aggregate = matches!(slot, GraphValue::Aggregate { .. });
        if aggregate && let Some(Degree::Group { unbounded: true }) = seen.map(|e| e.degree) {
            return Err(grf("GRF-0032", format!("a prefilter aggregates group variable {name}, unbounded where it reads it (R7.4)")));
        }
        made_here.extend(g.vars.get(var).map(|_| (k, var)));
        vars.extend(g.vars.get(var).filter(|_| aggregate).map(|_| var));
    }
    Ok((made_here, vars))
}

/// The name of COLUMNS item `v`, as written or of the property it reads.
fn column_name(v: &ValueExpression, name: Option<&String>) -> Result<String> {
    if let Some(n) = name {
        return Ok(n.clone());
    }
    if let ValueExpression::Column(cs) = v {
        return Ok(cs.column.clone());
    }
    Err(rule("[GS §8 R8.4] a COLUMNS item that is no property reference is named by AS"))
}

/// A column of a GRAPH_TABLE's result.
fn result_column(name: String, data_type: DataType) -> ColumnDesc {
    ColumnDesc { name, data_type, not_null: false, default: None, domain: None, collation: None, generated: None }
}

/// \[GS §8 R8.4\] COLUMNS of `g`, bound in `scope`, and the result's
/// columns: one of each item, of each property of `v` of `v.*`.
///
/// # Errors
///
/// SQLCODE -103 if one is unnamed or two have one name.
pub(super) fn columns(binder: &Binder<'_>, g: &GraphTable, scope: Scope<'_>) -> Result<(Vec<Expr>, Vec<ColumnDesc>)> {
    let graphing = binder.graphing.as_deref().ok_or_else(|| grf("GRF-0024", "a GRAPH_TABLE binder has no variables"))?;
    let (mut exprs, mut out) = (Vec::new(), Vec::new());
    for c in &g.columns {
        match c {
            GraphColumn::Value { value, name, at } => {
                let (bound, t) = binder.value_expression(value, scope).map_err(|e| e.or_at(*at))?;
                exprs.push(bound);
                out.push(result_column(column_name(value, name.as_ref()).map_err(|e| e.or_at(*at))?, t));
            }
            GraphColumn::All(v) => {
                let var = graphing
                    .vars
                    .iter()
                    .find(|x| x.name == v.name)
                    .ok_or_else(|| grf("GRF-0024", format!("{} is not declared", v.name)).or_at(v.at))?;
                for (k, (name, t)) in var.columns.iter().enumerate() {
                    let pos = var.offset.saturating_add(k);
                    exprs.push(binder.graph_column(pos, scope).unwrap_or(Expr::Column(pos)));
                    out.push(result_column(name.clone(), t.clone()));
                }
            }
        }
    }
    for (k, c) in out.iter().enumerate() {
        if out.iter().skip(k.saturating_add(1)).any(|d| d.name == c.name) {
            return Err(rule(format!("[GS §8 R8.4] two columns of a GRAPH_TABLE are named {}", c.name)).or_at(g.at));
        }
    }
    Ok((exprs, out))
}

/// \[GS §7.2\] COST of each path pattern's CHEAPEST selector, if written:
/// over one edge variable's columns.
pub(super) fn costs(binder: &Binder<'_>, pattern: &GraphPattern, g: &Graphing) -> Result<Vec<Option<CostShape>>> {
    let mut out = Vec::new();
    for path in &pattern.paths {
        let prefix = path.prefix.as_ref().or(pattern.keep.as_ref());
        let Some(Selector::Cheapest { cost: Some(cost), .. }) = prefix.and_then(|p| p.selector.as_ref()) else {
            out.push(None);
            continue;
        };
        g.take_reads();
        let (expr, t) = binder.value_expression(cost, Scope::Rows(ONE_EDGE))?;
        if !t.is_numeric() {
            return Err(rule("[GS §7.2] COST is of a numeric type"));
        }
        let reads = g.take_reads();
        let edge = reads.iter().all(|&v| g.vars.get(v).is_some_and(|x| x.edge));
        if reads.len() > 1 || !edge {
            return Err(rule("[GS §7.2] COST reads the properties of one edge variable"));
        }
        out.push(Some(CostShape { var: reads.first().copied(), expr }));
    }
    Ok(out)
}

/// The names of the properties `tables` of `infos` have, in the order a
/// variable binding them has them.
fn union_names(tables: &[usize], infos: &[super::elements::TableInfo]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for info in tables.iter().filter_map(|&t| infos.get(t)) {
        names.extend(info.properties.iter().filter(|(n, _)| !names.contains(n)).map(|(n, _)| n.clone()).collect::<Vec<_>>());
    }
    names
}

/// The expressions of the columns of a variable of property names
/// `names`, an edge's if `edge`, over a row of element table `t`.
fn projection(t: usize, source: &ElementSource, names: &[String], edge: bool) -> Vec<Expr> {
    let mut out: Vec<Expr> =
        names.iter().map(|n| source.properties.iter().find(|(p, _)| p == n).map_or(Expr::Const(Value::Null), |(_, e)| e.clone())).collect();
    let key = source.key.iter().map(|&k| Expr::Column(k)).collect();
    out.push(Expr::Call(ScalarRef(Arc::new(ElementIdentity { alias: source.alias.clone(), native: source.native })), key));
    out.push(Expr::Const(Value::Exact(Decimal::new(i128::from(super::scope::table_number(t)), 0))));
    if edge {
        // Its ends' identities, which the search fills (`exec/graph_table`).
        out.push(Expr::Const(Value::Null));
        out.push(Expr::Const(Value::Null));
        out.push(Expr::Const(Value::Bool(source.ends.as_ref().is_some_and(|e| e.directed))));
    }
    out
}

/// Each variable's projections and path patterns, and where each path
/// pattern's matches may start.
pub(super) fn fill_shapes(
    shapes: &mut [VarShape],
    paths: &mut [PathShape],
    (tables, infos, declared): (&[ElementSource], &[super::elements::TableInfo], &[Declared]),
) {
    for (shape, d) in shapes.iter_mut().zip(declared) {
        let candidates = candidates(d, infos);
        let names = union_names(&candidates, infos);
        shape.tables = candidates.iter().filter_map(|&t| Some((t, projection(t, tables.get(t)?, &names, d.edge)))).collect();
    }
    for (n, path) in paths.iter_mut().enumerate() {
        for (k, (_, _, s)) in path.variables.iter().enumerate() {
            if let (Some(shape), Ok(id)) = (shapes.get_mut(*s), u32::try_from(k)) {
                shape.bound.push((n, id));
            }
        }
        path.starts = match first_vertex(&path.pattern).and_then(|v| shapes.get(v)) {
            Some(v) => v.tables.iter().map(|(t, _)| *t).collect(),
            None => tables.iter().enumerate().filter(|(_, t)| t.ends.is_none()).map(|(k, _)| k).collect(),
        };
    }
}

/// The element variable of the node pattern every match of `p` begins
/// with, if one does.
fn first_vertex(p: &PatternShape) -> Option<usize> {
    match p {
        PatternShape::Vertex(t) => Some(t.shape),
        PatternShape::Concat(parts) => parts.iter().find(|part| !marks(part)).and_then(first_vertex),
        PatternShape::Group(_, body, _) => first_vertex(body),
        PatternShape::Edge(..) | PatternShape::Union(_) | PatternShape::Alternation(_) | PatternShape::Quantified(..) => None,
    }
}

/// Whether `p` marks where a subpath begins or ends, a test of no
/// element variable at the vertex after it, or the one before.
fn marks(p: &PatternShape) -> bool {
    let PatternShape::Vertex(t) = p else { return false };
    t.shape == SUBPATH_MARK
}

/// Slot `s` over the match row.
pub(super) fn slot_remapped(s: GraphValue, operands: &mut Operands) -> Result<GraphValue> {
    match s {
        GraphValue::Aggregate { call, var } => {
            if var == usize::MAX {
                return Err(rule("[GS §6.6] COUNT(*) of a GRAPH_TABLE counts the elements of a variable: COUNT(v.*)"));
            }
            let AggCall { func, arg, distinct, filter, distribution, result, label } = *call;
            let arg = arg.map(|e| operands.expr(e));
            let filter = filter.map(|c| operands.cond(c));
            let distribution = distribution.map(|d| Box::new(Distribution { fraction: operands.expr(d.fraction), ..*d }));
            Ok(GraphValue::Aggregate { call: Box::new(AggCall { func, arg, distinct, filter, distribution, result, label }), var })
        }
        GraphValue::Path { .. } | GraphValue::Iteration { .. } => Ok(s),
    }
}

/// \[GS §20\] The resource limits `g` writes.
pub(super) fn options(g: &GraphTable) -> GraphOptions {
    let o = g.options;
    let on_limit = o.truncate.map(|t| if t { OnLimit::Truncate } else { OnLimit::Error });
    GraphOptions {
        max_path_length: o.max_path_length.map(u64::from),
        max_paths_per_partition: o.max_paths_per_partition.map(u64::from),
        max_search_states: o.max_search_states.map(u64::from),
        on_limit,
        deterministic: o.deterministic,
    }
}
