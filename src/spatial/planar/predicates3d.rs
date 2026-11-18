//! \[DESIGN\] Exact spatial predicates on finite binary64 coordinates,
//! in the three stages of `predicates.rs`. They are Shewchuk's filtered
//! floating evaluation, outward-rounded intervals, then exact dyadic
//! arithmetic.

use std::cmp::Ordering;

use super::dyadic::Dyadic;
use super::interval::{Arithmetic, Interval};
use super::predicates::{proven, tame};

/// A point with x, y and z coordinates.
pub type Point3 = [f64; 3];

/// Shewchuk (1997), `exactinit`, `o3derrboundA` = (7 + 56ε)ε.
const ORIENT3D_BOUND: f64 = 7.771_561_172_376_103e-16;
/// `isperrboundA` = (16 + 224ε)ε.
const INSPHERE_BOUND: f64 = 1.776_356_839_400_253_2e-15;

/// The side of the plane through `a`, `b` and `c` on which `d` lies:
/// greater when `d` lies below it, `a`, `b`, `c` appearing
/// counterclockwise seen from above; less above; equal when coplanar.
/// This is the sign of the determinant of `a - d`, `b - d`, `c - d`.
pub fn orient3d(a: Point3, b: Point3, c: Point3, d: Point3) -> Ordering {
    filtered_orient3d(a, b, c, d)
        .or_else(|| orient3d_determinant::<Interval>(a, b, c, d).sign())
        .unwrap_or_else(|| orient3d_determinant::<Dyadic>(a, b, c, d).sign())
}

fn differences(points: [Point3; 4], apex: Point3) -> [[f64; 3]; 4] {
    let [ex, ey, ez] = apex;
    points.map(|[x, y, z]| [x - ex, y - ey, z - ez])
}

fn filtered_orient3d(a: Point3, b: Point3, c: Point3, d: Point3) -> Option<Ordering> {
    let [[adx, ady, adz], [bdx, bdy, bdz], [cdx, cdy, cdz], _] = differences([a, b, c, d], d);
    if !tame(&[adx, ady, adz, bdx, bdy, bdz, cdx, cdy, cdz]) {
        return None;
    }
    let (bdycdz, bdzcdy, cdyadz, cdzady, adybdz, adzbdy) = (bdy * cdz, bdz * cdy, cdy * adz, cdz * ady, ady * bdz, adz * bdy);
    let determinant = adx * (bdycdz - bdzcdy) + bdx * (cdyadz - cdzady) + cdx * (adybdz - adzbdy);
    let permanent =
        (bdycdz.abs() + bdzcdy.abs()) * adx.abs() + (cdyadz.abs() + cdzady.abs()) * bdx.abs() + (adybdz.abs() + adzbdy.abs()) * cdx.abs();
    proven(determinant, ORIENT3D_BOUND * permanent)
}

fn orient3d_determinant<T: Arithmetic>(a: Point3, b: Point3, c: Point3, d: Point3) -> T {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    let [cx, cy, cz] = c;
    let [dx, dy, dz] = d;
    let (adx, ady, adz) = (T::difference(ax, dx), T::difference(ay, dy), T::difference(az, dz));
    let (bdx, bdy, bdz) = (T::difference(bx, dx), T::difference(by, dy), T::difference(bz, dz));
    let (cdx, cdy, cdz) = (T::difference(cx, dx), T::difference(cy, dy), T::difference(cz, dz));
    let minor = |y1: &T, z1: &T, y2: &T, z2: &T| y1.times(z2).minus(&z1.times(y2));
    let first = adx.times(&minor(&bdy, &bdz, &cdy, &cdz));
    let second = bdx.times(&minor(&cdy, &cdz, &ady, &adz));
    let third = cdx.times(&minor(&ady, &adz, &bdy, &bdz));
    first.plus(&second).plus(&third)
}

