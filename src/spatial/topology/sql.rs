//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.1, §11.2] Where the rows
//! of a topology are kept, and how its routines read and write them.
//!
//! A topology is a schema of the topology's name [§11.1.1]. It holds the
//! views ST_NODE, ST_EDGE and ST_FACE of §11.1.2 to §11.1.4, and under
//! them three base tables of its own, ST_TOPO_NODE, ST_TOPO_EDGE and
//! ST_TOPO_FACE: the base tables of §11.2 without their TOPOLOGY column,
//! as one schema holds one topology. The text gives §11.2 as a model only
//! [§11.2.1] and lets an implementation define more objects of a topology
//! [§11.1.1]. The routines read the views, with the privileges of the
//! user who invokes them, and write the base tables with those of the
//! schema's owner, as the views "are updated only by the topology
//! functions provided" [§11.1.1].
//!
//! The name is the schema's name as it is written, a delimited
//! identifier, as `INFORMATION_SCHEMA.SCHEMATA` holds it as given
//! [§11.3.17 Description 2b].

use super::{Change, Changes, Edge, Face, Node, Topology};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::ProcedureContext;
use crate::geometry::model::Geom;
use crate::value::{DataType, Decimal, Spelled, Value};

pub(super) mod routines;
pub(super) mod tables;

pub use routines::procedures;
pub use tables::tables;

/// [§11.3 Definitional Rule 1] ST_MaxTopologyName, the most characters
/// of a topology's name: the most an identifier has.
pub const MAX_TOPOLOGY_NAME: usize = crate::lexer::MAX_IDENTIFIER_LENGTH;

/// The type INTEGER.
pub(super) const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The type of a topology's name, CHARACTER VARYING(ST_MaxTopologyName).
pub(super) fn name_type() -> DataType {
    DataType::varying(u32::try_from(MAX_TOPOLOGY_NAME).unwrap_or(u32::MAX))
}

/// A fault of rule `rule` of SQLSTATE `state`.
pub(super) fn fault(rule: &str, state: SqlState, detail: &str) -> SqlError {
    super::fault(rule, state, detail)
}

/// [§11.3 Description 2a] That argument `v` is not null.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is.
pub(super) fn given<'v>(v: Option<&'v Value>, rule: &str) -> Result<&'v Value> {
    v.filter(|v| !v.is_null()).ok_or_else(|| fault(rule, SqlState::SPATIAL_NULL_ARGUMENT, "an argument is null"))
}

/// [§11.3 Description 2a, 2b] The topology's name `v`.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is null; 2FF62 (invalid topology
/// name) if it is empty or longer than ST_MaxTopologyName, so no schema
/// has it.
pub(super) fn topology_name(v: Option<&Value>, rule: &str) -> Result<String> {
    let Value::Char(name) = given(v, &format!("{rule} 2a"))? else {
        return Err(fault(&format!("{rule} 2b"), SqlState::SPATIAL_INVALID_TOPOLOGY_NAME, "the name is no character string"));
    };
    if name.is_empty() || name.chars().count() > MAX_TOPOLOGY_NAME {
        return Err(fault(&format!("{rule} 2b"), SqlState::SPATIAL_INVALID_TOPOLOGY_NAME, &format!("'{name}' is no valid schema name")));
    }
    Ok(name.clone())
}

