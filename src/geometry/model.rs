//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2, §5.1.68] The geometry
//! model every spatial routine works on (RULES.md D119): one
//! value, `Geom`, of each of the 23 instantiable types of the hierarchy
//! (`model/kind.rs`), in XY, XYZ, XYM or XYZM, with its spatial reference
//! system's identifier.
//!
//! A value is a tree: a point's coordinates, a curve of any interpolation
//! (`Curve`), a surface (`Surface`), a solid's shells, or a collection of
//! those. Coordinates are kept flat, one `f64` after another, so many
//! points hold one allocation, strided by the value's `Dims`. Every part of
//! a value has the value's `Dims` and SRID [§5.1.68 Description b to e,
//! §4.2.1], which are kept once, at the root.
//!
//! Its stored form (`model/payload.rs`) is a version byte, the SRID, then
//! the value's canonical well-known binary representation
//! (`model/encode.rs`, `model/decode.rs`): little-endian, the codes of
//! Table 15, with the readings of the draft RULES.md D119
//! records for what its grammar leaves ambiguous.

mod body;
mod bounds;
mod cast;
mod check;
mod decode;
mod decode_parts;
mod encode;
mod kind;
mod linear;
mod parts;
mod payload;
mod xy;

pub use cast::cast;
pub(crate) use cast::empty as empty_body;
pub use decode::{read_wkb, read_wkb_typed};
pub use encode::write_wkb;
pub use kind::*;
pub use payload::{PAYLOAD_VERSION, header};

use crate::error::Result;
use crate::spatial::Bounds;
use crate::value::Value;

/// [§4.2.1] The coordinate dimensions of a value: x and y, with or without
/// z, with or without m.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dims {
    /// x and y, in R2.
    Xy,
    /// x, y and z, in R3.
    Xyz,
    /// x, y and m, in R3.
    Xym,
    /// x, y, z and m, in R4.
    Xyzm,
}

/// [§5.1.68 Table 15] What the code of a value with z adds to its base
/// code.
pub const WKB_Z: u32 = 1000;
/// What the code of a value with m adds.
pub const WKB_M: u32 = 2000;
/// What the code of a value with z and m adds.
pub const WKB_ZM: u32 = 3000;

/// The ordinates of a point of x and y alone.
const XY: usize = 2;
/// The ordinates of a point of x, y and one of z or m.
const XYZ: usize = 3;
/// The ordinates of a point of x, y, z and m.
const XYZM: usize = 4;

impl Dims {
    /// [§5.1.11] Whether a value has z coordinates (ST_Is3D).
    pub const fn has_z(self) -> bool {
        matches!(self, Dims::Xyz | Dims::Xyzm)
    }

    /// [§5.1.12] Whether a value has m coordinates (ST_IsMeasured).
    pub const fn has_m(self) -> bool {
        matches!(self, Dims::Xym | Dims::Xyzm)
    }

    /// How many `f64` one point takes: 2, 3 or 4.
    pub const fn stride(self) -> usize {
        match self {
            Dims::Xy => XY,
            Dims::Xyz | Dims::Xym => XYZ,
            Dims::Xyzm => XYZM,
        }
    }

    /// [§5.1.3] ST_CoordDim: the coordinate dimension, the stride.
    pub fn coordinate_dimension(self) -> i16 {
        i16::try_from(self.stride()).unwrap_or(i16::MAX)
    }

    /// How many `f64` a location or a direction of an affine placement
    /// takes: 3 with z, else 2 [§5.1.68 `<wkbreferencelocationz binary>`].
    pub const fn planar(self) -> usize {
        if self.has_z() { XYZ } else { XY }
    }

    /// [§5.1.68 Table 15] What its code adds to a base code.
    pub const fn wkb_offset(self) -> u32 {
        match self {
            Dims::Xy => 0,
            Dims::Xyz => WKB_Z,
            Dims::Xym => WKB_M,
            Dims::Xyzm => WKB_ZM,
        }
    }

    /// These dimensions with z if `z`, without if not.
    #[must_use]
    pub const fn with_z(self, z: bool) -> Dims {
        match (z, self.has_m()) {
            (false, false) => Dims::Xy,
            (true, false) => Dims::Xyz,
            (false, true) => Dims::Xym,
            (true, true) => Dims::Xyzm,
        }
    }

    /// These dimensions with m if `m`, without if not.
    #[must_use]
    pub const fn with_m(self, m: bool) -> Dims {
        match (self.has_z(), m) {
            (false, false) => Dims::Xy,
            (true, false) => Dims::Xyz,
            (false, true) => Dims::Xym,
            (true, true) => Dims::Xyzm,
        }
    }

