//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3] The routines that
//! change a topology, each a procedure written in Rust: those the text
//! defines as procedures are invoked by CALL, those it defines as
//! functions in a value expression that may invoke a function that
//! possibly modifies SQL-data (RULES.md D125). Each reads
//! the topology's views, makes its change to the topology read, checking
//! in the order its Description gives, and writes the rows that changed.

use std::sync::Arc;

use super::{
    INTEGER, delimited, fault, geometry, given, init_statements, int_value, integer, load, name_type, required, save, topology_name,
};
use crate::error::{Result, SqlState};
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::geometry::data_type_of;
use crate::geometry::model::Kind;
use crate::spatial::topology::{Faces, Topology, create};
use crate::value::{DataType, Value};

/// A routine of §11.3 that changes a topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoRoutine {
    /// [§11.3.1] ST_AddIsoNode, a function.
    AddIsoNode,
    /// [§11.3.2] ST_MoveIsoNode, a procedure.
    MoveIsoNode,
    /// [§11.3.3] ST_RemIsoNode, a procedure.
    RemIsoNode,
    /// [§11.3.4] ST_AddIsoEdge, a function.
    AddIsoEdge,
    /// [§11.3.6] ST_ChangeEdgeGeom, a procedure.
    ChangeEdgeGeom,
    /// [§11.3.7] ST_RemIsoEdge, a procedure.
    RemIsoEdge,
    /// [§11.3.8] ST_NewEdgesSplit, a function.
    NewEdgesSplit,
    /// [§11.3.9] ST_ModEdgeSplit, a function.
    ModEdgeSplit,
    /// [§11.3.10] ST_NewEdgeHeal, a function.
    NewEdgeHeal,
    /// [§11.3.11] ST_ModEdgeHeal, a procedure.
    ModEdgeHeal,
    /// [§11.3.12] ST_AddEdgeNewFaces, a function.
    AddEdgeNewFaces,
    /// [§11.3.13] ST_AddEdgeModFace, a function.
    AddEdgeModFace,
    /// [§11.3.14] ST_RemEdgeNewFace, a function.
    RemEdgeNewFace,
    /// [§11.3.15] ST_RemEdgeModFace, a procedure.
    RemEdgeModFace,
    /// [§11.3.17] ST_InitTopoGeo, a procedure.
    InitTopoGeo,
    /// [§11.3.18] ST_CreateTopoGeo, a procedure.
    CreateTopoGeo,
}

/// Every routine.
const ALL: [TopoRoutine; 16] = [
    TopoRoutine::AddIsoNode,
    TopoRoutine::MoveIsoNode,
    TopoRoutine::RemIsoNode,
    TopoRoutine::AddIsoEdge,
    TopoRoutine::ChangeEdgeGeom,
    TopoRoutine::RemIsoEdge,
    TopoRoutine::NewEdgesSplit,
    TopoRoutine::ModEdgeSplit,
    TopoRoutine::NewEdgeHeal,
    TopoRoutine::ModEdgeHeal,
    TopoRoutine::AddEdgeNewFaces,
    TopoRoutine::AddEdgeModFace,
    TopoRoutine::RemEdgeNewFace,
    TopoRoutine::RemEdgeModFace,
    TopoRoutine::InitTopoGeo,
    TopoRoutine::CreateTopoGeo,
];

/// The routines, built in.
pub fn procedures() -> Vec<Arc<dyn Procedure>> {
    ALL.into_iter().map(|r| -> Arc<dyn Procedure> { Arc::new(r) }).collect()
}

/// Its kinds of parameter after the topology's name.
#[derive(Debug, Clone, Copy)]
enum Param {
    /// An INTEGER.
    Int,
    /// An ST_Point.
    Point,
    /// An ST_Curve.
    Curve,
    /// An ST_GeomCollection.
    Collection,
}

