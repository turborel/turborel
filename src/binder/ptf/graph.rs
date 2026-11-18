//! \[EXTENSION\] An invocation of a function of GRAPH_ALGO, the graph
//! standard's §18 (RULES.md D111), bound as a `<PTF derived
//! table>`: its arguments put at their parameters' places, positionally
//! and then by name, as the standard writes `PAGERANK(g, damping =>
//! 0.85)`; its table arguments, the edges and the vertices, bound as a
//! polymorphic table function's are, so their rows are read with the
//! statement's privileges [GS §18 R18.2]; its value arguments bound to
//! their parameters' types; and its result's columns, a vertex's key and
//! its value, of a correlation name the function's own unless written.
//!
//! The edges argument is a table, its first two columns each edge's
//! source and destination, or a property graph's name, whose edge tables'
//! endpoint columns give the same two (`graph/catalogued.rs`).

mod catalogued;

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::from::Bounds;
use super::super::{Binder, Scope, assignable, rule};
use super::tables::{BoundTable, table_argument};
use super::{BoundPtf, Range, column, renamed};
use crate::ast::{GenericTable, PtfArgument, PtfInvocation, TableSemantics, ValueExpression};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::db::ptf::{GraphCall, GraphShape, Invoked};
use crate::error::{At, Result, SqlError};
use crate::graph::algo::{Algorithm, Kind, Output, Parameter, SCHEMA};
use crate::logical::{Expr, LogicalPlan, Polymorphic};
use crate::params::ParameterType;
use crate::value::{BIGINT_PRECISION, CharUnits, DOUBLE_PRECISION, DataType, Spelled, Value};

/// The longest COST, a column's name.
const COST_LENGTH: u32 = 128;
/// The edges' key columns, the source's and the destination's.
const ENDS: usize = 2;
/// How the edges and the vertices are read: whole, of no column passed
/// through.
pub(super) const WHOLE: GenericTable = GenericTable { pass_through: false, semantics: TableSemantics::Set { prune: false } };

/// Whether `inv` invokes a function of GRAPH_ALGO.
pub(super) fn is_graph_algorithm(inv: &PtfInvocation) -> bool {
    inv.schema.as_deref() == Some(SCHEMA)
}

/// An exception of a GRAPH_ALGO invocation.
pub(super) fn refused(algorithm: Algorithm, msg: &str) -> SqlError {
    rule(format!("[EXTENSION] GRAPH_ALGO.{}: {msg}", algorithm.name()))
}

/// The arguments of `inv` at the places of the parameters of `algorithm`:
/// those written without a name first, in order, then those named.
fn placed(algorithm: Algorithm, inv: &PtfInvocation) -> Result<Vec<Option<&PtfArgument>>> {
    let parameters = algorithm.parameters();
    let mut placed: Vec<Option<&PtfArgument>> = vec![None; parameters.len()];
    let mut named = false;
    for (k, (a, name)) in inv.arguments.iter().zip(&inv.names).enumerate() {
        let at = match (name, named) {
            (Some(n), _) => {
                parameters.iter().position(|p| p.name == n).ok_or_else(|| refused(algorithm, &format!("it has no parameter {n}")))?
            }
            (None, true) => return Err(refused(algorithm, "an argument without a name follows a named one")),
            (None, false) => k,
        };
        named = named || name.is_some();
        let slot = placed.get_mut(at).ok_or_else(|| refused(algorithm, &format!("it takes at most {} arguments", parameters.len())))?;
        if slot.replace(a).is_some() {
            let p = parameters.get(at).map_or("", |p| p.name);
            return Err(refused(algorithm, &format!("two arguments are of parameter {p}")));
        }
    }
    Ok(placed)
}

/// The type of a vertex's key: the edges' source's.
fn key_type(tables: &[BoundTable]) -> DataType {
    tables.first().and_then(|t| t.columns.first()).map_or_else(bigint, |c| c.data_type.clone())
}

/// DOUBLE PRECISION.
fn double() -> DataType {
    DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }
}

/// BIGINT.
fn bigint() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// The table argument `a` of parameter `p`, bound.
fn table(b: Bounds<'_>, (algorithm, p): (Algorithm, &Parameter), a: &PtfArgument, depth: usize) -> Result<BoundTable> {
    let PtfArgument::Table(ta) = a else {
        let what = if p.kind == Kind::Edges { ", or a property graph's name" } else { "" };
        return Err(refused(algorithm, &format!("{} is a table argument, TABLE ( <table or query> ){what}", p.name)));
    };
    let bound = table_argument(b, ta, WHOLE, depth)?;
    let least = if p.kind == Kind::Edges { ENDS } else { 1 };
    if bound.columns.len() < least {
        return Err(refused(algorithm, &format!("{}'s table has fewer than {least} columns", p.name)));
    }
    Ok(bound)
}