    /// The position of z in a point's ordinates, if it has one.
    pub const fn z_at(self) -> Option<usize> {
        if self.has_z() { Some(XY) } else { None }
    }

    /// The position of m in a point's ordinates, if it has one.
    pub const fn m_at(self) -> Option<usize> {
        match self {
            Dims::Xy | Dims::Xyz => None,
            Dims::Xym => Some(XY),
            Dims::Xyzm => Some(XYZ),
        }
    }
}

/// Points, one after another, each `Dims::stride` ordinates: x, y, then z
/// if any, then m if any. No ordinate is NaN or infinite, and none is
/// negative zero (`check`); an empty point holds none.
pub type Coords = Vec<f64>;

/// [§8.3] The rings of a polygon, the exterior first: each a closed
/// linestring of at least four points. An empty polygon has none.
pub type Rings = Vec<Coords>;

/// [§4.2] A geometry value: its spatial reference system's identifier, its
/// coordinate dimensions and its body. Every part of the body has these
/// dimensions and this identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Geom {
    /// [§5.1.5] The spatial reference system's numeric identifier, as
    /// ST_SRID gives it; it names no system unless the catalog has one of
    /// it (§14).
    pub srid: i32,
    /// [§4.2.1] Its coordinate dimensions.
    pub dims: Dims,
    /// What it is.
    pub body: Body,
}

/// [§4.2] What a geometry value is, of each kind of type.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// [§6.1] ST_Point: its ordinates, none if it is empty.
    Point(Coords),
    /// [§7.1] A curve.
    Curve(Curve),
    /// [§8.1] A surface.
    Surface(Surface),
    /// [§9.2] ST_BRepSolid: its shells, each an ST_PolyhdrlSurface or an
    /// ST_CompoundSurface, the outer shell first; none if it is empty.
    Solid(Vec<Surface>),
    /// [§10.1] A collection.
    Collection(Collection),
}

/// [§7.1] A curve, of each interpolation.
#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    /// [§7.2] ST_LineString: its points, none or at least two.
    LineString(Coords),
    /// [§7.3] ST_CircularString: its points, none or an odd number of at
    /// least three, each arc a start, an intermediate and an end point.
    CircularString(Coords),
    /// [§7.4] ST_Circle: three points, none if it is empty.
    Circle(Coords),
    /// [§7.5] ST_GeodesicString: its control points, none or at least two.
    GeodesicString(Coords),
    /// [§7.6] ST_EllipticalCurve; none if it is empty.
    Elliptical(Option<Elliptical>),
    /// [§7.7] ST_NURBSCurve; none if it is empty.
    Nurbs(Option<Nurbs>),
    /// [§7.8] ST_Clothoid; none if it is empty.
    Clothoid(Option<Clothoid>),
    /// [§7.9] ST_SpiralCurve; none if it is empty.
    Spiral(Option<Spiral>),
    /// [§7.10] ST_CompoundCurve: its curves, none empty, each ending where
    /// the next begins; none if it is empty.
    Compound(Vec<Curve>),
}

/// [§17.3] ST_AffinePlacement: a location and the reference directions the
/// basis vectors of a parameter space map to. Each is `Dims::planar`
/// ordinates: x and y, with z if the value has z.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// The location, the target of the parameter space's origin.
    pub location: Vec<f64>,
    /// The reference directions, one after another.
    pub directions: Vec<f64>,
}

/// The measures at the start and the end of a curve given by parameters,
/// present exactly when the value has m [§5.1.68 `<wkbstartm>`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measures {
    /// The m at the start.
    pub start: f64,
    /// The m at the end.
    pub end: f64,
}

/// [§4.2.9] An elliptical curve's attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Elliptical {
    /// The local coordinate system, its origin the curve's centre.
    pub placement: Placement,
    /// The axis length along u.
    pub u_axis: f64,
    /// The axis length along v.
    pub v_axis: f64,
    /// The constructive parameter angle at the start, in radians.
    pub start_angle: f64,
    /// The constructive parameter angle at the end, in radians.
    pub end_angle: f64,
    /// The measures, of a value with m.
    pub measures: Option<Measures>,
}

/// [§4.2.10] A NURBS curve's attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Nurbs {
    /// The degree of its polynomials [§5.1.68 `<wkbdegree>`, a byte].
    pub degree: u8,
    /// [§17.4] Its control points.
    pub points: Vec<NurbsPoint>,
    /// [§17.5] Its knots, strictly increasing.
    pub knots: Vec<Knot>,
    /// The measures, of a value with m.
    pub measures: Option<Measures>,
}

