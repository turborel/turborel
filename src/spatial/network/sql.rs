//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.1, §12.2] Where the rows
//! of a network are kept, and how its routines read and write them.
//!
//! A network is a schema of the network's name [§12.1.1]. It holds the
//! views ST_NODE and ST_LINK of §12.1.2 and §12.1.3, and under them two
//! base tables of its own, ST_NET_NODE and ST_NET_LINK: the base tables
//! of §12.2 without their NETWORK column, as one schema holds one network,
//! with §12.2.3's foreign keys from a link to its nodes. The text gives
//! §12.2 as a model only [§12.2.1] and lets an implementation define more
//! objects of a network [§12.1.1]. This is the shape a topology of §11
//! has. The routines read the views, with the privileges of the user who
//! invokes them, and write the base tables with those of the schema's
//! owner, as the views "are updated only by the topology functions
//! provided" [§12.1.1].
//!
//! The name is the schema's name as it is written, a delimited
//! identifier, as `INFORMATION_SCHEMA.SCHEMATA` holds it as given
//! [§12.3.7 Description 2b].

use super::{Change, Changes, Link, Network, damaged, fault};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::ProcedureContext;
use crate::geometry::model::Geom;
use crate::value::{DataType, Decimal, Spelled, Value};

pub(super) mod routines;
pub(super) mod tables;

pub use routines::procedures;
pub use tables::tables;

/// [§12.3 Definitional Rule 1] ST_MaxNetworkName, the most characters of
/// a network's name: the most an identifier has.
pub const MAX_NETWORK_NAME: usize = crate::lexer::MAX_IDENTIFIER_LENGTH;

/// The base table under the view ST_NODE.
pub const NODE_TABLE: &str = "ST_NET_NODE";
/// The base table under the view ST_LINK.
pub const LINK_TABLE: &str = "ST_NET_LINK";

/// The type INTEGER.
pub(super) const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// The type of a network's name, CHARACTER VARYING(ST_MaxNetworkName).
pub(super) fn name_type() -> DataType {
    DataType::varying(u32::try_from(MAX_NETWORK_NAME).unwrap_or(u32::MAX))
}

/// [§12.3 Description 2a] That argument `v` is not null.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is.
pub(super) fn given<'v>(v: Option<&'v Value>, rule: &str) -> Result<&'v Value> {
    v.filter(|v| !v.is_null()).ok_or_else(|| fault(rule, SqlState::SPATIAL_NULL_ARGUMENT, "an argument is null"))
}

/// [§12.3 Description 2a, 2b] The schema name `v`, whose exception of a
/// null value is of rule `rules.0` and of a name no schema may have is
/// `invalid` of rule `rules.1`.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is null; `invalid` if it is empty
/// or longer than an identifier, so no schema has it.
pub(super) fn schema_name(v: Option<&Value>, rules: (&str, &str), invalid: SqlState) -> Result<String> {
    let Value::Char(name) = given(v, rules.0)? else {
        return Err(fault(rules.1, invalid, "the name is no character string"));
    };
    if name.is_empty() || name.chars().count() > MAX_NETWORK_NAME {
        return Err(fault(rules.1, invalid, &format!("'{name}' is no valid schema name")));
    }
    Ok(name.to_string())
}

/// [§12.3 Description 2a, 2b] The network's name `v`, of the routine
/// whose Description is `rule`.
///
/// # Errors
///
/// SQLSTATE 2FF03 (null argument) if it is null; 2FF64 (invalid network
/// name) if no schema may have it.
pub(super) fn network_name(v: Option<&Value>, rule: &str) -> Result<String> {
    schema_name(v, (&format!("{rule} 2a"), &format!("{rule} 2b")), SqlState::SPATIAL_INVALID_NETWORK_NAME)
}

/// The integer `v`, none if it is null.
///
/// # Errors
///
/// SQLCODE -301 if it is no integer of 32 bits.
pub(super) fn integer(v: Option<&Value>) -> Result<Option<i32>> {
    let bad = || SqlError::new(Code::Data, "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.2] a value is no INTEGER");
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

/// The geometry `v`, none if it is null.
///
/// # Errors
///
/// SQLCODE -301 if it is no geometry value.
pub(super) fn optional_geometry(v: Option<&Value>) -> Result<Option<Geom>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Geom::from_value(v).map(Some),
    }
}

