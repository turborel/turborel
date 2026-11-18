//! The coordinate transform of ST_Transform (ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1.6), from one `<spatial reference system>` to another.
//! §5.1.6 Description 3d leaves the transform implementation-defined;
//! this one is the following pipeline.
//!
//! 1. Coordinates are put in east, north, up order by the source's AXIS
//!    items, or by the default order of each kind of system when it has
//!    none: longitude then latitude, easting then northing, and X, Y, Z.
//! 2. A projected system is unprojected; units become metres and radians
//!    and longitudes are taken from Greenwich, not the prime meridian.
//! 3. If the two datums differ, the point goes through geocentric X, Y, Z
//!    with each datum's TOWGS84 Helmert shift to WGS 84 and back. A WGS 84
//!    datum needs none. Datums with neither cannot be related, and the
//!    transform fails with F15 (§5.1.6 Description 3c).
//! 4. The target's steps are applied in reverse.
//!
//! A z coordinate is an ellipsoidal height in the system's linear unit,
//! metres when it has none; m coordinates pass through unchanged (§5.1.6
//! Description 6 asks them considered, and a datum change does not alter
//! a measure). Without z, heights are taken as zero and dropped.

use std::f64::consts::PI;

use super::ellipsoid::{Ellipsoid, Shift, WGS84_INVERSE_FLATTENING, WGS84_SEMI_MAJOR};
use super::fault::{Fault, Outcome};
use super::model::{Axis, CoordinateSystem, Datum, Direction, PrimeMeridian};
use super::projection::{Projector, normalize, wrap};

/// Names of the WGS 84 datum, normalized as projection names are.
const WGS84_NAMES: [&str; 4] = ["wgs_1984", "wgs_84", "wgs84", "world_geodetic_system_1984"];
/// Relative tolerance on ellipsoid parameters taken as WGS 84's.
const SAME_ELLIPSOID: f64 = 1e-12;
/// Radians per degree, the unit of a geocentric prime meridian.
const DEGREE_RADIANS: f64 = PI / 180.0;

/// Axis order and signs mapping stored coordinates to east, north, up.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axes {
    swap: bool,
    signs: [f64; 3],
}

/// What one system's coordinates are.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// Angular unit and height unit factors.
    Geographic { angular: f64, height: f64 },
    /// Linear unit factor and the projection.
    Projected { linear: f64, projector: Box<Projector> },
    /// Linear unit factor.
    Geocentric { linear: f64 },
}

/// One side of a transform.
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    kind: Kind,
    ellipsoid: Ellipsoid,
    /// Prime meridian longitude from Greenwich, radians.
    meridian: f64,
    axes: Axes,
}

/// A point between the two halves of the pipeline.
#[derive(Clone, Copy)]
enum Point {
    /// Longitude from Greenwich and latitude, radians; height, metres.
    Geodetic([f64; 3]),
    /// Geocentric X, Y, Z, metres, X through Greenwich.
    Geocentric([f64; 3]),
}

/// A prepared transform between two coordinate systems.
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    from: Frame,
    to: Frame,
    /// None if the datums are the same; else each side's shift to WGS 84,
    /// none for WGS 84 itself.
    shift: Option<(Option<Shift>, Option<Shift>)>,
}

fn failed(message: &str) -> Fault {
    Fault::transform(format!("[ISO/IEC 13249-3:2016 §5.1.6 Description 3c] {message}"))
}

fn sign(direction: Direction) -> f64 {
    match direction {
        Direction::North | Direction::East | Direction::Up | Direction::Other => 1.0_f64,
        Direction::South | Direction::West | Direction::Down => -1.0_f64,
    }
}

fn is_east(direction: Direction) -> bool {
    matches!(direction, Direction::East | Direction::West)
}

fn is_north(direction: Direction) -> bool {
    matches!(direction, Direction::North | Direction::South)
}

/// The order of `list`: an east and a north axis in either order, then
/// an up or down axis.
fn axes(list: &[Axis]) -> Outcome<Axes> {
    let mut out = Axes { swap: false, signs: [1.0_f64; 3] };
    let mut iter = list.iter().map(|axis| axis.direction);
    if let (Some(first), Some(second)) = (iter.next(), iter.next()) {
        out.swap = is_north(first) && is_east(second);
        if !(out.swap || (is_east(first) && is_north(second))) {
            return Err(failed("the axes are not an east and a north axis"));
        }
        let (east, north) = if out.swap { (second, first) } else { (first, second) };
        out.signs = [sign(east), sign(north), 1.0_f64];
    }
    if let Some(third) = iter.next() {
        if is_east(third) || is_north(third) {
            return Err(failed("the third axis is not vertical"));
        }
        out.signs[2] = sign(third);
    }
    Ok(out)
}

/// The prime meridian's longitude in radians; a geocentric system has no
/// angular unit, so its `<longitude>` is read in degrees.
fn meridian(pm: &PrimeMeridian, angular: Option<f64>) -> f64 {
    pm.longitude * angular.unwrap_or(DEGREE_RADIANS)
}

