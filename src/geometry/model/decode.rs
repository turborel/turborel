//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] A value read from its
//! well-known binary representation: as ST_GeomFromWKB reads one given it
//! (`read_wkb`), and as a stored value is read back (`read_canonical`).
//!
//! Given one, either byte order is read at each component, the deprecated
//! codes of Annex C too, negative zero is read as zero, and a value whose
//! optional part is left out is empty where nothing follows it; else the
//! empty forms `model/encode.rs` writes are read. Stored, only what the
//! writer writes is read, so one value has one form (RULES.md
//! D119). Counts are checked against the bytes left and the
//! shared part budget before anything is allocated.

use super::check::{failed, limited};
use super::encode::{BIG_ENDIAN, EMPTY_ORDINATE, LITTLE_ENDIAN};
use super::{Body, Collection, Coords, Curve, Dims, Geom, Kind, Surface, WKB_M, WKB_Z, WKB_ZM};
use crate::error::{Result, SqlError, SqlState};
use crate::geometry::{MAX_BYTES, MAX_DEPTH, MAX_PARTS};

/// The bytes of a byte order and a type code.
pub(super) const HEADER_BYTES: usize = 5;
/// The bytes of a count.
pub(super) const COUNT_BYTES: usize = 4;
/// The bytes of a number.
pub(super) const NUMBER_BYTES: usize = 8;
/// A triangle's points as written.
const TRIANGLE_POINTS: usize = 3;
/// Negative zero's bits, which no stored number has.
const NEGATIVE_ZERO: u64 = 0x8000_0000_0000_0000;

/// [§5.1.68] The value of the well-known binary representation `bytes`
/// given, of SRID `srid` [§5.1.63].
///
/// # Errors
///
/// SQLSTATE 2FF23 (invalid well-known binary representation) if `bytes`
/// is none, or holds bytes after it; 2FF25 of parts of mixed coordinate
/// dimensions; the model's (`Geom::new`) of a value that is no value of
/// its type; SQLCODE -900 past a representation limit.
pub fn read_wkb(bytes: &[u8], srid: i32) -> Result<Geom> {
    limited(bytes.len(), MAX_BYTES)?;
    let (dims, body) = read(bytes, false)?;
    Geom::new(srid, dims, body)
}

/// `read_wkb`, of a value of `kind` or a subtype of it, as the functions
/// of a type read one [§6.1.9, §7.2.8 and so on].
///
/// # Errors
///
/// As `read_wkb`; SQLSTATE 2FF23 of a value of another type.
pub fn read_wkb_typed(bytes: &[u8], srid: i32, kind: Kind) -> Result<Geom> {
    let g = read_wkb(bytes, srid)?;
    if !g.kind().is_subtype_of(kind) {
        return Err(wrong(&format!("the well-known binary representation is of {}, not {}", g.kind().sql_name(), kind.sql_name())));
    }
    Ok(g)
}

/// The dimensions and body of canonical bytes, as `model/encode.rs` writes
/// them.
pub(super) fn read_canonical(bytes: &[u8]) -> Result<(Dims, Body)> {
    read(bytes, true)
}

fn read(bytes: &[u8], canonical: bool) -> Result<(Dims, Body)> {
    let mut r = Reader { rest: bytes, parts: 0, canonical, dims: None };
    let (_, body) = r.geometry(1)?;
    if !r.rest.is_empty() {
        return Err(wrong("bytes follow the well-known binary representation"));
    }
    let dims = r.dims.unwrap_or(Dims::Xy);
    super::check::check(&Geom { srid: 0, dims, body: body.clone() })?;
    let mut g = Geom { srid: 0, dims, body };
    super::check::normalize(&mut g);
    Ok((g.dims, g.body))
}

/// SQLSTATE 2FF23, invalid well-known binary representation [§22].
pub(super) fn wrong(detail: &str) -> SqlError {
    failed(SqlState::SPATIAL_INVALID_WKB, "§5.1.68", detail)
}

