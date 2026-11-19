//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.8, §16.1.20, §16.2.22,
//! §17.2.21] The SQL transform groups of the types: each group's name,
//! its TO SQL routine and its FROM SQL routine, each a routine of the
//! type (`ops.rs`). CAST takes a value through each group's SQL type
//! (`types.rs`): the
//! well-known text from and to a character string, the radians of an
//! angle or a direction from and to a number, the well-known binary of a
//! vector from and to a binary string. The GML groups are their methods.
//! A value reaches a client as the ST_WellKnownText group gives it, the
//! default group. Reading: a built-in type is no row of
//! USER_DEFINED_TYPES, so INFORMATION_SCHEMA.TRANSFORMS shows none of
//! these groups, as it shows none of the geometry types'.

use super::Support;

/// A transform group of a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransformGroup {
    /// The type.
    pub of: Support,
    /// The group's name.
    pub name: &'static str,
    /// Its TO SQL routine.
    pub to_sql: &'static str,
    /// Its FROM SQL routine.
    pub from_sql: &'static str,
}

const fn group(of: Support, name: &'static str, to_sql: &'static str, from_sql: &'static str) -> TransformGroup {
    TransformGroup { of, name, to_sql, from_sql }
}

/// The groups, of the types in the order of their clauses.
pub const TRANSFORM_GROUPS: [TransformGroup; 10] = [
    group(Support::SpatialRefSys, "ST_WELLKNOWNTEXT", "ST_WKTSRSTOSQL", "ST_ASWKTSRS"),
    group(Support::Angle, "ST_WELLKNOWNTEXT", "ST_ANGLE", "ST_ASTEXT"),
    group(Support::Angle, "ST_WELLKNOWNBINARY", "ST_ANGLE", "ST_RADIANS"),
    group(Support::Angle, "ST_GML", "ST_GMLTOSQL", "ST_ASGML"),
    group(Support::Direction, "ST_WELLKNOWNTEXT", "ST_DIRECTION", "ST_ASTEXT"),
    group(Support::Direction, "ST_WELLKNOWNBINARY", "ST_DIRECTION", "ST_RADIANS"),
    group(Support::Direction, "ST_GML", "ST_GMLTOSQL", "ST_ASGML"),
    group(Support::Vector, "ST_WELLKNOWNTEXT", "ST_WKTTOSQL", "ST_ASTEXT"),
    group(Support::Vector, "ST_WELLKNOWNBINARY", "ST_WKBTOSQL", "ST_ASBINARY"),
    group(Support::Vector, "ST_GML", "ST_GMLTOSQL", "ST_ASGML"),
];
