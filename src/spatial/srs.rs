//! Spatial reference systems, units of measure, angles, directions and
//! coordinate transforms of SQL/MM Spatial, ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §4.6, §4.8, §14.1, §16.1, §16.2, §19.4 and §20.4.
//!
//! This is a pure engine on plain `[f64; 2]` and `[f64; 3]` coordinates
//! with no SQL dependency. The SQL catalogs, types and routines wrap it.
//! Its parts are the <spatial reference system> text of §14.1.9 and its
//! model ([`wkt`], [`model`]), the ST_SpatialRefSys semantics
//! ([`system`]), the units of measure ([`units`]), ellipsoids and datum shifts ([`ellipsoid`]), map
//! projections ([`projection`]), the transform pipeline of ST_Transform
//! ([`transform`]), ellipsoidal geodesics for geodetic ST_Distance,
//! ST_Length and ST_Area ([`geodesic`]), and the ST_Angle and
//! ST_Direction types ([`angle`], [`direction`]).

pub mod angle;
pub mod direction;
pub mod ellipsoid;
pub mod fault;
pub mod geodesic;
pub mod model;
mod notation;
mod number;
pub mod projection;
pub mod system;
pub mod transform;
pub mod units;
pub mod wkt;

pub use fault::{Condition, Fault};

// Numeric constants shared by the engine's formulas (RULES.md R-CONST-3).

/// Two, the factor of double angles and half-angle identities.
const TWO: f64 = 2.0;
/// One half, of half-angle identities.
const HALF: f64 = 0.5;

#[cfg(test)]
mod tests;