/// The integer `v`, none if it is null.
///
/// # Errors
///
/// SQLCODE -301 if it is no integer of 32 bits.
pub(super) fn integer(v: Option<&Value>) -> Result<Option<i32>> {
    let bad = || SqlError::new(Code::Data, "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3] a value is no INTEGER");
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

/// The integer argument `v`, which is not null.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is null; SQLCODE -301 if it is no
/// integer.
pub(super) fn required(v: Option<&Value>, rule: &str) -> Result<i32> {
    integer(Some(given(v, rule)?))?.ok_or_else(|| fault(rule, SqlState::SPATIAL_NULL_ARGUMENT, "an argument is null"))
}

/// The geometry argument `v`, which is not null.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is null; -301 if it is no
/// geometry value.
pub(super) fn geometry(v: Option<&Value>, rule: &str) -> Result<Geom> {
    Geom::from_value(given(v, rule)?)
}

/// The value of integer `n`.
pub(super) fn int_value(n: i32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The value of integer `n`, or the null value.
fn optional(n: Option<i32>) -> Value {
    n.map_or(Value::Null, int_value)
}

/// `name` as a delimited identifier.
pub(super) fn delimited(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The query of the rows of view `view` of topology `name`, its columns
/// `columns`, each row one value of a row type, as a multiset of rows is
/// of degree one [ISO/IEC 9075-2:2023 §6.41 SR1].
fn query(name: &str, view: &str, columns: &str) -> String {
    format!("SELECT ROW({columns}) FROM {}.{view}", delimited(name))
}

/// [§11.1.2 to §11.1.4] The queries of the three views of topology
/// `name`: ST_NODE, ST_EDGE and ST_FACE.
pub(super) fn reads(name: &str) -> Vec<String> {
    vec![
        query(name, "ST_NODE", "NODE_ID, GEOMETRY, CONTAINING_FACE"),
        query(name, "ST_EDGE", "EDGE_ID, START_NODE, END_NODE, NEXT_LEFT_EDGE, NEXT_RIGHT_EDGE, LEFT_FACE, RIGHT_FACE, GEOMETRY"),
        query(name, "ST_FACE", "FACE_ID, MBR"),
    ]
}

/// [§11.3 Description 2b, 2c] What a failure to read a topology's views
/// is: SQLSTATE 2FF63 (topology privilege denied) of a privilege the user
/// lacks; 2FF62 (invalid topology name) of a view that is not there,
/// whose name is so no topology's. Reading: the text's 2c applies where
/// the view is there, as no privilege is granted on what is not.
pub fn read_failed(e: SqlError) -> SqlError {
    let rule = "§11.3 Description 2";
    if e.code == Code::Privilege {
        fault(&format!("{rule}c"), SqlState::SPATIAL_TOPOLOGY_PRIVILEGE_DENIED, &e.msg)
    } else if e.code == Code::SyntaxRule {
        fault(&format!("{rule}b"), SqlState::SPATIAL_INVALID_TOPOLOGY_NAME, &e.msg)
    } else {
        e
    }
}

/// The value at `k` of `row`.
fn at(row: &[Value], k: usize) -> &Value {
    row.get(k).unwrap_or(&Value::Null)
}

/// The identifier at `k` of `row`, which is not null.
fn id(row: &[Value], k: usize) -> Result<i32> {
    integer(Some(at(row, k)))?.ok_or_else(|| super::damaged("a topology's identifier is null"))
}

/// The topology of the rows of its three views, `read`.
///
/// # Errors
///
/// SQLCODE -916 if a row is not one of a view.
pub fn from_rows(read: &[Vec<Vec<Value>>]) -> Result<Topology> {
    let [nodes, edges, faces] = read else { return Err(super::damaged("a topology is read of three views")) };
    let mut topo = Topology::default();
    for r in nodes {
        let geometry = Geom::from_value(at(r, 1))?;
        topo.srid.get_or_insert(geometry.srid);
        topo.nodes.insert(id(r, 0)?, Node { geometry, containing_face: integer(Some(at(r, 2)))? });
    }
    for r in edges {
        let geometry = Geom::from_value(at(r, 7))?;
        topo.srid.get_or_insert(geometry.srid);
        let (start, end, next_left, next_right) = (id(r, 1)?, id(r, 2)?, id(r, 3)?, id(r, 4)?);
        let edge = Edge { start, end, next_left, next_right, left_face: id(r, 5)?, right_face: id(r, 6)?, geometry };
        topo.edges.insert(id(r, 0)?, edge);
    }
    for r in faces {
        let v = at(r, 1);
        let mbr = if v.is_null() { None } else { Some(Geom::from_value(v)?) };
        topo.faces.insert(id(r, 0)?, Face { mbr });
    }
    Ok(topo)
}

/// Topology `name`, read in `cx`.
///
/// # Errors
///
/// As `read_failed`; SQLCODE -916 of a row that is not one of a view.
pub(super) fn load(cx: &mut dyn ProcedureContext, name: &str) -> Result<Topology> {
    let read = reads(name).iter().map(|q| cx.rows(q, &[]).map_err(read_failed)).collect::<Result<Vec<_>>>()?;
    from_rows(&read.into_iter().map(|rows| rows.into_iter().map(fields).collect()).collect::<Vec<_>>())
}

/// The fields of `row`, a query's row of one value of a row type.
fn fields(row: Vec<Value>) -> Vec<Value> {
    match row.as_slice() {
        [Value::Row(fields)] => fields.to_vec(),
        _ => row,
    }
}

/// The base table of topology `name` under view `view`.
fn base(name: &str, table: &str) -> String {
    format!("{}.{table}", delimited(name))
}

/// The statements that make `changes` to the base tables of topology
/// `name`, in an order no foreign key refuses: faces made, nodes and edges
/// made or changed, then edges, nodes and faces deleted.
fn statements(name: &str, changes: Changes) -> Vec<(String, Vec<Value>)> {
    let (face, node, edge) = (base(name, "ST_TOPO_FACE"), base(name, "ST_TOPO_NODE"), base(name, "ST_TOPO_EDGE"));
    let mut made = Vec::new();
    let mut gone = Vec::new();
    for c in changes.faces {
        match c {
            Change::Insert(id, f) => made.push((format!("INSERT INTO {face} VALUES (?, ?)"), vec![int_value(id), mbr(&f)])),
            Change::Update(id, f) => made.push((format!("UPDATE {face} SET MBR = ? WHERE FACE_ID = ?"), vec![mbr(&f), int_value(id)])),
            Change::Delete(id) => gone.push((format!("DELETE FROM {face} WHERE FACE_ID = ?"), vec![int_value(id)])),
        }
    }
    let mut nodes_gone = Vec::new();
    for c in changes.nodes {
        match c {
            Change::Insert(id, n) => made.push((format!("INSERT INTO {node} VALUES (?, ?, ?)"), node_row(id, &n))),
            Change::Update(id, n) => {
                let sql = format!("UPDATE {node} SET GEOMETRY = ?, CONTAINING_FACE = ? WHERE NODE_ID = ?");
                made.push((sql, vec![n.geometry.to_value(), optional(n.containing_face), int_value(id)]));
            }
            Change::Delete(id) => nodes_gone.push((format!("DELETE FROM {node} WHERE NODE_ID = ?"), vec![int_value(id)])),
        }
    }
    let mut edges_gone = Vec::new();
    for c in changes.edges {
        match c {
            Change::Insert(id, e) => made.push((format!("INSERT INTO {edge} VALUES (?, ?, ?, ?, ?, ?, ?, ?)"), edge_row(id, &e))),
            Change::Update(id, e) => {
                let sql = format!(
                    "UPDATE {edge} SET START_NODE = ?, END_NODE = ?, NEXT_LEFT_EDGE = ?, NEXT_RIGHT_EDGE = ?, LEFT_FACE = ?, RIGHT_FACE = ?, GEOMETRY = ? WHERE EDGE_ID = ?"
                );
                let mut row = edge_row(id, &e);
                row.rotate_left(1);
                made.push((sql, row));
            }
            Change::Delete(id) => edges_gone.push((format!("DELETE FROM {edge} WHERE EDGE_ID = ?"), vec![int_value(id)])),
        }
    }
    made.into_iter().chain(edges_gone).chain(nodes_gone).chain(gone).collect()
}

/// The MBR of `f`, or the null value.
fn mbr(f: &Face) -> Value {
    f.mbr.as_ref().map_or(Value::Null, Geom::to_value)
}

/// The row of node `id`.
fn node_row(id: i32, n: &Node) -> Vec<Value> {
    vec![int_value(id), n.geometry.to_value(), optional(n.containing_face)]
}

/// The row of edge `id`.
fn edge_row(id: i32, e: &Edge) -> Vec<Value> {
    let ids = [id, e.start, e.end, e.next_left, e.next_right, e.left_face, e.right_face];
    ids.into_iter().map(int_value).chain([e.geometry.to_value()]).collect()
}

/// Topology `name`, read as `before`, written as `after`, in `cx`, with
/// the privileges of the owner of its schema.
///
/// # Errors
///
/// A statement's.
pub(super) fn save(cx: &mut dyn ProcedureContext, name: &str, (before, after): (&Topology, &Topology)) -> Result<()> {
    for (sql, args) in statements(name, before.changes(after)) {
        cx.run_owned(name, &sql, &args)?;
    }
    Ok(())
}

/// [§11.3.17 Description 2c] The statements that make topology `name`:
/// its schema, its base tables and views, and its universal face.
pub(super) fn init_statements(name: &str) -> Vec<String> {
    let schema = delimited(name);
    let (face, node, edge) = ("ST_TOPO_FACE", "ST_TOPO_NODE", "ST_TOPO_EDGE");
    let definition = format!(
        "CREATE SCHEMA {schema} \
         CREATE TABLE {face} (FACE_ID INTEGER NOT NULL PRIMARY KEY, MBR ST_POLYGON) \
         CREATE TABLE {node} (NODE_ID INTEGER NOT NULL PRIMARY KEY, GEOMETRY ST_POINT NOT NULL, \
         CONTAINING_FACE INTEGER REFERENCES {face} (FACE_ID)) \
         CREATE TABLE {edge} (EDGE_ID INTEGER NOT NULL PRIMARY KEY, \
         START_NODE INTEGER NOT NULL REFERENCES {node} (NODE_ID), END_NODE INTEGER NOT NULL REFERENCES {node} (NODE_ID), \
         NEXT_LEFT_EDGE INTEGER NOT NULL, NEXT_RIGHT_EDGE INTEGER NOT NULL, \
         LEFT_FACE INTEGER NOT NULL REFERENCES {face} (FACE_ID), RIGHT_FACE INTEGER NOT NULL REFERENCES {face} (FACE_ID), \
         GEOMETRY ST_CURVE NOT NULL) \
         CREATE VIEW ST_NODE AS SELECT NODE_ID, GEOMETRY, CONTAINING_FACE FROM {node} \
         CREATE VIEW ST_EDGE AS SELECT EDGE_ID, START_NODE, END_NODE, NEXT_LEFT_EDGE, NEXT_RIGHT_EDGE, \
         LEFT_FACE, RIGHT_FACE, GEOMETRY FROM {edge} \
         CREATE VIEW ST_FACE AS SELECT FACE_ID, MBR FROM {face}"
    );
    vec![definition, format!("INSERT INTO {schema}.{face} VALUES (0, NULL)")]
}