impl TopoRoutine {
    /// Its name, as an identifier folds.
    pub fn sql_name(self) -> &'static str {
        match self {
            Self::AddIsoNode => "ST_ADDISONODE",
            Self::MoveIsoNode => "ST_MOVEISONODE",
            Self::RemIsoNode => "ST_REMISONODE",
            Self::AddIsoEdge => "ST_ADDISOEDGE",
            Self::ChangeEdgeGeom => "ST_CHANGEEDGEGEOM",
            Self::RemIsoEdge => "ST_REMISOEDGE",
            Self::NewEdgesSplit => "ST_NEWEDGESSPLIT",
            Self::ModEdgeSplit => "ST_MODEDGESPLIT",
            Self::NewEdgeHeal => "ST_NEWEDGEHEAL",
            Self::ModEdgeHeal => "ST_MODEDGEHEAL",
            Self::AddEdgeNewFaces => "ST_ADDEDGENEWFACES",
            Self::AddEdgeModFace => "ST_ADDEDGEMODFACE",
            Self::RemEdgeNewFace => "ST_REMEDGENEWFACE",
            Self::RemEdgeModFace => "ST_REMEDGEMODFACE",
            Self::InitTopoGeo => "ST_INITTOPOGEO",
            Self::CreateTopoGeo => "ST_CREATETOPOGEO",
        }
    }

    /// The subclause of its Description.
    fn rule(self) -> &'static str {
        match self {
            Self::AddIsoNode => "§11.3.1 Description",
            Self::MoveIsoNode => "§11.3.2 Description",
            Self::RemIsoNode => "§11.3.3 Description",
            Self::AddIsoEdge => "§11.3.4 Description",
            Self::ChangeEdgeGeom => "§11.3.6 Description",
            Self::RemIsoEdge => "§11.3.7 Description",
            Self::NewEdgesSplit => "§11.3.8 Description",
            Self::ModEdgeSplit => "§11.3.9 Description",
            Self::NewEdgeHeal => "§11.3.10 Description",
            Self::ModEdgeHeal => "§11.3.11 Description",
            Self::AddEdgeNewFaces => "§11.3.12 Description",
            Self::AddEdgeModFace => "§11.3.13 Description",
            Self::RemEdgeNewFace => "§11.3.14 Description",
            Self::RemEdgeModFace => "§11.3.15 Description",
            Self::InitTopoGeo => "§11.3.17 Description",
            Self::CreateTopoGeo => "§11.3.18 Description",
        }
    }

    /// Its parameters after the topology's name.
    fn params(self) -> &'static [Param] {
        match self {
            Self::AddIsoNode | Self::MoveIsoNode | Self::NewEdgesSplit | Self::ModEdgeSplit => &[Param::Int, Param::Point],
            Self::RemIsoNode | Self::RemIsoEdge | Self::RemEdgeNewFace | Self::RemEdgeModFace => &[Param::Int],
            Self::AddIsoEdge | Self::AddEdgeNewFaces | Self::AddEdgeModFace => &[Param::Int, Param::Int, Param::Curve],
            Self::ChangeEdgeGeom => &[Param::Int, Param::Curve],
            Self::NewEdgeHeal | Self::ModEdgeHeal => &[Param::Int, Param::Int],
            Self::InitTopoGeo => &[],
            Self::CreateTopoGeo => &[Param::Collection],
        }
    }

    /// Whether the text defines it as a function, of an INTEGER result.
    pub fn is_function(self) -> bool {
        matches!(
            self,
            Self::AddIsoNode
                | Self::AddIsoEdge
                | Self::NewEdgesSplit
                | Self::ModEdgeSplit
                | Self::NewEdgeHeal
                | Self::AddEdgeNewFaces
                | Self::AddEdgeModFace
                | Self::RemEdgeNewFace
        )
    }

    /// Its change of `topo`, of its arguments `args`, the topology's name
    /// first; the identifier it returns, if it returns one.
    fn edit(self, topo: &mut Topology, args: &[Value]) -> Result<Option<i32>> {
        let rule = format!("{} 2", self.rule());
        let int = |k: usize| required(args.get(k), &rule);
        let geom = |k: usize| geometry(args.get(k), &rule);
        Ok(match self {
            Self::AddIsoNode => Some(topo.add_iso_node(integer(args.get(1))?, &geom(2)?)?),
            Self::MoveIsoNode => topo.move_iso_node(int(1)?, &geom(2)?).map(|()| None)?,
            Self::RemIsoNode => topo.remove_iso_node(int(1)?).map(|()| None)?,
            Self::AddIsoEdge => Some(topo.add_iso_edge(int(1)?, int(2)?, &geom(3)?)?),
            Self::ChangeEdgeGeom => topo.change_edge_geom(int(1)?, &geom(2)?).map(|()| None)?,
            Self::RemIsoEdge => topo.remove_iso_edge(int(1)?).map(|()| None)?,
            Self::NewEdgesSplit => Some(topo.split_edge(int(1)?, &geom(2)?, true)?),
            Self::ModEdgeSplit => Some(topo.split_edge(int(1)?, &geom(2)?, false)?),
            Self::NewEdgeHeal => topo.heal(int(1)?, int(2)?, true)?,
            Self::ModEdgeHeal => topo.heal(int(1)?, int(2)?, false)?,
            Self::AddEdgeNewFaces => Some(topo.add_edge(int(1)?, int(2)?, &geom(3)?, Faces::New)?),
            Self::AddEdgeModFace => Some(topo.add_edge(int(1)?, int(2)?, &geom(3)?, Faces::Modify)?),
            Self::RemEdgeNewFace => topo.remove_edge(int(1)?, true)?,
            Self::RemEdgeModFace => topo.remove_edge(int(1)?, false)?,
            Self::CreateTopoGeo => create(topo, &geom(1)?).map(|()| None)?,
            Self::InitTopoGeo => None,
        })
    }

    /// It run of `args` in `cx`: the identifier it returns, if any.
    fn run(self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<Option<i32>> {
        let name = topology_name(args.first(), self.rule())?;
        match self {
            Self::InitTopoGeo => init(&name, cx).map(|()| None),
            Self::CreateTopoGeo => {
                let (before, mut after) = for_create(&name, args, cx)?;
                self.edit(&mut after, args)?;
                save(cx, &name, (&before, &after)).map(|()| None)
            }
            Self::AddIsoNode
            | Self::MoveIsoNode
            | Self::RemIsoNode
            | Self::AddIsoEdge
            | Self::ChangeEdgeGeom
            | Self::RemIsoEdge
            | Self::NewEdgesSplit
            | Self::ModEdgeSplit
            | Self::NewEdgeHeal
            | Self::ModEdgeHeal
            | Self::AddEdgeNewFaces
            | Self::AddEdgeModFace
            | Self::RemEdgeNewFace
            | Self::RemEdgeModFace => {
                let before = load(cx, &name)?;
                let mut after = before.clone();
                let made = self.edit(&mut after, args)?;
                save(cx, &name, (&before, &after))?;
                Ok(made)
            }
        }
    }
}

