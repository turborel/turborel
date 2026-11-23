//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.7, §12.3.14 to
//! §12.3.16] The routines that make a network whole: ST_InitTopoNet, its
//! schema and views; ST_LogiNetFromTGeo and ST_SpatNetFromTGeo, its nodes
//! and links of a topology's; and ST_SpatNetFromGeom, of a collection of
//! geometry values. Each but the first fills a network that is there and
//! empty.

use super::super::{delimited, from_rows, given, init_statement, integer, load, read_all, save, schema_name};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::builtin::ProcedureContext;
use crate::geometry::model::Geom;
use crate::spatial::network::{Network, fault};
use crate::value::Value;

/// The query of the count of the schemas of a name.
const SCHEMATA: &str = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?";
/// The query of the count of the views of a name in a schema.
const VIEWS: &str = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?";

/// The count of the rows of `sql` of `args` in `cx`.
fn count(cx: &mut dyn ProcedureContext, sql: &str, args: &[&str]) -> Result<i32> {
    let values: Vec<Value> = args.iter().map(|a| Value::Char((*a).into())).collect();
    let rows = cx.rows(sql, &values)?;
    Ok(integer(rows.first().and_then(|r| r.first()))?.unwrap_or_default())
}

/// [§12.3.7] ST_InitTopoNet: network `name` made in `cx`.
///
/// # Errors
///
/// SQLSTATE 2FF56 (schema already exists) if a schema has the name; the
/// statement's.
pub(super) fn init(name: &str, cx: &mut dyn ProcedureContext) -> Result<()> {
    if count(cx, SCHEMATA, &[name])? > 0_i32 {
        return Err(fault(
            "§12.3.7 Description 2b",
            SqlState::SPATIAL_SCHEMA_ALREADY_EXISTS,
            &format!("schema {} exists", delimited(name)),
        ));
    }
    cx.run(&init_statement(name), &[])
}

/// What must be there for a routine to fill a network: schemas, each of
/// its rule, then views, each of its schema, name and rule.
struct Needed<'a> {
    /// The schemas.
    schemas: Vec<(&'a str, String)>,
    /// The views.
    views: Vec<(&'a str, &'a str, String)>,
}

/// The exception of the first of `needed` that is not there in `cx`:
/// SQLSTATE 2FF47 (non-existent schema) or 2FF48 (non-existent view);
/// none if all are.
///
/// # Errors
///
/// A query's.
fn missing(cx: &mut dyn ProcedureContext, needed: &Needed<'_>) -> Result<Option<SqlError>> {
    for (schema, rule) in &needed.schemas {
        if count(cx, SCHEMATA, &[schema])? == 0_i32 {
            return Ok(Some(fault(rule, SqlState::SPATIAL_NON_EXISTENT_SCHEMA, &format!("no schema {}", delimited(schema)))));
        }
    }
    for (schema, view, rule) in &needed.views {
        if count(cx, VIEWS, &[schema, view])? == 0_i32 {
            let detail = format!("schema {} has no view {view}", delimited(schema));
            return Ok(Some(fault(rule, SqlState::SPATIAL_NON_EXISTENT_VIEW, &detail)));
        }
    }
    Ok(None)
}

/// Whether `e` is the failure to read a view that is not there.
fn absent(e: &SqlError) -> bool {
    e.sqlstate() == SqlState::SPATIAL_INVALID_NETWORK_NAME || e.code == Code::SyntaxRule
}

/// [Description 2m, 2h] That network `net` of rule `rule` is empty.
///
/// # Errors
///
/// SQLSTATE 2FF42 (non-empty view) if it has a node or a link.
fn empty(net: &Network, rule: &str) -> Result<()> {
    let (which, letter) = if net.nodes.is_empty() { ("ST_LINK", "ii") } else { ("ST_NODE", "i") };
    if net.is_empty() {
        return Ok(());
    }
    Err(fault(&format!("{rule} {letter}"), SqlState::SPATIAL_NON_EMPTY_VIEW, &format!("the network's {which} has rows")))
}

/// [§11.1.2, §11.1.3] The queries of the nodes and edges of topology
/// `name`, of the columns a network's rows have.
fn topology_reads(name: &str) -> Vec<String> {
    let schema = delimited(name);
    vec![
        format!("SELECT ROW(NODE_ID, GEOMETRY) FROM {schema}.ST_NODE"),
        format!("SELECT ROW(EDGE_ID, START_NODE, END_NODE, GEOMETRY) FROM {schema}.ST_EDGE"),
    ]
}

