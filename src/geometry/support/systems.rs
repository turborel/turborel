//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1] The routines of
//! ST_SpatialRefSys, each the engine's (`spatial::srs::system`): the
//! constructors of a `<spatial reference system>` and of an SRID,
//! ST_AsWKTSRS, ST_WKTSRSToSQL, ST_SRID and ST_Equals. ST_OrderingEquals
//! [§14.1.7] is the function of that name the geometry types share
//! (`ordering.rs`).
//!
//! A system made from text takes the SRID its AUTHORITY gives it, as the
//! engine reads it; else that of the system supplied whose definition is
//! this text, or of the system defined in the statement's catalog whose
//! definition reads as the same canonical text; else none, and
//! ST_SRID of it is the null value [§14.1.5 Description 2, the SRID being
//! implementation-dependent]. A system of SRID 0 is the unspecified
//! plane, which has no text, so ST_AsWKTSRS of it is the null value.

use super::Support::SpatialRefSys as S;
use super::ops::{Op, Systems, Ty, arg, clob, fault, int, integer, text};
use super::values::{system_of, system_value};
use crate::error::Result;
use crate::spatial::srs::system::SpatialRefSys;
use crate::system::DEFINITION_LENGTH;
use crate::value::Value;

/// ST_MaxSRSAsText, the longest DEFINITION the catalog holds.
pub const MAX_SRS_AS_TEXT: u32 = DEFINITION_LENGTH;

const SYSTEM: Ty = Ty::Of(S);
const TEXT: Ty = Ty::Clob(MAX_SRS_AS_TEXT);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(S, &[TEXT], |a, s| from_text(&text(a, 0)?, s)).in_catalog(),
    Op::constructor(S, &[Ty::Integer], |a, s| {
        SpatialRefSys::from_srid_in(integer(a, 0)?, s).map(|system| system_value(&system)).map_err(|f| fault(&f))
    })
    .in_catalog(),
    Op::method("ST_ASWKTSRS", &[SYSTEM], TEXT, |a, _| Ok(system_of(arg(a, 0)?)?.as_wkt().map_or(Value::Null, |t| clob(&t)))),
    Op::method("ST_WKTSRSTOSQL", &[SYSTEM, TEXT], SYSTEM, |a, s| from_text(&text(a, 1)?, s)).in_catalog(),
    Op::method("ST_SRID", &[SYSTEM], Ty::Integer, |a, _| Ok(system_of(arg(a, 0)?)?.srid().map_or(Value::Null, |n| int(i64::from(n))))),
    Op::method("ST_EQUALS", &[SYSTEM, SYSTEM], Ty::Integer, |a, _| {
        Ok(int(i64::from(system_of(arg(a, 0)?)?.equals(&system_of(arg(a, 1)?)?))))
    }),
];

/// [§14.1.2 Description 3, §14.1.4] The system `text` defines, of the
/// SRID its AUTHORITY gives, or of a system of the same text.
///
/// # Errors
///
/// SQLSTATE 2FF22 if `text` is no `<spatial reference system>`.
pub(super) fn from_text(text: &str, systems: Systems<'_>) -> Result<Value> {
    let system = SpatialRefSys::from_text(text).map_err(|f| fault(&f))?;
    if system.srid().is_some() {
        return Ok(system_value(&system));
    }
    let wkt = system.as_wkt();
    let same = |definition: &str| SpatialRefSys::from_text(definition).is_ok_and(|other| other.as_wkt() == wkt);
    let supplied = crate::system::SRS_SEED.iter().find(|row| row.definition == text.trim()).map(|row| row.id);
    let defined = || systems.and_then(|c| c.systems().iter().find(|row| same(&row.definition))).map(|row| row.id);
    Ok(system_value(&match supplied.or_else(defined) {
        Some(id) => system.with_srid(id),
        None => system,
    }))
}