/// A read through well-known binary bytes.
pub(super) struct Reader<'a> {
    /// The bytes not yet read.
    pub(super) rest: &'a [u8],
    /// The parts read, against `MAX_PARTS`.
    parts: usize,
    /// Whether the bytes are a stored value's.
    pub(super) canonical: bool,
    /// The value's dimensions, once its first header is read.
    pub(super) dims: Option<Dims>,
}

impl Reader<'_> {
    pub(super) fn charge(&mut self, count: usize) -> Result<()> {
        self.parts = self.parts.saturating_add(count);
        limited(self.parts, MAX_PARTS)
    }

    pub(super) fn dims(&self) -> Dims {
        self.dims.unwrap_or(Dims::Xy)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (taken, rest) = self.rest.split_at_checked(N).ok_or_else(|| wrong("the bytes end too soon"))?;
        self.rest = rest;
        taken.try_into().map_err(|_| wrong("the bytes end too soon"))
    }

    pub(super) fn byte(&mut self) -> Result<u8> {
        let [b] = self.take()?;
        Ok(b)
    }

    /// `<byte order>`: whether little-endian.
    pub(super) fn order(&mut self) -> Result<bool> {
        match (self.byte()?, self.canonical) {
            (LITTLE_ENDIAN, _) => Ok(true),
            (BIG_ENDIAN, false) => Ok(false),
            (BIG_ENDIAN, true) | (2..=u8::MAX, _) => Err(wrong("a byte order is neither 0 nor 1, or a stored value's is not 1")),
        }
    }

    pub(super) fn uint(&mut self, little: bool) -> Result<u32> {
        let bytes = self.take()?;
        Ok(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    }

    pub(super) fn int(&mut self, little: bool) -> Result<i32> {
        let bytes = self.take()?;
        Ok(if little { i32::from_le_bytes(bytes) } else { i32::from_be_bytes(bytes) })
    }

    /// A count of parts of at least `width` bytes each, checked against the
    /// bytes left and the part budget.
    pub(super) fn count(&mut self, little: bool, width: usize) -> Result<usize> {
        let count = usize::try_from(self.uint(little)?).map_err(|_| wrong("a count is too large"))?;
        limited(count, MAX_PARTS.saturating_sub(self.parts))?;
        if count.saturating_mul(width) > self.rest.len() {
            return Err(wrong("a count exceeds the bytes left"));
        }
        Ok(count)
    }

    /// A number: of a stored value, never negative zero.
    pub(super) fn number(&mut self, little: bool) -> Result<f64> {
        let bytes = self.take()?;
        let n = if little { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) };
        if self.canonical && n.to_bits() == NEGATIVE_ZERO {
            return Err(wrong("a stored number is negative zero"));
        }
        Ok(n)
    }

    pub(super) fn numbers(&mut self, little: bool, count: usize) -> Result<Coords> {
        if count.saturating_mul(NUMBER_BYTES) > self.rest.len() {
            return Err(wrong("the bytes end too soon"));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.number(little)?);
        }
        Ok(out)
    }

    /// Numbers that are a part's, or NaN alike if it is empty: none then.
    /// A stored empty part's NaN is `EMPTY_ORDINATE` exactly.
    pub(super) fn maybe_empty(&mut self, little: bool, count: usize) -> Result<Option<Coords>> {
        let numbers = self.numbers(little, count)?;
        if !numbers.iter().any(|n| n.is_nan()) {
            return Ok(Some(numbers));
        }
        let all = if self.canonical { numbers.iter().all(|n| n.to_bits() == EMPTY_ORDINATE) } else { numbers.iter().all(|n| n.is_nan()) };
        if !all {
            return Err(wrong("a part's numbers are NaN, and not all of them as an empty part's are"));
        }
        Ok(None)
    }

    /// Whether an optional part is left out: nothing follows, which a
    /// value given may do and a stored one does not.
    pub(super) fn omitted(&self) -> bool {
        !self.canonical && self.rest.is_empty()
    }

    /// A component: its byte order, its code and its body; its dimensions
    /// those of every component before it.
    pub(super) fn geometry(&mut self, depth: usize) -> Result<(Kind, Body)> {
        limited(depth, MAX_DEPTH)?;
        self.charge(1)?;
        let little = self.order()?;
        let code = self.uint(little)?;
        let (kind, dims) = code_of(code, self.canonical)?;
        if *self.dims.get_or_insert(dims) != dims {
            return Err(failed(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§5.1.68", "a component's coordinate dimensions differ"));
        }
        let body = self.body(kind, little, depth)?;
        Ok((kind, body))
    }

    fn body(&mut self, kind: Kind, little: bool, depth: usize) -> Result<Body> {
        let stride = self.dims().stride();
        let deeper = depth.saturating_add(1);
        Ok(match kind {
            Kind::Point => Body::Point(if self.omitted() { Vec::new() } else { self.maybe_empty(little, stride)?.unwrap_or_default() }),
            Kind::LineString => Body::Curve(Curve::LineString(self.points(little)?)),
            Kind::CircularString => Body::Curve(Curve::CircularString(self.points(little)?)),
            Kind::Circle => Body::Curve(Curve::Circle(self.points(little)?)),
            Kind::GeodesicString => Body::Curve(Curve::GeodesicString(self.points(little)?)),
            Kind::EllipticalCurve => Body::Curve(Curve::Elliptical(self.elliptical(little)?)),
            Kind::NurbsCurve => Body::Curve(Curve::Nurbs(self.nurbs(little)?)),
            Kind::Clothoid => Body::Curve(Curve::Clothoid(self.clothoid(little)?)),
            Kind::SpiralCurve => Body::Curve(Curve::Spiral(self.spiral(little)?)),
            Kind::CompoundCurve => Body::Curve(Curve::Compound(self.parts(little, deeper, curve_of)?)),
            Kind::CurvePolygon => Body::Surface(Surface::CurvePolygon(self.parts(little, deeper, curve_of)?)),
            Kind::Polygon => Body::Surface(Surface::Polygon(self.rings(little)?)),
            Kind::Triangle => Body::Surface(Surface::Triangle(self.triangle(little)?)),
            Kind::PolyhdrlSurface => Body::Surface(Surface::PolyhdrlSurface(self.parts(little, deeper, patch_of)?)),
            Kind::Tin => Body::Surface(Surface::Tin(self.tin(little, deeper)?)),
            Kind::CompoundSurface => Body::Surface(Surface::Compound(self.parts(little, deeper, surface_of)?)),
            Kind::BRepSolid => Body::Solid(self.parts(little, deeper, surface_of)?),
            Kind::GeomCollection => Body::Collection(Collection::Geometry(self.parts(little, deeper, |_, b| Some(b))?)),
            Kind::MultiPoint => Body::Collection(Collection::MultiPoint(self.parts(little, deeper, point_of)?)),
            Kind::MultiCurve => Body::Collection(Collection::MultiCurve(self.parts(little, deeper, curve_of)?)),
            Kind::MultiLineString => Body::Collection(Collection::MultiLineString(self.parts(little, deeper, line_of)?)),
            Kind::MultiSurface => Body::Collection(Collection::MultiSurface(self.parts(little, deeper, surface_of)?)),
            Kind::MultiPolygon => Body::Collection(Collection::MultiPolygon(self.parts(little, deeper, polygon_of)?)),
            Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Solid => return Err(wrong("a type code names no instantiable type")),
        })
    }

    /// `<num>` points, or none if left out.
    pub(super) fn points(&mut self, little: bool) -> Result<Coords> {
        if self.omitted() {
            return Ok(Vec::new());
        }
        let stride = self.dims().stride();
        let count = self.count(little, stride.saturating_mul(NUMBER_BYTES))?;
        self.charge(count)?;
        self.numbers(little, count.saturating_mul(stride))
    }

    fn rings(&mut self, little: bool) -> Result<Vec<Coords>> {
        if self.omitted() {
            return Ok(Vec::new());
        }
        let count = self.count(little, COUNT_BYTES)?;
        self.charge(count)?;
        let mut rings = Vec::with_capacity(count);
        for _ in 0..count {
            rings.push(self.points(little)?);
        }
        Ok(rings)
    }

    /// Three points, written as a triangle's, as its ring of four.
    pub(super) fn triangle(&mut self, little: bool) -> Result<Coords> {
        if self.omitted() {
            return Ok(Vec::new());
        }
        let stride = self.dims().stride();
        let Some(mut ring) = self.maybe_empty(little, TRIANGLE_POINTS.saturating_mul(stride))? else { return Ok(Vec::new()) };
        self.charge(TRIANGLE_POINTS)?;
        let first: Vec<f64> = ring.get(..stride).map(<[f64]>::to_vec).unwrap_or_default();
        ring.extend(first);
        Ok(ring)
    }

    /// `<num>` components, each read whole and taken by `of`, which gives
    /// none of a component not of the type its place needs.
    pub(super) fn parts<T>(&mut self, little: bool, depth: usize, of: impl Fn(Kind, Body) -> Option<T>) -> Result<Vec<T>> {
        if self.omitted() {
            return Ok(Vec::new());
        }
        let count = self.count(little, HEADER_BYTES)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let (kind, body) = self.geometry(depth)?;
            out.push(of(kind, body).ok_or_else(|| {
                failed(
                    SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
                    "§5.1.68",
                    &format!("a component of {} is not of a type its place takes", kind.sql_name()),
                )
            })?);
        }
        Ok(out)
    }
}