/// [§12.3.14 Description 2f] What a failure to read a topology's views
/// is: SQLSTATE 2FF63 (topology privilege denied) of a privilege the user
/// lacks, else as it is.
fn topology_failed(e: SqlError) -> SqlError {
    if e.code == Code::Privilege { fault("§12.3.14 Description 2f", SqlState::SPATIAL_TOPOLOGY_PRIVILEGE_DENIED, &e.msg) } else { e }
}

/// [§12.3.14, §12.3.15] Network `name` filled from the nodes and edges of
/// the topology `args` names: of their geometry if `spatial`, of none
/// else. The checks of Description 2a to 2m run in the text's order; the
/// privilege of 2c is known only of a network whose views are there.
///
/// # Errors
///
/// SQLSTATE 2FF65 or 2FF63 of a privilege not granted; 2FF03, 2FF62 of
/// the topology's name; 2FF47, 2FF48 of what is not there; 2FF42 if the
/// network is not empty.
pub(super) fn from_topology(name: &str, args: &[Value], spatial: bool, cx: &mut dyn ProcedureContext) -> Result<()> {
    let rule = if spatial { "§12.3.15 Description" } else { "§12.3.14 Description" };
    let network = load(cx, name);
    if let Err(e) = &network
        && !absent(e)
    {
        return Err(e.clone());
    }
    let rules = (format!("{rule} 2d"), format!("{rule} 2e"));
    let topology = schema_name(args.get(1), (&rules.0, &rules.1), SqlState::SPATIAL_INVALID_TOPOLOGY_NAME)?;
    let read = read_all(cx, &topology_reads(&topology)).map_err(topology_failed);
    if let Err(e) = &read
        && e.code != Code::SyntaxRule
    {
        return Err(e.clone());
    }
    let needed = Needed {
        schemas: vec![(name, format!("{rule} 2g")), (&topology, format!("{rule} 2h"))],
        views: vec![
            (name, "ST_NODE", format!("{rule} 2i")),
            (name, "ST_LINK", format!("{rule} 2j")),
            (&topology, "ST_NODE", format!("{rule} 2k")),
            (&topology, "ST_EDGE", format!("{rule} 2l")),
        ],
    };
    let (Ok(before), Ok(read)) = (network, read) else {
        return Err(missing(cx, &needed)?
            .unwrap_or_else(|| fault(&format!("{rule} 2b"), SqlState::SPATIAL_INVALID_NETWORK_NAME, "a view is not there")));
    };
    empty(&before, &format!("{rule} 2m"))?;
    let mut after = from_rows(&read)?;
    if !spatial {
        after.nodes.values_mut().for_each(|g| *g = None);
        after.links.values_mut().for_each(|l| l.geometry = None);
    }
    save(cx, name, (&before, &after))
}

/// [§12.3.16] ST_SpatNetFromGeom: network `name` filled from the
/// collection `args` gives (`Network::add_geometry`).
///
/// # Errors
///
/// SQLSTATE 2FF65 of a privilege not granted; 2FF03 of a null collection;
/// 2FF47, 2FF48 of what is not there; 2FF42 if the network is not empty;
/// 2FF02 of a surface or solid.
pub(super) fn from_geometry(name: &str, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
    let rule = "§12.3.16 Description";
    let network = load(cx, name);
    if let Err(e) = &network
        && !absent(e)
    {
        return Err(e.clone());
    }
    let collection = Geom::from_value(given(args.get(1), &format!("{rule} 2d"))?)?;
    let needed = Needed {
        schemas: vec![(name, format!("{rule} 2e"))],
        views: vec![(name, "ST_NODE", format!("{rule} 2f")), (name, "ST_LINK", format!("{rule} 2g"))],
    };
    let Ok(before) = network else {
        return Err(missing(cx, &needed)?
            .unwrap_or_else(|| fault(&format!("{rule} 2b"), SqlState::SPATIAL_INVALID_NETWORK_NAME, "a view is not there")));
    };
    empty(&before, &format!("{rule} 2h"))?;
    let mut after = Network::default();
    after.add_geometry(&collection)?;
    save(cx, name, (&before, &after))
}
