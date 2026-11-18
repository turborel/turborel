//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.4, §15.4.5, §15.6.8,
//! §15.9.1 Description 15 to 24] Locating a position expression along an
//! ST_LRCurve: ST_Point, ST_LRPosition, ST_Equals, and whether a
//! referent's position and location agree.
//!
//! The text says what each LRM type measures and leaves the arithmetic to
//! the implementation. Implementation-defined choices:
//!
//! - An "absolute" distance along less the start value of the element in
//!   that LRM [§15.9.1 Description 16a iii, "modified by M"] is the
//!   distance from the start, converted from its unit to the curve's.
//! - An "interpolative" distance along is that part of the curve's length
//!   the distance bears to the element's default measure [Description 17].
//! - A "local interpolative" distance along is an m value: the place is
//!   the first where the m coordinates interpolate to it [Description 18].
//! - A "relative" distance along is measured from the place on the curve
//!   nearest the "from" referent, towards the "towards" referent if one
//!   is given, else in the curve's direction; with no referent, from the
//!   start [Description 19 to 21]. A referent is found among the
//!   catalog's features: the one its feature id names, or, that being the
//!   null value, the first feature that has a referent of its name
//!   [Description 23, 24].
//! - Only an ST_LRCurve has a geometry to locate along: a position along an
//!   ST_LRFeature or ST_LRDirectedEdge is not located.

use super::lrm::{ABSOLUTE, INTERPOLATIVE, LOCAL_INTERPOLATIVE, RELATIVE};
use super::measure::{Path, RELATIVE_TOLERANCE, Station, Vertex, point_geom, point_vertex};
use super::model::{DistanceExp, LinearElement, LrCurve, LrMeasure, Lrm, PositionExp, Referent};
use super::offset::{apply, from_coords, lateral_of, to_coords, vertical_of};
use super::{Catalog, deeper, failed, required};
use crate::error::{Result, SqlError, SqlState};
use crate::geometry::model::Geom;

/// A position expression located: its place, the SRID of its curve and
/// the tolerance of places on that curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Located {
    /// The place.
    pub at: Vertex,
    /// The SRID of the curve.
    pub srid: i32,
    /// `Path::tolerance` of the curve.
    pub tolerance: f64,
}

/// What locating along one curve needs.
#[derive(Clone, Copy)]
struct Along<'a> {
    /// The curve.
    curve: &'a LrCurve,
    /// Its path.
    path: &'a Path,
    /// The LRM the distance is measured in.
    lrm: &'a Lrm,
    /// The catalog.
    cat: &'a Catalog<'a>,
}

impl LrCurve {
    /// [§15.4.4 Description 2] ST_Point(ST_PositionExp): the point the
    /// position expression locates, along this curve. Implementation-
    /// defined: a position expression naming another linear element is
    /// refused, and the point has the z and m the curve has there.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 2a); 2FF02 (invalid argument) of a
    /// position along another element; as the location fails.
    pub fn point(&self, apositionexpression: Option<&PositionExp>, catalog: &Catalog<'_>) -> Result<Geom> {
        let pe = required(apositionexpression, "§15.4.4 Description 2a", "apositionexpression")?;
        if pe.element_id != self.base.id {
            return Err(failed(
                SqlState::SPATIAL_INVALID_ARGUMENT,
                "§15.4.4 Description 2b",
                &format!("the position expression is along the linear element {}, not {}", pe.element_id, self.base.id),
            ));
        }
        let located = locate_on(self, pe, catalog, 0)?;
        point_geom(located.at, self.curve.srid)
    }

    /// [§15.4.5 Description 2 to 5] ST_LRPosition(ST_Point): the position
    /// along this curve, in its default LRM, nearest `apoint`. Offsets
    /// [Description 4] are given only by an LRM that allows them, of a
    /// point off the curve: a lateral offset, and a vertical one if both
    /// have z and they differ.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 2a); 2FF10 (mixed spatial reference systems) of a
    /// point of another SRID; 2FF81 (invalid LRM) of a default LRM not in
    /// the catalog; 2FF84 (missing measure values) of a local
    /// interpolative LRM along a curve without m.
    pub fn lr_position(&self, apoint: Option<&Geom>, catalog: &Catalog<'_>) -> Result<PositionExp> {
        let point = required(apoint, "§15.4.5 Description 2a", "apoint")?;
        if point.srid != self.curve.srid {
            return Err(failed(
                SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS,
                "§15.4.5 Description 2b",
                "mixed spatial reference systems",
            ));
        }
        let at = point_vertex(point)?;
        let lrm = catalog.lrm(self.base.default_lrm).ok_or_else(|| no_lrm(self.base.default_lrm, "§15.4.5 Description 2b"))?;
        let (d, _) = distance_to(self, lrm, &at, catalog)?;
        Ok(PositionExp::of_element_and_lrm(LinearElement::Curve(self.clone()), lrm.clone(), d))
    }
}