/// [Table 15] The type and dimensions of a code; of a stored value, no
/// deprecated code.
fn code_of(code: u32, canonical: bool) -> Result<(Kind, Dims)> {
    let deprecated = code > super::WKB_DEPRECATED_BASE;
    let (base, dims) = if deprecated || code < WKB_Z {
        (code, Dims::Xy)
    } else if code < WKB_M {
        (code.saturating_sub(WKB_Z), Dims::Xyz)
    } else if code < WKB_ZM {
        (code.saturating_sub(WKB_M), Dims::Xym)
    } else {
        (code.saturating_sub(WKB_ZM), Dims::Xyzm)
    };
    let kind =
        Kind::of_wkb_code(base).filter(|_| !(deprecated && canonical)).ok_or_else(|| wrong(&format!("no type has the code {code}")))?;
    // ST_BRepSolid is of z alone: 1025 [Table 15].
    if kind == Kind::BRepSolid && dims != Dims::Xyz {
        return Err(wrong("ST_BRepSolid's code is 1025, of z"));
    }
    Ok((kind, dims))
}

fn point_of(kind: Kind, body: Body) -> Option<Coords> {
    if let (Kind::Point, Body::Point(c)) = (kind, body) { Some(c) } else { None }
}

fn line_of(kind: Kind, body: Body) -> Option<Coords> {
    if let (Kind::LineString, Body::Curve(Curve::LineString(c))) = (kind, body) { Some(c) } else { None }
}

pub(super) fn curve_of(_: Kind, body: Body) -> Option<Curve> {
    if let Body::Curve(c) = body { Some(c) } else { None }
}

fn surface_of(_: Kind, body: Body) -> Option<Surface> {
    if let Body::Surface(s) = body { Some(s) } else { None }
}

/// [§5.1.68 `<wkbpolygonpatch binary>`] A polygon, a triangle's ring as a
/// polygon's.
fn polygon_of(_: Kind, body: Body) -> Option<Vec<Coords>> {
    match body {
        Body::Surface(Surface::Polygon(rings)) => Some(rings),
        Body::Surface(Surface::Triangle(ring)) => Some(if ring.is_empty() { Vec::new() } else { vec![ring] }),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
    }
}

/// A polyhedral surface's patch: a polygon, not empty.
fn patch_of(kind: Kind, body: Body) -> Option<Vec<Coords>> {
    polygon_of(kind, body).filter(|rings| !rings.is_empty())
}
