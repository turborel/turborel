//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1, §13.1.2]
//! ST_ShortestUndPath and ST_ShortestDirPath, table functions written in
//! Rust (RULES.md D-SPATIAL-ROUTINES). The table and columns they name
//! are known as the invocation is bound, a literal or a parameter, and are
//! read by a query of the invoker's privileges; the start and end points
//! may be any values.
//!
//! Readings. The table's name is read as a `<table name>` [Description
//! 2a], so it folds as SQL text does; a column's name is the name the
//! catalog holds, as a value of SQL_IDENTIFIER is, so it does not fold. A
//! path's nodes are the start and end points of its curve, compared in x
//! and y as ST_Equals compares points. A row whose identifier, geometry,
//! start point or weight is null is no path, as one whose geometry is no
//! ST_Curve is not [Description 8]. A weight that is negative, infinite
//! or no number is 2FF02 (invalid argument). Without a weight column a
//! path weighs its curve's length in the coordinates' own unit, which is
//! the system's linear unit where it has one [Description 6]. Of
//! ST_ShortestDirPath, a path is walked from the end of its curve that is
//! its start point to the other; a start point at neither end is 2FF02.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::{Edge, Graph, shortest};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::TableFunction;
use crate::geometry::data_type_of;
use crate::geometry::model::{Body, Geom, Kind};
use crate::lexer::MAX_IDENTIFIER_LENGTH;
use crate::parser::Parser;
use crate::spatial::curves::adapt::from_model;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::observe::path_length;
use crate::spatial::curves::path::path;
use crate::spatial::curves::piece::Metric;
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, Spelled, Value};

/// A routine of §13.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortestPath {
    /// [§13.1.1] ST_ShortestUndPath: each path walked either way.
    Undirected,
    /// [§13.1.2] ST_ShortestDirPath: each path walked from its start point.
    Directed,
}

/// The routines, built in.
pub fn tables() -> Vec<Arc<dyn TableFunction>> {
    [ShortestPath::Undirected, ShortestPath::Directed].into_iter().map(|r| -> Arc<dyn TableFunction> { Arc::new(r) }).collect()
}

/// [§13.1.1 Definitional Rule 2] DT1's implementation-defined length: a
/// name of three identifiers, each delimited with its quotes doubled, and
/// two periods.
pub const TABLE_NAME_LENGTH: usize = 3 * (2 * MAX_IDENTIFIER_LENGTH + 2) + 2;

/// [Definitional Rule 3] DT2's length, not less than 128.
pub const COLUMN_NAME_LENGTH: usize = MAX_IDENTIFIER_LENGTH;

/// [§13.1.1] The fewest arguments of ST_ShortestUndPath, of its form of
/// no weight column; ST_ShortestDirPath takes one more, a start point
/// column. Each takes one more again with a weight column.
const UNDIRECTED_LEAST: usize = 5;

/// The letters of the items of a Description, by an argument's position.
const LETTERS: [&str; 7] = ["a", "b", "c", "d", "e", "f", "g"];

/// The type INTEGER.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The type DOUBLE PRECISION.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };

impl ShortestPath {
    /// The subclause of its Description.
    fn rule(self) -> &'static str {
        match self {
            Self::Undirected => "§13.1.1 Description",
            Self::Directed => "§13.1.2 Description",
        }
    }

    /// How many names it takes with `count` arguments, and whether one is
    /// of a weight column; none if it takes no such count.
    fn names(self, count: usize) -> Option<(usize, bool)> {
        let least = match self {
            Self::Undirected => UNDIRECTED_LEAST,
            Self::Directed => UNDIRECTED_LEAST.saturating_add(1),
        };
        let weighted = count == least.saturating_add(1);
        (count == least || weighted).then(|| (count.saturating_sub(2), weighted))
    }
}

/// An exception of SQL/MM Spatial: `state` of the rule `rule`.
fn fault(rule: &str, state: SqlState, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {rule}] {detail}")).with_state(state)
}