/// [§15.4.5 Description 3 to 5] The distance expression, in `lrm`, of the
/// place on `curve` nearest `at`, with the offsets that reach `at` if
/// `lrm` allows offsets; and how far `at` is from `curve` beyond its
/// tolerance, 0 if it is on it.
///
/// # Errors
///
/// As `LrCurve::lr_position`.
pub(crate) fn distance_to(curve: &LrCurve, lrm: &Lrm, at: &Vertex, cat: &Catalog<'_>) -> Result<(DistanceExp, f64)> {
    let path = Path::of_curve(&curve.curve)?;
    let foot = path.foot(at.x, at.y);
    let along = Along { curve, path: &path, lrm, cat };
    let mut d = DistanceExp::along(along.measure_at(&foot.station)?);
    let tolerance = path.tolerance();
    let dz = at.z.zip(foot.station.at.z).map(|(p, c)| p - c).filter(|dz| dz.abs() > tolerance);
    let off = if foot.distance > tolerance { foot.distance } else { 0.0_f64 };
    if let Some(unit) = lrm.offset_units.as_deref() {
        if off > 0.0_f64 {
            d.lateral = Some(lateral_of(&foot, unit, lrm, cat)?);
        }
        if let Some(dz) = dz {
            d.vertical = Some(vertical_of(dz, unit, lrm, cat)?);
        }
    }
    Ok((d, off.hypot(dz.unwrap_or_default())))
}

impl PositionExp {
    /// [§15.6.8 Description 2] ST_Equals(ST_PositionExp): 1 if SELF and
    /// `apositionexpression` are the same linearly referenced location, 0
    /// if not, the null value of SELF the null value. Implementation-
    /// defined: equal values are the same location; two that locate along
    /// curves are the same if their places are within the tolerance; two
    /// along the same element in the same LRM, of no referent and no
    /// offset, are the same if their distances along are; of any others it
    /// is not possible to determine.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 2a); 2FF88 (indeterminate equality) (Description 2c i).
    pub fn equals(this: Option<&PositionExp>, apositionexpression: Option<&PositionExp>, catalog: &Catalog<'_>) -> Result<Option<i32>> {
        let other = required(apositionexpression, "§15.6.8 Description 2a", "apositionexpression")?;
        let Some(me) = this else { return Ok(None) };
        if me == other {
            return Ok(Some(1));
        }
        if let (Ok(a), Ok(b)) = (locate(me, catalog, 0), locate(other, catalog, 0))
            && a.srid == b.srid
        {
            return Ok(Some(i32::from(close(&a.at, &b.at, a.tolerance.max(b.tolerance)))));
        }
        same_measure(me, other, catalog)
            .map(|same| Some(i32::from(same)))
            .ok_or_else(|| failed(SqlState::SPATIAL_INDETERMINATE_EQUALITY, "§15.6.8 Description 2c i", "indeterminate equality"))
    }
}

/// [§15.6.8 Description 2c] Whether two positions along the same element
/// in the same LRM, of no referent and no offset, have the same distance
/// along; none of others.
fn same_measure(a: &PositionExp, b: &PositionExp, cat: &Catalog<'_>) -> Option<bool> {
    let comparable = a.element_id == b.element_id && a.lrm_id == b.lrm_id;
    let plain = |p: &PositionExp| !p.distance.has_referent() && !p.distance.has_offset();
    if !comparable || !plain(a) || !plain(b) {
        return None;
    }
    let lrm = a.lrm.as_ref().or_else(|| cat.lrm(a.lrm_id))?;
    let ma = to_coords(&a.distance.along, &lrm.units, cat).ok()?;
    let mb = to_coords(&b.distance.along, &lrm.units, cat).ok()?;
    Some((ma - mb).abs() <= RELATIVE_TOLERANCE * ma.abs().max(mb.abs()).max(1.0_f64))
}

/// Whether `a` and `b` are within `tolerance`, in the plane and in z if
/// both have z.
fn close(a: &Vertex, b: &Vertex, tolerance: f64) -> bool {
    let dz = a.z.zip(b.z).map_or(0.0_f64, |(za, zb)| za - zb);
    (a.x - b.x).hypot(a.y - b.y).hypot(dz) <= tolerance
}

