//! The parsed form of a `<spatial reference system>` of ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §14.1.9: projected, geographic and geocentric
//! coordinate systems and the datum, spheroid, prime meridian, unit,
//! projection and parameter items they are made of.
//!
//! Beyond the grammar it carries three items of the OGC coordinate
//! transformation text the reference systems in use are written in, each
//! an \[EXTENSION\]: AUTHORITY on any item, TOWGS84 in DATUM (the seven
//! Helmert parameters a datum shift needs; the grammar has no other way
//! to relate two datums) and AXIS in each coordinate system.

/// An AUTHORITY item: the organization and its code for the item.
#[derive(Clone, Debug, PartialEq)]
pub struct Authority {
    /// The organization, such as EPSG.
    pub name: String,
    /// The organization's identifier of the item, as text.
    pub code: String,
}

/// A `<unit>`: a name and the metres or radians per unit (§14.1.9 m).
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// `<unit name>`.
    pub name: String,
    /// `<conversion factor>`, above zero.
    pub factor: f64,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<spheroid>` (§14.1.9 k).
#[derive(Clone, Debug, PartialEq)]
pub struct Spheroid {
    /// `<spheroid name>`.
    pub name: String,
    /// `<semi-major axis>`, metres, above zero (§14.1.9 l).
    pub semi_major: f64,
    /// `<inverse flattening>`; zero means a sphere.
    pub inverse_flattening: f64,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// TOWGS84: the seven-parameter Helmert transformation from the datum to
/// WGS 84, in the position vector convention (EPSG method 9606).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Helmert {
    /// Translations along X, Y and Z, metres.
    pub translation: [f64; 3],
    /// Rotations about X, Y and Z, arc-seconds.
    pub rotation: [f64; 3],
    /// Scale difference, parts per million.
    pub scale: f64,
}

/// A `<datum>` (§14.1.9 j).
#[derive(Clone, Debug, PartialEq)]
pub struct Datum {
    /// `<datum name>`.
    pub name: String,
    /// Its `<spheroid>`.
    pub spheroid: Spheroid,
    /// TOWGS84, if given.
    pub to_wgs84: Option<Helmert>,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<prime meridian>` (§14.1.9 l).
#[derive(Clone, Debug, PartialEq)]
pub struct PrimeMeridian {
    /// `<prime meridian name>`.
    pub name: String,
    /// `<longitude>` from Greenwich, in the angular unit of the geographic
    /// system, or in degrees in a geocentric one, which has none.
    pub longitude: f64,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// The direction of an AXIS item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// NORTH.
    North,
    /// SOUTH.
    South,
    /// EAST.
    East,
    /// WEST.
    West,
    /// UP.
    Up,
    /// DOWN.
    Down,
    /// OTHER, as of the geocentric X axis.
    Other,
}

/// An AXIS item: a name and a direction.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// The axis name.
    pub name: String,
    /// Where it points.
    pub direction: Direction,
}

/// A `<parameter>` of a projection (§14.1.9 i).
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// `<parameter name>`.
    pub name: String,
    /// `<value>`: angles in the geographic system's angular unit, lengths in
    /// the projected system's linear unit.
    pub value: f64,
}

/// A `<projection>` (§14.1.9 h).
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    /// `<projection name>`.
    pub name: String,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<geographic cs>` (§14.1.9 e).
#[derive(Clone, Debug, PartialEq)]
pub struct Geographic {
    /// `<name>`.
    pub name: String,
    /// Its `<datum>`.
    pub datum: Datum,
    /// Its `<prime meridian>`.
    pub prime_meridian: PrimeMeridian,
    /// Its `<angular unit>`.
    pub angular_unit: Unit,
    /// The optional `<linear unit>` of ellipsoidal heights (§23.3 claim 8).
    pub linear_unit: Option<Unit>,
    /// Its AXIS items, none or two or three.
    pub axes: Vec<Axis>,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<projected cs>` (§14.1.9 d).
#[derive(Clone, Debug, PartialEq)]
pub struct Projected {
    /// `<name>`.
    pub name: String,
    /// The `<geographic cs>` it projects.
    pub geographic: Geographic,
    /// Its `<projection>`.
    pub projection: Projection,
    /// Its `<parameter>`s, in order.
    pub parameters: Vec<Parameter>,
    /// Its `<linear unit>`.
    pub linear_unit: Unit,
    /// Its AXIS items, none or two.
    pub axes: Vec<Axis>,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<geocentric cs>` (§14.1.9 f).
#[derive(Clone, Debug, PartialEq)]
pub struct Geocentric {
    /// `<name>`.
    pub name: String,
    /// Its `<datum>`.
    pub datum: Datum,
    /// Its `<prime meridian>`.
    pub prime_meridian: PrimeMeridian,
    /// Its `<linear unit>`.
    pub linear_unit: Unit,
    /// Its AXIS items, none or three.
    pub axes: Vec<Axis>,
    /// Its AUTHORITY, if given.
    pub authority: Option<Authority>,
}

/// A `<spatial reference system>` (§14.1.9 b).
#[derive(Clone, Debug, PartialEq)]
pub enum CoordinateSystem {
    /// A `<projected cs>`, X and Y.
    Projected(Box<Projected>),
    /// A `<geographic cs>`, longitude and latitude.
    Geographic(Box<Geographic>),
    /// A `<geocentric cs>`, X, Y and Z.
    Geocentric(Box<Geocentric>),
}

impl CoordinateSystem {
    /// Its `<name>`.
    pub fn name(&self) -> &str {
        match self {
            CoordinateSystem::Projected(cs) => &cs.name,
            CoordinateSystem::Geographic(cs) => &cs.name,
            CoordinateSystem::Geocentric(cs) => &cs.name,
        }
    }

    /// Its own AUTHORITY, if given.
    pub fn authority(&self) -> Option<&Authority> {
        match self {
            CoordinateSystem::Projected(cs) => cs.authority.as_ref(),
            CoordinateSystem::Geographic(cs) => cs.authority.as_ref(),
            CoordinateSystem::Geocentric(cs) => cs.authority.as_ref(),
        }
    }

    /// Its datum.
    pub fn datum(&self) -> &Datum {
        match self {
            CoordinateSystem::Projected(cs) => &cs.geographic.datum,
            CoordinateSystem::Geographic(cs) => &cs.datum,
            CoordinateSystem::Geocentric(cs) => &cs.datum,
        }
    }

    /// The `<linear unit>` lengths are measured in: a projected or
    /// geocentric system's own, or a geographic system's optional one.
    /// §5.1.41 Description 2b makes ST_Distance return values in it.
    pub fn linear_unit(&self) -> Option<&Unit> {
        match self {
            CoordinateSystem::Projected(cs) => Some(&cs.linear_unit),
            CoordinateSystem::Geographic(cs) => cs.linear_unit.as_ref(),
            CoordinateSystem::Geocentric(cs) => Some(&cs.linear_unit),
        }
    }

    /// Whether coordinates are longitudes and latitudes on an ellipsoid, so
    /// lengths, distances and areas are geodesic (§3.1.4 d).
    pub fn is_geodetic(&self) -> bool {
        matches!(self, CoordinateSystem::Geographic(_))
    }
}
