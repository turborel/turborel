//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3] The routines that
//! change a network, each a procedure written in Rust: those the text
//! defines as procedures are invoked by CALL, those it defines as
//! functions in a value expression that may invoke a function that
//! possibly modifies SQL-data (RULES.md D-SPATIAL-ROUTINES). Each reads
//! the network's views, makes its change to the network read, checking in
//! the order its Description gives, and writes the rows that changed.

use std::sync::Arc;

use super::{INTEGER, int_value, load, name_type, network_name, optional_geometry, required, save};
use crate::error::{Result, SqlState};
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::geometry::data_type_of;
use crate::geometry::model::{Geom, Kind};
use crate::spatial::network::heal::Heal;
use crate::spatial::network::split::Split;
use crate::spatial::network::{Network, fault};
use crate::value::{DataType, Value};

mod whole;

/// A routine of §12.3 that changes a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRoutine {
    /// [§12.3.1] ST_AddIsoNetNode, a function.
    AddIsoNetNode,
    /// [§12.3.2] ST_MoveIsoNetNode, a procedure.
    MoveIsoNetNode,
    /// [§12.3.3] ST_RemIsoNetNode, a procedure.
    RemIsoNetNode,
    /// [§12.3.4] ST_AddLink, a function.
    AddLink,
    /// [§12.3.5] ST_ChangeLinkGeom, a procedure.
    ChangeLinkGeom,
    /// [§12.3.6] ST_RemoveLink, a procedure.
    RemoveLink,
    /// [§12.3.7] ST_InitTopoNet, a procedure.
    InitTopoNet,
    /// [§12.3.8] ST_NewLogLinkSplit, a function.
    NewLogLinkSplit,
    /// [§12.3.9] ST_ModLogLinkSplit, a function.
    ModLogLinkSplit,
    /// [§12.3.10] ST_NewGeoLinkSplit, a function.
    NewGeoLinkSplit,
    /// [§12.3.11] ST_ModGeoLinkSplit, a function.
    ModGeoLinkSplit,
    /// [§12.3.12] ST_NewLinkHeal, a function.
    NewLinkHeal,
    /// [§12.3.13] ST_ModLinkHeal, a procedure.
    ModLinkHeal,
    /// [§12.3.14] ST_LogiNetFromTGeo, a procedure.
    LogiNetFromTGeo,
    /// [§12.3.15] ST_SpatNetFromTGeo, a procedure.
    SpatNetFromTGeo,
    /// [§12.3.16] ST_SpatNetFromGeom, a procedure.
    SpatNetFromGeom,
}

/// Every routine.
const ALL: [NetRoutine; 16] = [
    NetRoutine::AddIsoNetNode,
    NetRoutine::MoveIsoNetNode,
    NetRoutine::RemIsoNetNode,
    NetRoutine::AddLink,
    NetRoutine::ChangeLinkGeom,
    NetRoutine::RemoveLink,
    NetRoutine::InitTopoNet,
    NetRoutine::NewLogLinkSplit,
    NetRoutine::ModLogLinkSplit,
    NetRoutine::NewGeoLinkSplit,
    NetRoutine::ModGeoLinkSplit,
    NetRoutine::NewLinkHeal,
    NetRoutine::ModLinkHeal,
    NetRoutine::LogiNetFromTGeo,
    NetRoutine::SpatNetFromTGeo,
    NetRoutine::SpatNetFromGeom,
];

/// The routines, built in.
pub fn procedures() -> Vec<Arc<dyn Procedure>> {
    ALL.into_iter().map(|r| -> Arc<dyn Procedure> { Arc::new(r) }).collect()
}

/// Its kinds of parameter after the network's name.
#[derive(Debug, Clone, Copy)]
enum Param {
    /// An INTEGER.
    Int,
    /// An ST_Point.
    Point,
    /// An ST_Curve.
    Curve,
    /// A topology's name.
    Name,
    /// An ST_GeomCollection.
    Collection,
}