/// [§17.4] ST_NURBSPoint: a control point adjusted for its weight, and the
/// weight, none of a curve that is not rational.
#[derive(Debug, Clone, PartialEq)]
pub struct NurbsPoint {
    /// The weighted point, `Dims::planar` ordinates.
    pub point: Vec<f64>,
    /// The weight.
    pub weight: Option<f64>,
}

/// [§17.5] ST_Knot: a knot's value and how often it occurs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knot {
    /// The value.
    pub value: f64,
    /// The multiplicity [§5.1.68 `<wkbmultiplicity>`, a byte].
    pub multiplicity: u8,
}

/// [§4.2.11] A clothoid's attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Clothoid {
    /// Places the Fresnel integrals' curve.
    pub placement: Placement,
    /// The scale factor.
    pub scale_factor: f64,
    /// The arc length from the inflection point at the start.
    pub start_distance: f64,
    /// The arc length from the inflection point at the end.
    pub end_distance: f64,
    /// The measures, of a value with m.
    pub measures: Option<Measures>,
}

/// [§4.2.12] A spiral's attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Spiral {
    /// Places the spiral: its start point is the origin, its initial
    /// direction the positive x axis.
    pub placement: Placement,
    /// The length.
    pub length: f64,
    /// The curvature at the start.
    pub start_curvature: f64,
    /// The curvature at the end.
    pub end_curvature: f64,
    /// The kind of spiral.
    pub spiral_type: SpiralType,
    /// The measures, of a value with m.
    pub measures: Option<Measures>,
}

/// [§4.2.12] The kinds of spiral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiralType {
    /// clothoid.
    Clothoid,
    /// bloss.
    Bloss,
    /// biquadratic.
    Biquadratic,
    /// sine.
    Sine,
    /// cosine.
    Cosine,
}

/// [§4.2.12] The names of the kinds of spiral, as `SpiralType` orders them.
pub const SPIRAL_TYPES: [&str; 5] = ["clothoid", "bloss", "biquadratic", "sine", "cosine"];

impl SpiralType {
    /// Its name, as written.
    pub fn name(self) -> &'static str {
        let at = match self {
            SpiralType::Clothoid => 0,
            SpiralType::Bloss => 1,
            SpiralType::Biquadratic => 2,
            SpiralType::Sine => 3,
            SpiralType::Cosine => 4,
        };
        SPIRAL_TYPES.get(at).copied().unwrap_or_default()
    }

    /// The kind named `name`, in any case.
    pub fn named(name: &str) -> Option<SpiralType> {
        let all = [SpiralType::Clothoid, SpiralType::Bloss, SpiralType::Biquadratic, SpiralType::Sine, SpiralType::Cosine];
        all.into_iter().find(|t| t.name().eq_ignore_ascii_case(name))
    }
}

/// [§8.1] A surface.
#[derive(Debug, Clone, PartialEq)]
pub enum Surface {
    /// [§8.2] ST_CurvePolygon: its rings, the exterior first, each a closed
    /// curve; none if it is empty.
    CurvePolygon(Vec<Curve>),
    /// [§8.3] ST_Polygon.
    Polygon(Rings),
    /// [§8.4] ST_Triangle: its exterior ring of four points, the last the
    /// first; none if it is empty.
    Triangle(Coords),
    /// [§8.5] ST_PolyhdrlSurface: its patches, each a polygon.
    PolyhdrlSurface(Vec<Rings>),
    /// [§8.6] ST_TIN.
    Tin(Tin),
    /// [§8.7] ST_CompoundSurface: its surfaces; none if it is empty.
    Compound(Vec<Surface>),
}

/// [§4.2.19] A TIN: its triangles, the elements it was made of, and the
/// longest side a triangle may have.
#[derive(Debug, Clone, PartialEq)]
pub struct Tin {
    /// Its triangles, each an exterior ring of four points; none if it is
    /// empty [§5.1.68 Description cm].
    pub triangles: Vec<Coords>,
    /// [§17.1] Its elements.
    pub elements: Vec<TinElement>,
    /// The longest side a triangle may have, if one is set.
    pub max_side_length: Option<f64>,
}

/// [§17.1] ST_TINElement.
#[derive(Debug, Clone, PartialEq)]
pub struct TinElement {
    /// The element type, at most 30 characters [§5.1.68 Description co].
    pub element_type: String,
    /// The element's identifier.
    pub id: i32,
    /// The element's tag, at most 64 characters.
    pub tag: String,
    /// The element's geometry.
    pub geometry: Box<Body>,
}