/// The value of integer `n`.
pub(super) fn int_value(n: i32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The value of geometry `g`, or the null value.
fn geometry_value(g: Option<&Geom>) -> Value {
    g.map_or(Value::Null, Geom::to_value)
}

/// `name` as a delimited identifier.
pub(super) fn delimited(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// [§12.1.2, §12.1.3] The queries of the two views of network `name`,
/// each row one value of a row type, as a multiset of rows is of degree
/// one [ISO/IEC 9075-2:2023 §6.41 SR1].
pub(super) fn reads(name: &str) -> Vec<String> {
    let schema = delimited(name);
    vec![
        format!("SELECT ROW(NODE_ID, GEOMETRY) FROM {schema}.ST_NODE"),
        format!("SELECT ROW(LINK_ID, START_NODE, END_NODE, GEOMETRY) FROM {schema}.ST_LINK"),
    ]
}

/// [§12.3 Description 2b, 2c] What a failure to read a network's views
/// is: SQLSTATE 2FF65 (network privilege denied) of a privilege the user
/// lacks; 2FF64 (invalid network name) of a view that is not there, whose
/// name is so no network's. Reading: 2c applies where the view is there,
/// as no privilege is granted on what is not.
pub fn read_failed(e: SqlError) -> SqlError {
    if e.code == Code::Privilege {
        fault("§12.3 Description 2c", SqlState::SPATIAL_NETWORK_PRIVILEGE_DENIED, &e.msg)
    } else if e.code == Code::SyntaxRule {
        fault("§12.3 Description 2b", SqlState::SPATIAL_INVALID_NETWORK_NAME, &e.msg)
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
    integer(Some(at(row, k)))?.ok_or_else(|| damaged("a network's identifier is null"))
}

/// The network of the rows of its two views, `read`: of nodes, the
/// identifier and geometry; of links, the identifier, the two nodes and
/// the geometry.
///
/// # Errors
///
/// SQLCODE -916 if a row is not one of a view.
pub fn from_rows(read: &[Vec<Vec<Value>>]) -> Result<Network> {
    let [nodes, links] = read else { return Err(damaged("a network is read of two views")) };
    let mut net = Network::default();
    for r in nodes {
        net.nodes.insert(id(r, 0)?, optional_geometry(Some(at(r, 1)))?);
    }
    for r in links {
        let link = Link { start: id(r, 1)?, end: id(r, 2)?, geometry: optional_geometry(Some(at(r, 3)))? };
        net.links.insert(id(r, 0)?, link);
    }
    Ok(net)
}

/// The fields of `row`, a query's row of one value of a row type.
fn fields(row: Vec<Value>) -> Vec<Value> {
    match row.as_slice() {
        [Value::Row(fields)] => fields.to_vec(),
        _ => row,
    }
}

/// The rows of each of `queries` in `cx`, each row's fields.
///
/// # Errors
///
/// A query's.
pub(super) fn read_all(cx: &mut dyn ProcedureContext, queries: &[String]) -> Result<Vec<Vec<Vec<Value>>>> {
    queries.iter().map(|q| cx.rows(q, &[]).map(|rows| rows.into_iter().map(fields).collect())).collect()
}

/// Network `name`, read in `cx`.
///
/// # Errors
///
/// As `read_failed`; SQLCODE -916 of a row that is not one of a view.
pub(super) fn load(cx: &mut dyn ProcedureContext, name: &str) -> Result<Network> {
    from_rows(&read_all(cx, &reads(name)).map_err(read_failed)?)
}

/// The statements that make `changes` to the base tables of network
/// `name`, in an order no foreign key refuses: nodes made or changed,
/// links made, changed and deleted, then nodes deleted.
fn statements(name: &str, changes: Changes) -> Vec<(String, Vec<Value>)> {
    let schema = delimited(name);
    let (node, link) = (format!("{schema}.{NODE_TABLE}"), format!("{schema}.{LINK_TABLE}"));
    let mut made = Vec::new();
    let mut nodes_gone = Vec::new();
    for c in changes.nodes {
        match c {
            Change::Insert(id, g) => {
                made.push((format!("INSERT INTO {node} VALUES (?, ?)"), vec![int_value(id), geometry_value(g.as_ref())]));
            }
            Change::Update(id, g) => {
                made.push((format!("UPDATE {node} SET GEOMETRY = ? WHERE NODE_ID = ?"), vec![geometry_value(g.as_ref()), int_value(id)]));
            }
            Change::Delete(id) => nodes_gone.push((format!("DELETE FROM {node} WHERE NODE_ID = ?"), vec![int_value(id)])),
        }
    }
    let mut links_gone = Vec::new();
    for c in changes.links {
        match c {
            Change::Insert(id, l) => made.push((format!("INSERT INTO {link} VALUES (?, ?, ?, ?)"), link_row(id, &l))),
            Change::Update(id, l) => {
                let mut row = link_row(id, &l);
                row.rotate_left(1);
                made.push((format!("UPDATE {link} SET START_NODE = ?, END_NODE = ?, GEOMETRY = ? WHERE LINK_ID = ?"), row));
            }
            Change::Delete(id) => links_gone.push((format!("DELETE FROM {link} WHERE LINK_ID = ?"), vec![int_value(id)])),
        }
    }
    links_gone.into_iter().chain(made).chain(nodes_gone).collect()
}

/// The row of link `id`.
fn link_row(id: i32, l: &Link) -> Vec<Value> {
    vec![int_value(id), int_value(l.start), int_value(l.end), geometry_value(l.geometry.as_ref())]
}

/// Network `name`, read as `before`, written as `after`, in `cx`, with
/// the privileges of the owner of its schema.
///
/// # Errors
///
/// A statement's.
pub(super) fn save(cx: &mut dyn ProcedureContext, name: &str, (before, after): (&Network, &Network)) -> Result<()> {
    for (sql, args) in statements(name, before.changes(after)) {
        cx.run_owned(name, &sql, &args)?;
    }
    Ok(())
}

/// [§12.3.7 Description 2c] The statement that makes network `name`: its
/// schema, its base tables and its views.
pub(super) fn init_statement(name: &str) -> String {
    let schema = delimited(name);
    format!(
        "CREATE SCHEMA {schema} \
         CREATE TABLE {NODE_TABLE} (NODE_ID INTEGER NOT NULL PRIMARY KEY, GEOMETRY ST_POINT) \
         CREATE TABLE {LINK_TABLE} (LINK_ID INTEGER NOT NULL PRIMARY KEY, \
         START_NODE INTEGER NOT NULL REFERENCES {NODE_TABLE} (NODE_ID), \
         END_NODE INTEGER NOT NULL REFERENCES {NODE_TABLE} (NODE_ID), GEOMETRY ST_CURVE) \
         CREATE VIEW ST_NODE AS SELECT NODE_ID, GEOMETRY FROM {NODE_TABLE} \
         CREATE VIEW ST_LINK AS SELECT LINK_ID, START_NODE, END_NODE, GEOMETRY FROM {LINK_TABLE}"
    )
}