/// `name` as a delimited identifier.
fn delimited(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// [Description 2a] The table named by `v`, trimmed of spaces, as a
/// delimited name.
///
/// # Errors
///
/// SQLSTATE 2FF02 (invalid argument) if it is null or no `<table name>`.
fn table_name(v: &Value, rule: &str) -> Result<String> {
    let bad = || fault(rule, SqlState::SPATIAL_INVALID_ARGUMENT, "the table's name is no <table name>");
    let Value::Char(s) = v else { return Err(bad()) };
    let name = Parser::new(s.trim_matches(' ')).and_then(|mut p| p.only_table_name()).map_err(|_| bad())?;
    Ok(match name.auth {
        Some(schema) => format!("{}.{}", delimited(&schema), delimited(&name.ident)),
        None => delimited(&name.ident),
    })
}

/// [Description 2b to 2e] The column named by `v`, as a delimited name.
///
/// # Errors
///
/// SQLSTATE 2FF02 (invalid argument) if it is null or empty.
fn column_name(v: &Value, rule: &str) -> Result<String> {
    let bad = || fault(rule, SqlState::SPATIAL_INVALID_ARGUMENT, "a column's name is no name");
    let Value::Char(s) = v else { return Err(bad()) };
    if s.is_empty() { Err(bad()) } else { Ok(delimited(s)) }
}

impl TableFunction for ShortestPath {
    fn name(&self) -> &str {
        match self {
            Self::Undirected => "ST_SHORTESTUNDPATH",
            Self::Directed => "ST_SHORTESTDIRPATH",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        let (names, _) = self.names(count)?;
        let table = DataType::varying(u32::try_from(TABLE_NAME_LENGTH).unwrap_or(u32::MAX));
        let column = DataType::varying(u32::try_from(COLUMN_NAME_LENGTH).unwrap_or(u32::MAX));
        let columns = std::iter::repeat_n(column, names.saturating_sub(1));
        Some(std::iter::once(table).chain(columns).chain([data_type_of(Kind::Point), data_type_of(Kind::Point)]).collect())
    }

    fn columns(&self) -> Vec<(String, DataType)> {
        vec![("SHORTEST_PATH".to_owned(), DataType::array(INTEGER, MAX_ARRAY_CARDINALITY)), ("TOTAL_WEIGHT".to_owned(), DOUBLE)]
    }

    /// The query of the paths' rows: the identifier, the geometry, then
    /// the start point and the weight where they are named.
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        let (names, _) = self.names(known.len()).unwrap_or_default();
        let rule = format!("{} 2a", self.rule());
        let mut given = Vec::new();
        for v in known.iter().take(names) {
            let Some(v) = v else {
                let msg = format!(
                    "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {rule}] the table's and columns' names are not known as the invocation is bound: give literals or parameters"
                );
                return Err(SqlError::new(Code::SyntaxRule, msg));
            };
            given.push(*v);
        }
        let Some((table, columns)) = given.split_first() else { return Ok(Vec::new()) };
        let table = table_name(table, &rule)?;
        let columns = columns.iter().map(|c| column_name(c, &rule)).collect::<Result<Vec<_>>>()?;
        Ok(vec![format!("SELECT ROW({}) FROM {table}", columns.join(", "))])
    }

    /// [Description 2a to 2e] A table or column that is not there is
    /// SQLSTATE 2FF02 (invalid argument).
    fn read_failed(&self, e: SqlError) -> SqlError {
        if e.code == Code::SyntaxRule { fault(&format!("{} 2", self.rule()), SqlState::SPATIAL_INVALID_ARGUMENT, &e.msg) } else { e }
    }

    fn rows(&self, args: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let (names, weighted) = self.names(args.len()).unwrap_or_default();
        let rows = read.first().map(Vec::as_slice).unwrap_or_default();
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let shape = Shape { directed: *self == Self::Directed, weighted, rule: self.rule() };
        let (graph, nodes) = graph_of(rows, shape)?;
        let node = |k: usize| -> Result<usize> {
            let key = args.get(k).and_then(|v| Geom::from_value(v).ok()).and_then(|g| point_key(&g));
            key.and_then(|key| nodes.get(&key).copied()).ok_or_else(|| shape.invalid(k, "the point is no end of a path's curve"))
        };
        let (start, end) = (node(names)?, node(names.saturating_add(1))?);
        let Some(found) = shortest(&graph, start, end)? else { return Ok(Vec::new()) };
        found.paths.into_iter().map(|p| result_row(p, found.total)).collect()
    }
}

/// The row of path `ids` of total weight `total`.
///
/// # Errors
///
/// SQLSTATE 2202F if the path has more edges than the array holds.
fn result_row(ids: Vec<i32>, total: f64) -> Result<Vec<Value>> {
    if ids.len() > usize::try_from(MAX_ARRAY_CARDINALITY).unwrap_or(usize::MAX) {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §9.2 GR3] a shortest path of {} paths is longer than an array of {MAX_ARRAY_CARDINALITY}",
            ids.len()
        );
        return Err(SqlError::new(Code::Data, msg).with_state(SqlState::ARRAY_DATA_RIGHT_TRUNCATION));
    }
    let array: Vec<Value> = ids.into_iter().map(|id| Value::Exact(Decimal::new(i128::from(id), 0))).collect();
    Ok(vec![Value::Array(array.into()), Value::Approx(total)])
}

/// What the rows of a path table hold, and of which routine.
#[derive(Debug, Clone, Copy)]
struct Shape {
    /// Whether each row has a start point, after its geometry.
    directed: bool,
    /// Whether each row has a weight, last.
    weighted: bool,
    /// The routine's Description.
    rule: &'static str,
}

impl Shape {
    /// The item of the Description that describes argument `k`: of its
    /// form of a weight column, Description 2, else 3.
    fn item(self, k: usize) -> String {
        let section = if self.weighted { "2" } else { "3" };
        format!("{} {section}{}", self.rule, LETTERS.get(k).copied().unwrap_or_default())
    }