/// The rules of a null argument after the network's name, by position:
/// the Description's 2d, then 2e.
const NULL_RULES: [&str; 2] = ["2d", "2e"];

impl NetRoutine {
    /// Its name, as an identifier folds.
    pub fn sql_name(self) -> &'static str {
        match self {
            Self::AddIsoNetNode => "ST_ADDISONETNODE",
            Self::MoveIsoNetNode => "ST_MOVEISONETNODE",
            Self::RemIsoNetNode => "ST_REMISONETNODE",
            Self::AddLink => "ST_ADDLINK",
            Self::ChangeLinkGeom => "ST_CHANGELINKGEOM",
            Self::RemoveLink => "ST_REMOVELINK",
            Self::InitTopoNet => "ST_INITTOPONET",
            Self::NewLogLinkSplit => "ST_NEWLOGLINKSPLIT",
            Self::ModLogLinkSplit => "ST_MODLOGLINKSPLIT",
            Self::NewGeoLinkSplit => "ST_NEWGEOLINKSPLIT",
            Self::ModGeoLinkSplit => "ST_MODGEOLINKSPLIT",
            Self::NewLinkHeal => "ST_NEWLINKHEAL",
            Self::ModLinkHeal => "ST_MODLINKHEAL",
            Self::LogiNetFromTGeo => "ST_LOGINETFROMTGEO",
            Self::SpatNetFromTGeo => "ST_SPATNETFROMTGEO",
            Self::SpatNetFromGeom => "ST_SPATNETFROMGEOM",
        }
    }

    /// The subclause of its Description.
    fn rule(self) -> &'static str {
        match self {
            Self::AddIsoNetNode => "§12.3.1 Description",
            Self::MoveIsoNetNode => "§12.3.2 Description",
            Self::RemIsoNetNode => "§12.3.3 Description",
            Self::AddLink => "§12.3.4 Description",
            Self::ChangeLinkGeom => "§12.3.5 Description",
            Self::RemoveLink => "§12.3.6 Description",
            Self::InitTopoNet => "§12.3.7 Description",
            Self::NewLogLinkSplit => "§12.3.8 Description",
            Self::ModLogLinkSplit => "§12.3.9 Description",
            Self::NewGeoLinkSplit => "§12.3.10 Description",
            Self::ModGeoLinkSplit => "§12.3.11 Description",
            Self::NewLinkHeal => "§12.3.12 Description",
            Self::ModLinkHeal => "§12.3.13 Description",
            Self::LogiNetFromTGeo => "§12.3.14 Description",
            Self::SpatNetFromTGeo => "§12.3.15 Description",
            Self::SpatNetFromGeom => "§12.3.16 Description",
        }
    }

    /// Its parameters after the network's name.
    fn params(self) -> &'static [Param] {
        match self {
            Self::AddIsoNetNode => &[Param::Point],
            Self::MoveIsoNetNode | Self::NewGeoLinkSplit | Self::ModGeoLinkSplit => &[Param::Int, Param::Point],
            Self::RemIsoNetNode | Self::RemoveLink | Self::NewLogLinkSplit | Self::ModLogLinkSplit => &[Param::Int],
            Self::AddLink => &[Param::Int, Param::Int, Param::Curve],
            Self::ChangeLinkGeom => &[Param::Int, Param::Curve],
            Self::NewLinkHeal | Self::ModLinkHeal => &[Param::Int, Param::Int],
            Self::InitTopoNet => &[],
            Self::LogiNetFromTGeo | Self::SpatNetFromTGeo => &[Param::Name],
            Self::SpatNetFromGeom => &[Param::Collection],
        }
    }

    /// Whether the text defines it as a function, of an INTEGER result.
    pub fn is_function(self) -> bool {
        matches!(
            self,
            Self::AddIsoNetNode
                | Self::AddLink
                | Self::NewLogLinkSplit
                | Self::ModLogLinkSplit
                | Self::NewGeoLinkSplit
                | Self::ModGeoLinkSplit
                | Self::NewLinkHeal
        )
    }

    /// The integer argument at `k`, which is not null [Description 2d,
    /// 2e].
    fn int(self, args: &[Value], k: usize) -> Result<i32> {
        let letter = NULL_RULES.get(k.saturating_sub(1)).copied().unwrap_or_default();
        required(args.get(k), &format!("{} {letter}", self.rule()))
    }

    /// The point argument at `k`, which is not null [Description 2e].
    fn point(self, args: &[Value], k: usize) -> Result<Geom> {
        let rule = format!("{} 2e", self.rule());
        optional_geometry(args.get(k))?.ok_or_else(|| fault(&rule, SqlState::SPATIAL_NULL_ARGUMENT, "an argument is null"))
    }

    /// Its change of `net`, of its arguments `args`, the network's name
    /// first; the identifier it returns, if it returns one.
    fn edit(self, net: &mut Network, args: &[Value]) -> Result<Option<i32>> {
        let geom = |k: usize| optional_geometry(args.get(k));
        Ok(match self {
            Self::AddIsoNetNode => Some(net.add_iso_node(geom(1)?)?),
            Self::MoveIsoNetNode => net.move_iso_node(self.int(args, 1)?, geom(2)?).map(|()| None)?,
            Self::RemIsoNetNode => net.remove_iso_node(self.int(args, 1)?).map(|()| None)?,
            Self::AddLink => Some(net.add_link((self.int(args, 1)?, self.int(args, 2)?), geom(3)?)?),
            Self::ChangeLinkGeom => net.change_link_geom(self.int(args, 1)?, geom(2)?).map(|()| None)?,
            Self::RemoveLink => {
                net.remove_link(self.int(args, 1)?);
                None
            }
            Self::NewLogLinkSplit => Some(net.split_logical(self.int(args, 1)?, Split::New)?),
            Self::ModLogLinkSplit => Some(net.split_logical(self.int(args, 1)?, Split::Modify)?),
            Self::NewGeoLinkSplit => Some(net.split_spatial(self.int(args, 1)?, &self.point(args, 2)?, Split::New)?),
            Self::ModGeoLinkSplit => Some(net.split_spatial(self.int(args, 1)?, &self.point(args, 2)?, Split::Modify)?),
            Self::NewLinkHeal => net.heal((self.int(args, 1)?, self.int(args, 2)?), Heal::New)?,
            Self::ModLinkHeal => net.heal((self.int(args, 1)?, self.int(args, 2)?), Heal::Modify)?,
            Self::InitTopoNet | Self::LogiNetFromTGeo | Self::SpatNetFromTGeo | Self::SpatNetFromGeom => None,
        })
    }

    /// It run of `args` in `cx`: the identifier it returns, if any.
    fn run(self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<Option<i32>> {
        let name = network_name(args.first(), self.rule())?;
        match self {
            Self::InitTopoNet => whole::init(&name, cx).map(|()| None),
            Self::LogiNetFromTGeo => whole::from_topology(&name, args, false, cx).map(|()| None),
            Self::SpatNetFromTGeo => whole::from_topology(&name, args, true, cx).map(|()| None),
            Self::SpatNetFromGeom => whole::from_geometry(&name, args, cx).map(|()| None),
            Self::AddIsoNetNode
            | Self::MoveIsoNetNode
            | Self::RemIsoNetNode
            | Self::AddLink
            | Self::ChangeLinkGeom
            | Self::RemoveLink
            | Self::NewLogLinkSplit
            | Self::ModLogLinkSplit
            | Self::NewGeoLinkSplit
            | Self::ModGeoLinkSplit
            | Self::NewLinkHeal
            | Self::ModLinkHeal => {
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
        Param::Name => name_type(),
        Param::Collection => data_type_of(Kind::GeomCollection),
    }
}

impl Procedure for NetRoutine {
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