/// [§10.1] A collection.
#[derive(Debug, Clone, PartialEq)]
pub enum Collection {
    /// [§10.1] ST_GeomCollection: any geometries.
    Geometry(Vec<Body>),
    /// [§10.2] ST_MultiPoint: points, each empty or not.
    MultiPoint(Vec<Coords>),
    /// [§10.3] ST_MultiCurve.
    MultiCurve(Vec<Curve>),
    /// [§10.4] ST_MultiLineString.
    MultiLineString(Vec<Coords>),
    /// [§10.5] ST_MultiSurface.
    MultiSurface(Vec<Surface>),
    /// [§10.6] ST_MultiPolygon.
    MultiPolygon(Vec<Rings>),
}

impl Geom {
    /// A value, checked as every value is (`check`): its coordinates finite
    /// and positive zero, its parts of the counts and closure their types
    /// need, within the representation's limits.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 (invalid argument) or 2FF25 (mixed coordinate
    /// dimensions) of a body that is no value of its type; SQLCODE -900
    /// past a representation limit (RULES.md D119).
    pub fn new(srid: i32, dims: Dims, body: Body) -> Result<Geom> {
        let mut geom = Geom { srid, dims, body };
        check::check(&geom)?;
        check::normalize(&mut geom);
        Ok(geom)
    }

    /// [§5.1.4] Its most specific type.
    pub fn kind(&self) -> Kind {
        self.body.kind()
    }

    /// [§5.1.7] Whether it is the empty set: it has no point.
    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    /// Its envelope in x and y, holding every point of it, the points a
    /// curve passes between its control points among them; none if it is
    /// empty. A curve given by parameters is bounded, not traced
    /// (`model/bounds.rs`).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 if a bound is not finite.
    pub fn bounds(&self) -> Result<Option<Bounds>> {
        bounds::of(self)
    }

    /// The same value with another SRID, its coordinates as they are
    /// [§5.1.5 ST_SRID(INTEGER)].
    #[must_use]
    pub fn with_srid(mut self, srid: i32) -> Geom {
        self.srid = srid;
        self
    }

    /// A part of it, `body`, as a value of its own: its SRID and dimensions.
    #[must_use]
    pub fn part(&self, body: Body) -> Geom {
        Geom { srid: self.srid, dims: self.dims, body }
    }

    /// Its stored form: the payload of an `ST_Geometry` value.
    pub fn to_value(&self) -> Value {
        payload::value(self)
    }

    /// The value `v` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if `v` is no geometry value; -916 if its payload is
    /// malformed.
    pub fn from_value(v: &Value) -> Result<Geom> {
        payload::geom(v)
    }

    /// Its canonical payload bytes: the version, the SRID and its
    /// canonical well-known binary representation.
    pub fn encode(&self) -> Vec<u8> {
        payload::encode(self)
    }

    /// The value of canonical payload bytes `encode` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not canonical.
    pub fn decode(bytes: &[u8]) -> Result<Geom> {
        payload::decode(bytes)
    }

    /// [§4.2.1.6] Its linear form, which planar algorithms take: the same
    /// value if it is of points, linestrings and polygons, a triangle as a
    /// polygon, a polyhedral surface or a TIN as a multipolygon, and a
    /// compound curve or curve polygon of linestrings as a linestring or
    /// polygon; the curves the linearizer takes (`model/linear.rs`) else.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a curve no linearizer takes yet.
    pub fn linear(&self) -> Result<Geom> {
        linear::of(self)
    }

    /// [§4.2.1.6 rule 3] The value without z and m: of x and y alone.
    #[must_use]
    pub fn xy(&self) -> Geom {
        xy::of(self)
    }

    /// The ordinate `at` of every point of the value's linear form, a
    /// solid's of its own points: x at 0, y at 1, then z and m as the
    /// dimensions have them.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a value the linear form does not take.
    pub fn ordinates(&self, at: usize) -> Result<Vec<f64>> {
        let mut body = if matches!(self.body, Body::Solid(_)) { self.body.clone() } else { self.linear()?.body };
        let stride = self.dims.stride();
        let (mut out, mut i) = (Vec::new(), 0_usize);
        parts::numbers_mut(&mut body, &mut |x: &mut f64| {
            if i.checked_rem(stride) == Some(at) {
                out.push(*x);
            }
            i = i.wrapping_add(1);
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