fn ellipsoid(datum: &Datum) -> Outcome<Ellipsoid> {
    Ellipsoid::new(datum.spheroid.semi_major, datum.spheroid.inverse_flattening)
}

fn frame(system: &CoordinateSystem) -> Outcome<Frame> {
    match system {
        CoordinateSystem::Geographic(cs) => {
            let angular = cs.angular_unit.factor;
            let height = cs.linear_unit.as_ref().map_or(1.0_f64, |unit| unit.factor);
            let kind = Kind::Geographic { angular, height };
            Ok(Frame {
                kind,
                ellipsoid: ellipsoid(&cs.datum)?,
                meridian: meridian(&cs.prime_meridian, Some(angular)),
                axes: axes(&cs.axes)?,
            })
        }
        CoordinateSystem::Projected(cs) => {
            let geographic = &cs.geographic;
            let angular = geographic.angular_unit.factor;
            let linear = cs.linear_unit.factor;
            let ellipsoid = ellipsoid(&geographic.datum)?;
            let projector = Projector::new(&ellipsoid, &cs.projection.name, &cs.parameters, angular, linear)?;
            let kind = Kind::Projected { linear, projector: Box::new(projector) };
            Ok(Frame { kind, ellipsoid, meridian: meridian(&geographic.prime_meridian, Some(angular)), axes: axes(&cs.axes)? })
        }
        CoordinateSystem::Geocentric(cs) => {
            let standard = [Direction::Other, Direction::East, Direction::North];
            if !(cs.axes.is_empty() || cs.axes.iter().map(|a| a.direction).eq(standard)) {
                return Err(failed("a geocentric system's axes are not X OTHER, Y EAST, Z NORTH"));
            }
            let kind = Kind::Geocentric { linear: cs.linear_unit.factor };
            let axes = Axes { swap: false, signs: [1.0_f64; 3] };
            Ok(Frame { kind, ellipsoid: ellipsoid(&cs.datum)?, meridian: meridian(&cs.prime_meridian, None), axes })
        }
    }
}

/// Whether `datum` is WGS 84 by name and ellipsoid.
fn is_wgs84(datum: &Datum) -> bool {
    let name = normalize(&datum.name);
    let close = |a: f64, b: f64| (a - b).abs() <= SAME_ELLIPSOID * b;
    WGS84_NAMES.contains(&name.as_str())
        && close(datum.spheroid.semi_major, WGS84_SEMI_MAJOR)
        && close(datum.spheroid.inverse_flattening, WGS84_INVERSE_FLATTENING)
}

/// The datum's shift to WGS 84: some shift, or none for WGS 84 itself.
fn to_wgs84(datum: &Datum) -> Outcome<Option<Shift>> {
    if let Some(helmert) = &datum.to_wgs84 {
        return Ok(Some(Shift::new(helmert)));
    }
    if is_wgs84(datum) {
        return Ok(None);
    }
    Err(failed(&format!("the datum {} has no TOWGS84 and is not WGS 84", datum.name)))
}

/// Whether two datums are the same: one name, one spheroid, one shift.
fn same_datum(a: &Datum, b: &Datum) -> bool {
    normalize(&a.name) == normalize(&b.name)
        && a.spheroid.semi_major.to_bits() == b.spheroid.semi_major.to_bits()
        && a.spheroid.inverse_flattening.to_bits() == b.spheroid.inverse_flattening.to_bits()
        && a.to_wgs84 == b.to_wgs84
}

impl Transform {
    /// The transform from `from` to `to`.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if either system's projection,
    /// parameters or axes are unsupported, or the datums differ and one
    /// lacks a shift to WGS 84.
    pub fn new(from: &CoordinateSystem, to: &CoordinateSystem) -> Outcome<Transform> {
        let shift = if same_datum(from.datum(), to.datum()) { None } else { Some((to_wgs84(from.datum())?, to_wgs84(to.datum())?)) };
        Ok(Transform { from: frame(from)?, to: frame(to)?, shift })
    }

    /// `point` with its z coordinate, transformed.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if the point is outside the
    /// domain of a projection, so a result is not finite.
    pub fn apply(&self, point: [f64; 3]) -> Outcome<[f64; 3]> {
        let middle = self.shifted(self.from.unload(point));
        let out = self.to.load(middle);
        if !out.iter().all(|v| v.is_finite()) {
            return Err(failed("a point is outside the domain of the transform"));
        }
        Ok(out)
    }

    /// `point` without z, transformed at height zero.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry as [`Transform::apply`], or if
    /// either system is geocentric, which needs all three coordinates.
    pub fn apply_2d(&self, point: [f64; 2]) -> Outcome<[f64; 2]> {
        if self.from.is_geocentric() || self.to.is_geocentric() {
            return Err(failed("a geocentric system needs z coordinates"));
        }
        let [x, y] = point;
        let [tx, ty, _] = self.apply([x, y, 0.0_f64])?;
        Ok([tx, ty])
    }