/// [§15.10.2 Description 8b and its peers] Whether the point `position`
/// and the place `location` locates are known to be the same place: an
/// error saying why not, if not.
///
/// # Errors
///
/// The reason the places are not known to be the same.
pub(crate) fn same_place(position: &Geom, location: &PositionExp, cat: &Catalog<'_>) -> Result<()> {
    let at = point_vertex(position)?;
    let located = locate(location, cat, 0)?;
    if located.srid != position.srid {
        return Err(failed(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§15.10.1", "mixed spatial reference systems"));
    }
    if close(&at, &located.at, located.tolerance) {
        return Ok(());
    }
    Err(failed(SqlState::SPATIAL_INVALID_ARGUMENT, "§15.10.1", "the position is not the place the location gives"))
}

/// The place `pe` locates, along the curve it names.
///
/// # Errors
///
/// SQLSTATE 2FF02 of an element not in the catalog or not an
/// ST_LRCurve; as `locate_on`.
pub(crate) fn locate(pe: &PositionExp, cat: &Catalog<'_>, depth: usize) -> Result<Located> {
    let element = pe.element.as_deref().or_else(|| cat.element(pe.element_id)).ok_or_else(|| {
        failed(SqlState::SPATIAL_INVALID_ARGUMENT, "§15.6.1 Description 13", &format!("no linear element has the leid {}", pe.element_id))
    })?;
    let curve = element.as_curve().ok_or_else(|| {
        failed(
            SqlState::SPATIAL_INVALID_ARGUMENT,
            "§15.4.4",
            &format!("the linear element {} is an {}, not an ST_LRCurve", pe.element_id, element.kind().sql_name()),
        )
    })?;
    locate_on(curve, pe, cat, depth)
}

/// The place `pe` locates along `curve`.
///
/// # Errors
///
/// SQLSTATE 2FF81 (invalid LRM) of an LRM not in the catalog or of no
/// known type; 2FF79 (distance longer than curve); 2FF84 (missing measure
/// values); 2FF02; as the units and offsets fail.
pub(crate) fn locate_on(curve: &LrCurve, pe: &PositionExp, cat: &Catalog<'_>, depth: usize) -> Result<Located> {
    let next = deeper(depth)?;
    let lrm = pe.lrm.as_ref().or_else(|| cat.lrm(pe.lrm_id)).ok_or_else(|| no_lrm(pe.lrm_id, "§15.6.1 Description 12"))?;
    let path = Path::of_curve(&curve.curve)?;
    let along = Along { curve, path: &path, lrm, cat };
    let station = along.station(&pe.distance, next)?;
    let at = apply(&station, &pe.distance, lrm, cat)?;
    Ok(Located { at, srid: curve.curve.srid, tolerance: path.tolerance() })
}

impl Along<'_> {
    /// [§15.9.1 Description 16 to 21] The station `d` measures to.
    fn station(&self, d: &DistanceExp, depth: usize) -> Result<Station> {
        let lrm_type = self.lrm.lrm_type.to_ascii_lowercase();
        if d.has_referent() && lrm_type != RELATIVE {
            return Err(failed(
                SqlState::SPATIAL_INVALID_ARGUMENT,
                "§15.9.1 Description 19",
                &format!("a distance expression of an LRM of type {} has a referent", self.lrm.lrm_type),
            ));
        }
        let s = match lrm_type.as_str() {
            ABSOLUTE => to_coords(&d.along, &self.lrm.units, self.cat)? - self.start()?,
            INTERPOLATIVE => self.along_default(&d.along)? * self.path.length(),
            LOCAL_INTERPOLATIVE => self.at_m(d.along.measure)?,
            RELATIVE => self.relative(d, depth)?,
            _ => return Err(unknown_type(self.lrm)),
        };
        let tolerance = self.path.tolerance();
        if s < -tolerance || s > self.path.length() + tolerance {
            return Err(failed(SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE, "§15.9.1 Description 31", "distance longer than curve"));
        }
        Ok(self.path.at(s))
    }

    /// [§15.4.5 Description 3] The distance along, in the LRM, of
    /// `station`, measured from the start.
    fn measure_at(&self, station: &Station) -> Result<LrMeasure> {
        let units = &self.lrm.units;
        let measure = match self.lrm.lrm_type.to_ascii_lowercase().as_str() {
            ABSOLUTE => from_coords(station.s + self.start()?, units, self.cat)?,
            RELATIVE => from_coords(station.s, units, self.cat)?,
            INTERPOLATIVE => {
                let length = self.path.length();
                if length <= 0.0_f64 {
                    return Err(by_zero("§15.9.1 Description 17"));
                }
                let m = &self.curve.base.default_measure;
                return Ok(LrMeasure { measure: station.s / length * m.measure, units: m.units.clone() });
            }
            LOCAL_INTERPOLATIVE => station.at.m.ok_or_else(missing_m)?,
            _ => return Err(unknown_type(self.lrm)),
        };
        Ok(LrMeasure { measure, units: Some(units.clone()) })
    }

    /// [§15.9.1 Description 16a] The element's start value in the LRM, in
    /// coordinate units; 0 if it has none.
    fn start(&self) -> Result<f64> {
        let found = self.curve.base.start_values.iter().flatten().find(|s| s.lrm == self.lrm.id);
        found.map_or(Ok(0.0_f64), |s| to_coords(&s.measure, &self.lrm.units, self.cat))
    }

    /// [§15.9.1 Description 17] The part of the default measure `along` is.
    fn along_default(&self, along: &LrMeasure) -> Result<f64> {
        let whole = to_coords(&self.curve.base.default_measure, &self.lrm.units, self.cat)?;
        if whole <= 0.0_f64 {
            return Err(by_zero("§15.9.1 Description 17"));
        }
        Ok(to_coords(along, &self.lrm.units, self.cat)? / whole)
    }

    /// [§15.9.1 Description 18] The distance along of the m value `m`.
    fn at_m(&self, m: f64) -> Result<f64> {
        if !self.path.is_measured() {
            return Err(missing_m());
        }
        self.path.s_at_m(m).ok_or_else(|| {
            failed(
                SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE,
                "§15.9.1 Description 18",
                "distance longer than curve: no m value brackets it",
            )
        })
    }

    /// [§15.9.1 Description 19 to 21] The distance along from the start of
    /// a relative distance expression.
    fn relative(&self, d: &DistanceExp, depth: usize) -> Result<f64> {
        let along = to_coords(&d.along, &self.lrm.units, self.cat)?;
        let Some(from) = d.from_name.as_deref() else { return Ok(along) };
        let start = self.referent_s(d.from_feature.as_deref(), from, depth)?;
        let Some(towards) = d.towards_name.as_deref() else { return Ok(start + along) };
        let end = self.referent_s(d.towards_feature.as_deref(), towards, depth)?;
        Ok(if end < start { start - along } else { start + along })
    }

    /// The distance along of the place on the curve nearest the referent
    /// `name` of the feature `feature`.
    fn referent_s(&self, feature: Option<&str>, name: &str, depth: usize) -> Result<f64> {
        let r = find_referent(self.cat, feature, name)?;
        let at = referent_place(r, self.cat, depth)?;
        Ok(self.path.foot(at.x, at.y).station.s)
    }
}

/// [§15.9.1 Description 23, 24] The referent `name` of the feature
/// `feature`, or of the first feature having one of that name.
fn find_referent<'a>(cat: &Catalog<'a>, feature: Option<&str>, name: &str) -> Result<&'a Referent> {
    let features = cat.elements.iter().filter_map(LinearElement::as_feature);
    let mut of = features.filter(|f| feature.is_none_or(|id| f.feature_id == id));
    of.find_map(|f| f.referents.iter().flatten().find(|r| r.name == name)).ok_or_else(|| {
        failed(
            SqlState::SPATIAL_INVALID_ARGUMENT,
            "§15.9.1 Description 23",
            &format!("no feature of the catalog has a referent named {name}"),
        )
    })
}