/// The value argument `value` of parameter `p`, of type `declared`, bound.
fn value(b: Bounds<'_>, (algorithm, p): (Algorithm, &Parameter), value: &ValueExpression, declared: DataType) -> Result<Expr> {
    let binder = Binder::new(b.cx, b.outer, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let scope = Scope::Rows("[EXTENSION] an argument of a function of GRAPH_ALGO has no set function");
    let (expr, found) = binder.typed(value, ParameterType::Of(declared.clone()), scope)?;
    if !assignable(&declared, &found) {
        return Err(refused(algorithm, &format!("{} is of {declared}, which an argument of {found} cannot be stored in", p.name)));
    }
    Ok(Expr::Cast(Box::new(expr), declared))
}

/// The type of a value parameter of kind `kind`, `key` a vertex's key's.
fn declared(kind: Kind, key: &DataType) -> DataType {
    match kind {
        Kind::Number => double(),
        Kind::Count => bigint(),
        Kind::Start | Kind::Edges | Kind::Vertices => key.clone(),
        Kind::Cost => DataType::Varying { length: COST_LENGTH, units: CharUnits::Characters },
    }
}

/// The columns of the result of `algorithm`, `key` a vertex's key's type.
fn result_columns(algorithm: Algorithm, key: &DataType) -> Vec<ColumnDesc> {
    let typed = |o: Output| match o {
        Output::Real => double(),
        Output::Count => bigint(),
        Output::Key => key.clone(),
    };
    algorithm.result().iter().map(|&(n, o)| column(n.to_owned(), typed(o))).collect()
}

/// That the key columns of `tables`, each's first, the edges' first two,
/// are comparable with the edges' source's, `key`.
fn keys_comparable(algorithm: Algorithm, tables: &[BoundTable], key: &DataType) -> Result<()> {
    let ends = tables.iter().enumerate().flat_map(|(k, t)| t.columns.iter().take(if k == 0 { ENDS } else { 1 }));
    for c in ends {
        if !key.comparable(&c.data_type) {
            return Err(refused(
                algorithm,
                &format!("column {} is of {}, which is not comparable with the source's {key}", c.name, c.data_type),
            ));
        }
    }
    Ok(())
}

/// The invocation of a function of GRAPH_ALGO `inv`, of the correlation
/// name `correlation` and derived column list `names` if written, bound
/// in `b`, nested `depth` deep.
///
/// # Errors
///
/// SQLCODE -103 if GRAPH_ALGO has no function of its name, an argument is
/// not of a parameter, two are of one, one is of the wrong kind, the
/// edges or START is missing, or a key column is not comparable with the
/// edges' source; as `catalogued::bound` of a graph's name.
pub(super) fn bound_graph_algorithm(
    b: Bounds<'_>,
    (inv, correlation, names): (&PtfInvocation, Option<&str>, Option<&[String]>),
    depth: usize,
) -> Result<BoundPtf> {
    let algorithm = Algorithm::named(&inv.name)
        .ok_or_else(|| rule(format!("[EXTENSION] there is no graph algorithm {SCHEMA}.{}", inv.name)))
        .or_at(inv.at)?;
    if !inv.copartition.is_empty() {
        return Err(refused(algorithm, "its arguments have no co-partition clause")).or_at(inv.at);
    }
    let placed = placed(algorithm, inv).or_at(inv.at)?;
    let Inputs { tables, key, edge_columns, shape } = inputs(b, algorithm, &placed, depth).or_at(inv.at)?;
    let (parameters, arguments) = arguments(b, algorithm, &placed, &key).or_at(inv.at)?;
    let mut columns = result_columns(algorithm, &key);
    if let Some(n) = names {
        renamed(&mut columns, n).or_at(inv.at)?;
    }
    let call = GraphCall {
        name: QualifiedName { auth: SCHEMA.to_owned(), ident: algorithm.name().to_owned() },
        algorithm,
        parameters,
        vertices: tables.len() > 1,
        edge_columns,
        shape,
        user: b.cx.auth.to_owned(),
    };
    let node_names = columns.iter().map(|c| c.name.clone()).collect();
    let inputs = tables.into_iter().map(|t| t.plan).collect();
    let plan =
        LogicalPlan::Polymorphic(Box::new(Polymorphic { call: Arc::new(Invoked::Graph(call)), arguments, inputs, names: node_names }));
    let c = correlation.unwrap_or(algorithm.name()).to_owned();
    let ranges = vec![Range { correlation: Some(c), table: None, at: inv.at, columns: (0..columns.len()).collect() }];
    Ok(BoundPtf { plan, columns, ranges })
}

/// The table arguments of an invocation, bound: the edges' then the
/// vertices' if given, or a property graph's element tables.
struct Inputs {
    /// Their plans' tables.
    tables: Vec<BoundTable>,
    /// The type of a vertex.
    key: DataType,
    /// The names of the edges' columns, of which COST names one.
    edge_columns: Vec<String>,
    /// Of a property graph, how its tables' rows are its edges and vertices.
    shape: Option<GraphShape>,
}

/// The table arguments `placed` of the parameters of `algorithm`, bound,
/// nested `depth` deep.
fn inputs<'a>(b: Bounds<'_>, algorithm: Algorithm, placed: &[Option<&'a PtfArgument>], depth: usize) -> Result<Inputs> {
    let given = |a: Option<&'a PtfArgument>| a.filter(|a| !matches!(a, PtfArgument::Null(_)));
    let written: Vec<(&Parameter, Option<&PtfArgument>)> = algorithm
        .parameters()
        .iter()
        .zip(placed)
        .filter(|(p, _)| matches!(p.kind, Kind::Edges | Kind::Vertices))
        .map(|(p, a)| (p, given(*a)))
        .collect();
    let mut tables = Vec::new();
    for &(p, a) in &written {
        match (a, p.kind) {
            (Some(PtfArgument::Value(v)), Kind::Edges) => {
                let Some(g) = catalogued::graph(b, v) else {
                    return Err(refused(algorithm, &format!("{} names no property graph", p.name)));
                };
                if written.iter().any(|(q, w)| q.kind == Kind::Vertices && w.is_some()) {
                    return Err(refused(algorithm, "a property graph's vertices are its vertex tables' rows, so it takes no VERTICES"));
                }
                let c = catalogued::bound(b, algorithm, &g, depth)?;
                return Ok(Inputs { tables: c.tables, key: c.key, edge_columns: c.edge_columns, shape: Some(c.shape) });
            }
            (Some(a), _) => tables.push(table(b, (algorithm, p), a, depth)?),
            (None, Kind::Edges) => return Err(refused(algorithm, "it takes the edges, its first argument")),
            (None, Kind::Vertices | Kind::Start | Kind::Number | Kind::Count | Kind::Cost) => {}
        }
    }
    let key = key_type(&tables);
    keys_comparable(algorithm, &tables, &key)?;
    let edge_columns = tables.first().map(|t| t.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
    Ok(Inputs { tables, key, edge_columns, shape: None })
}

/// What `arguments` binds: the value parameters' names; their arguments.
type Arguments = (Vec<&'static str>, Vec<Expr>);

/// The value arguments `placed` of the parameters of `algorithm`, bound,
/// `key` the type of a vertex.
fn arguments<'a>(b: Bounds<'_>, algorithm: Algorithm, placed: &[Option<&'a PtfArgument>], key: &DataType) -> Result<Arguments> {
    let (mut parameters, mut values) = (Vec::new(), Vec::new());
    let given = |a: Option<&'a PtfArgument>| a.filter(|a| !matches!(a, PtfArgument::Null(_)));
    for (p, a) in algorithm.parameters().iter().zip(placed).filter(|(p, _)| !matches!(p.kind, Kind::Edges | Kind::Vertices)) {
        let expr = match (given(*a), p.kind == Kind::Start) {
            (Some(PtfArgument::Value(v)), _) => value(b, (algorithm, p), v, declared(p.kind, key))?,
            (Some(PtfArgument::Table(_) | PtfArgument::Descriptor(..) | PtfArgument::Null(_)), _) => {
                return Err(refused(algorithm, &format!("{} takes a value", p.name)));
            }
            (None, true) => return Err(refused(algorithm, "it takes START, the vertex it starts from")),
            (None, false) => Expr::Const(Value::Null),
        };
        parameters.push(p.name);
        values.push(expr);
    }
    Ok((parameters, values))
}