    /// The exception of an argument `k` that is not what it must be.
    fn invalid(self, k: usize, what: &str) -> SqlError {
        fault(&self.item(k), SqlState::SPATIAL_INVALID_ARGUMENT, what)
    }
}

/// The bits of x and y of a position, negative zero made positive, by
/// which two positions are one node.
fn key(x: f64, y: f64) -> [u64; 2] {
    [(x + 0.0_f64).to_bits(), (y + 0.0_f64).to_bits()]
}

/// The key of point `g`, none if it is empty or no point.
fn point_key(g: &Geom) -> Option<[u64; 2]> {
    let Body::Point(c) = &g.body else { return None };
    Some(key(*c.first()?, *c.get(1)?))
}

/// One path of the table: its identifier, the keys of the nodes it is
/// walked from and to, and its weight.
type PathRow = (i32, [u64; 2], [u64; 2], f64);

/// The graph of the paths of `rows`, and each node's number by its key.
///
/// # Errors
///
/// SQLSTATE 2FF05 (duplicate value) if two rows have one identifier
/// [Description 7]; as `path_of`.
fn graph_of(rows: &[Vec<Value>], shape: Shape) -> Result<(Graph, BTreeMap<[u64; 2], usize>)> {
    let mut ids = BTreeSet::new();
    let mut paths = Vec::new();
    for row in rows {
        let id = identifier(row.first(), shape)?;
        if let Some(id) = id
            && !ids.insert(id)
        {
            return Err(fault(&format!("{} 7", shape.rule), SqlState::SPATIAL_DUPLICATE_VALUE, &format!("two paths have identifier {id}")));
        }
        paths.extend(path_of(id, row, shape)?);
    }
    let mut nodes = BTreeMap::new();
    let mut graph = Graph::default();
    for (id, from, to, weight) in paths {
        let mut number = |k: [u64; 2]| {
            let next = nodes.len();
            *nodes.entry(k).or_insert(next)
        };
        let (from, to) = (number(from), number(to));
        graph.edges.push(Edge { from, to, weight, id });
        if !shape.directed {
            graph.edges.push(Edge { from: to, to: from, weight, id });
        }
    }
    graph.nodes = nodes.len();
    Ok((graph, nodes))
}

/// The identifier `v`, none if it is null.
///
/// # Errors
///
/// SQLSTATE 2FF02 (invalid argument) if it is no INTEGER value.
fn identifier(v: Option<&Value>, shape: Shape) -> Result<Option<i32>> {
    let bad = || shape.invalid(1, "a path's identifier is no INTEGER");
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Exact(d)) => {
            if d.scale != 0 {
                return Err(bad());
            }
            i32::try_from(d.units).map(Some).map_err(|_| bad())
        }
        Some(_) => Err(bad()),
    }
}

/// [Description 8] The path of row `row`, of identifier `id`; none if any
/// of its values is null or its geometry is no curve, or an empty one.
///
/// # Errors
///
/// SQLSTATE 2FF02 (invalid argument) of a geometry that is no geometry
/// value, a start point at neither end of the curve, or a weight that is
/// no number, negative or infinite; a curve's that is not well formed.
fn path_of(id: Option<i32>, row: &[Value], shape: Shape) -> Result<Option<PathRow>> {
    // The positions of the arguments naming the geometry, start point and
    // weight columns.
    let (geometry, start, weight_at) = (2, 3, if shape.directed { 4 } else { 3 });
    let present = |k: usize| row.get(k).filter(|value| !value.is_null());
    let (Some(id), Some(stored)) = (id, present(1)) else { return Ok(None) };
    let geom = Geom::from_value(stored).map_err(|_| shape.invalid(geometry, "a path's geometry is no ST_Geometry value"))?;
    let Body::Curve(curve) = &geom.body else { return Ok(None) };
    let Some(traced) = path(&from_model(curve, geom.dims), &Planar)? else { return Ok(None) };
    let (mut from, mut to) = (key(traced.start.x, traced.start.y), key(traced.end.x, traced.end.y));
    if shape.directed {
        let Some(point) = present(2) else { return Ok(None) };
        let at = Geom::from_value(point)
            .ok()
            .and_then(|g| point_key(&g))
            .ok_or_else(|| shape.invalid(start, "a path's start point is no point"))?;
        if at == to && at != from {
            std::mem::swap(&mut from, &mut to);
        } else if at != from {
            return Err(shape.invalid(start, "a path's start point is at neither end of its curve"));
        }
    }
    let weight = if shape.weighted {
        let Some(stored_weight) = row.last().filter(|value| !value.is_null()) else { return Ok(None) };
        stored_weight.as_f64().ok_or_else(|| shape.invalid(weight_at, "a path's weight is no number"))?
    } else {
        path_length(&traced, Metric::Plane, &Planar)?
    };
    if !(weight.is_finite() && weight >= 0.0_f64) {
        return Err(shape.invalid(weight_at, "a path's weight is negative or not finite"));
    }
    Ok(Some((id, from, to, weight)))
}