/// [§15.10.1] The place of a referent: its position, else the place its
/// location gives.
fn referent_place(r: &Referent, cat: &Catalog<'_>, depth: usize) -> Result<Vertex> {
    if let Some(p) = &r.position {
        return point_vertex(p);
    }
    match &r.location {
        Some(at) => Ok(locate(at, cat, depth)?.at),
        None => {
            Err(failed(SqlState::SPATIAL_INVALID_ARGUMENT, "§15.10.1", &format!("the referent {} has no position and no location", r.name)))
        }
    }
}

/// SQLSTATE 2FF81: no LRM of the catalog has the id `id`.
fn no_lrm(id: i32, rule: &str) -> SqlError {
    failed(SqlState::SPATIAL_INVALID_LRM, rule, &format!("invalid LRM: no LRM has the lrmid {id}"))
}

/// SQLSTATE 2FF81: an LRM of a type §15.1.1 Description 13 does not name.
fn unknown_type(lrm: &Lrm) -> SqlError {
    failed(
        SqlState::SPATIAL_INVALID_LRM,
        "§15.1.1 Description 13",
        &format!("invalid LRM: the LRM type {} is not one of the four", lrm.lrm_type),
    )
}

/// SQLSTATE 2FF84: a local interpolative LRM along a curve without m.
fn missing_m() -> SqlError {
    failed(SqlState::SPATIAL_MISSING_MEASURE_VALUES, "§15.9.1 Description 18", "missing measure values")
}

/// SQLSTATE 2FF13, cited as `rule`.
fn by_zero(rule: &str) -> SqlError {
    failed(SqlState::SPATIAL_ATTEMPTED_DIVISION_BY_ZERO, rule, "attempted division by zero")
}