    /// Every point of `coordinates`, laid out `stride` values per point
    /// with x and y first and z third when `has_z`; any further values,
    /// m coordinates, are left as they are.
    ///
    /// # Errors
    ///
    /// As [`Transform::apply`] and [`Transform::apply_2d`]; on failure the
    /// coordinates are unchanged. F02 invalid argument if `stride` is too
    /// small for the coordinates.
    pub fn apply_all(&self, coordinates: &mut [f64], stride: usize, has_z: bool) -> Outcome<()> {
        let mut out = coordinates.to_vec();
        for point in out.chunks_mut(stride.max(1)) {
            let mut values = point.iter_mut();
            let (Some(x), Some(y)) = (values.next(), values.next()) else {
                return Err(Fault::argument("[DESIGN] a point has fewer values than its coordinates"));
            };
            if !has_z {
                [*x, *y] = self.apply_2d([*x, *y])?;
                continue;
            }
            let Some(z) = values.next() else {
                return Err(Fault::argument("[DESIGN] a point has no z value"));
            };
            [*x, *y, *z] = self.apply([*x, *y, *z])?;
        }
        coordinates.copy_from_slice(&out);
        Ok(())
    }

    fn shifted(&self, point: Point) -> Point {
        let Some((from, to)) = &self.shift else {
            return point;
        };
        let xyz = self.from.geocentric(point);
        let wgs = from.as_ref().map_or(xyz, |shift| shift.forward(xyz));
        Point::Geocentric(to.as_ref().map_or(wgs, |shift| shift.inverse(wgs)))
    }
}

impl Frame {
    fn is_geocentric(&self) -> bool {
        matches!(self.kind, Kind::Geocentric { .. })
    }

    /// Stored coordinates in east, north, up order.
    fn canonical(&self, point: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = point;
        let [east, north] = if self.axes.swap { [y, x] } else { [x, y] };
        let [se, sn, su] = self.axes.signs;
        [east * se, north * sn, z * su]
    }

    /// East, north, up coordinates in stored order.
    fn stored(&self, point: [f64; 3]) -> [f64; 3] {
        let [se, sn, su] = self.axes.signs;
        let [east, north, up] = point;
        let [x, y] = if self.axes.swap { [north * sn, east * se] } else { [east * se, north * sn] };
        [x, y, up * su]
    }

    /// A stored point as geodetic or geocentric coordinates (steps 1, 2).
    fn unload(&self, point: [f64; 3]) -> Point {
        let [east, north, up] = self.canonical(point);
        match &self.kind {
            Kind::Geographic { angular, height } => Point::Geodetic([east * angular + self.meridian, north * angular, up * height]),
            Kind::Projected { linear, projector } => {
                let [lambda, phi] = projector.inverse(east * linear, north * linear);
                Point::Geodetic([lambda + self.meridian, phi, up * linear])
            }
            Kind::Geocentric { linear } => Point::Geocentric(rotate([east * linear, north * linear, up * linear], self.meridian)),
        }
    }

    /// Geocentric coordinates of `point` on this frame's ellipsoid.
    fn geocentric(&self, point: Point) -> [f64; 3] {
        match point {
            Point::Geodetic([lambda, phi, h]) => self.ellipsoid.geocentric(lambda, phi, h),
            Point::Geocentric(xyz) => xyz,
        }
    }

    /// `point` in this frame's stored coordinates (step 4).
    fn load(&self, point: Point) -> [f64; 3] {
        if let Kind::Geocentric { linear } = &self.kind {
            let [x, y, z] = rotate(self.geocentric(point), -self.meridian);
            return self.stored([x / linear, y / linear, z / linear]);
        }
        let [lambda, phi, h] = match point {
            Point::Geodetic(geodetic) => geodetic,
            Point::Geocentric(xyz) => self.ellipsoid.geodetic(xyz),
        };
        let lambda = longitude(lambda - self.meridian);
        match &self.kind {
            Kind::Geographic { angular, height } => self.stored([lambda / angular, phi / angular, h / height]),
            Kind::Projected { linear, projector } => {
                let [east, north] = projector.forward(lambda, phi);
                self.stored([east / linear, north / linear, h / linear])
            }
            Kind::Geocentric { .. } => self.stored([lambda, phi, h]),
        }
    }
}

/// A longitude brought into [−π, π] only if outside it, so ±180° stay.
fn longitude(lambda: f64) -> f64 {
    if lambda.abs() <= PI { lambda } else { -wrap(-lambda) }
}

/// `xyz` rotated by `angle` about the Z axis, from a frame whose X axis
/// is at longitude `angle` to one whose X axis is at Greenwich.
fn rotate(xyz: [f64; 3], angle: f64) -> [f64; 3] {
    let [x, y, z] = xyz;
    let (sine, cosine) = angle.sin_cos();
    [x * cosine - y * sine, x * sine + y * cosine, z]
}