/// Whether `query` lies inside the sphere through `a`, `b`, `c` and `d`:
/// greater inside, less outside, equal on it, when `orient3d(a, b, c,
/// d)` is greater; the signs reverse when it is less.
pub fn insphere(a: Point3, b: Point3, c: Point3, d: Point3, query: Point3) -> Ordering {
    filtered_insphere([a, b, c, d], query)
        .or_else(|| insphere_determinant::<Interval>([a, b, c, d], query).sign())
        .unwrap_or_else(|| insphere_determinant::<Dyadic>([a, b, c, d], query).sign())
}

fn filtered_insphere(points: [Point3; 4], query: Point3) -> Option<Ordering> {
    let rows = differences(points, query);
    if !tame(rows.as_flattened()) {
        return None;
    }
    let [[aex, aey, aez], [bex, bey, bez], [cex, cey, cez], [dex, dey, dez]] = rows;
    let cross = |x1: f64, y1: f64, x2: f64, y2: f64| (x1 * y2 - x2 * y1, (x1 * y2).abs() + (x2 * y1).abs());
    let ab = cross(aex, aey, bex, bey);
    let bc = cross(bex, bey, cex, cey);
    let cd = cross(cex, cey, dex, dey);
    let da = cross(dex, dey, aex, aey);
    let ac = cross(aex, aey, cex, cey);
    let bd = cross(bex, bey, dex, dey);
    let abc = aez * bc.0 - bez * ac.0 + cez * ab.0;
    let bcd = bez * cd.0 - cez * bd.0 + dez * bc.0;
    let cda = cez * da.0 + dez * ac.0 + aez * cd.0;
    let dab = dez * ab.0 + aez * bd.0 + bez * da.0;
    let lift = |x: f64, y: f64, z: f64| x * x + y * y + z * z;
    let (alift, blift, clift, dlift) = (lift(aex, aey, aez), lift(bex, bey, bez), lift(cex, cey, cez), lift(dex, dey, dez));
    let determinant = (dlift * abc - clift * dab) + (blift * cda - alift * bcd);
    let [az, bz, cz, dz] = [aez.abs(), bez.abs(), cez.abs(), dez.abs()];
    let permanent = (cd.1 * bz + bd.1 * cz + bc.1 * dz) * alift
        + (da.1 * cz + ac.1 * dz + cd.1 * az) * blift
        + (ab.1 * dz + bd.1 * az + da.1 * bz) * clift
        + (bc.1 * az + ac.1 * bz + ab.1 * cz) * dlift;
    proven(determinant, INSPHERE_BOUND * permanent)
}

/// The determinant of the lifted rows `(p - e, |p - e|²)`, expanded by
/// its last column into four 3 × 3 minors.
fn insphere_determinant<T: Arithmetic>(points: [Point3; 4], apex: Point3) -> T {
    let [ex, ey, ez] = apex;
    let rows = points.map(|[x, y, z]| [T::difference(x, ex), T::difference(y, ey), T::difference(z, ez)]);
    let [a, b, c, d] = &rows;
    let lift = |[x, y, z]: &[T; 3]| x.times(x).plus(&y.times(y)).plus(&z.times(z));
    let minor = |p: &[T; 3], q: &[T; 3], r: &[T; 3]| {
        let [px, py, pz] = p;
        let [qx, qy, qz] = q;
        let [rx, ry, rz] = r;
        let first = px.times(&qy.times(rz).minus(&qz.times(ry)));
        let second = qx.times(&ry.times(pz).minus(&rz.times(py)));
        let third = rx.times(&py.times(qz).minus(&pz.times(qy)));
        first.plus(&second).plus(&third)
    };
    let positive = lift(d).times(&minor(a, b, c)).plus(&lift(b).times(&minor(c, d, a)));
    let negative = lift(c).times(&minor(d, a, b)).plus(&lift(a).times(&minor(b, c, d)));
    positive.minus(&negative)
}

#[cfg(test)]
mod tests;