/// The type of parameter `p`.
fn type_of(p: Param) -> DataType {
    match p {
        Param::Int => INTEGER,
        Param::Point => data_type_of(Kind::Point),
        Param::Curve => data_type_of(Kind::Curve),
        Param::Collection => data_type_of(Kind::GeomCollection),
    }
}

/// The count of the rows of `sql` of `args` in `cx`.
fn count(cx: &mut dyn ProcedureContext, sql: &str, args: &[Value]) -> Result<i32> {
    let rows = cx.rows(sql, args)?;
    let first = rows.first().and_then(|r| r.first());
    Ok(integer(first)?.unwrap_or_default())
}

/// [§11.3.17] ST_InitTopoGeo: topology `name` made in `cx`.
///
/// # Errors
///
/// SQLSTATE 2FF56 (schema already exists) if a schema has the name; a
/// statement's.
fn init(name: &str, cx: &mut dyn ProcedureContext) -> Result<()> {
    let schemata = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?";
    if count(cx, schemata, &[Value::Char(name.to_owned())])? > 0_i32 {
        return Err(fault(
            "§11.3.17 Description 2b",
            SqlState::SPATIAL_SCHEMA_ALREADY_EXISTS,
            &format!("schema {} exists", delimited(name)),
        ));
    }
    for sql in init_statements(name) {
        cx.run(&sql, &[])?;
    }
    Ok(())
}

/// [§11.3.18 Description 2c to 2h] Topology `name`, read for
/// ST_CreateTopoGeo of `args`, twice: as it is and to change.
///
/// # Errors
///
/// SQLSTATE 2FF63 (topology privilege denied); 2FF03 (null argument) of a
/// null collection; 2FF47 (non-existent schema) if no schema has the
/// name; 2FF48 (non-existent view) if one of its views is not there.
fn for_create(name: &str, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<(Topology, Topology)> {
    let loaded = load(cx, name);
    if let Err(e) = &loaded
        && e.sqlstate() == SqlState::SPATIAL_TOPOLOGY_PRIVILEGE_DENIED
    {
        return Err(e.clone());
    }
    given(args.get(1), "§11.3.18 Description 2d")?;
    let topo = match loaded {
        Ok(t) => t,
        Err(e) => {
            // A view that is not there makes the name no topology's.
            if e.sqlstate() == SqlState::SPATIAL_INVALID_TOPOLOGY_NAME {
                return Err(missing(name, cx)?);
            }
            return Err(e);
        }
    };
    Ok((topo.clone(), topo))
}

/// [§11.3.18 Description 2e to 2h] Why topology `name` cannot be read:
/// its schema or one of its views is not there.
fn missing(name: &str, cx: &mut dyn ProcedureContext) -> Result<crate::error::SqlError> {
    let schemata = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?";
    let named = [Value::Char(name.to_owned())];
    if count(cx, schemata, &named)? == 0_i32 {
        return Ok(fault("§11.3.18 Description 2e", SqlState::SPATIAL_NON_EXISTENT_SCHEMA, &format!("no schema {}", delimited(name))));
    }
    for (view, rule) in [("ST_NODE", "2f"), ("ST_EDGE", "2g"), ("ST_FACE", "2h")] {
        let views = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?";
        if count(cx, views, &[Value::Char(name.to_owned()), Value::Char(view.to_owned())])? == 0_i32 {
            let detail = format!("schema {} has no view {view}", delimited(name));
            return Ok(fault(&format!("§11.3.18 Description {rule}"), SqlState::SPATIAL_NON_EXISTENT_VIEW, &detail));
        }
    }
    Ok(fault("§11.3.18 Description 2b", SqlState::SPATIAL_INVALID_TOPOLOGY_NAME, &format!("{} is no topology", delimited(name))))
}

impl Procedure for TopoRoutine {
    fn name(&self) -> &str {
        self.sql_name()
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        let params = self.params();
        (count == params.len().saturating_add(1)).then(|| std::iter::once(name_type()).chain(params.iter().map(|p| type_of(*p))).collect())
    }

    fn returns(&self, count: usize) -> Option<DataType> {
        (self.is_function() && self.parameters(count).is_some()).then_some(INTEGER)
    }

    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
        self.run(args, cx).map(|_| ())
    }

    fn evaluate(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<Value> {
        Ok(self.run(args, cx)?.map_or(Value::Null, int_value))
    }
}
