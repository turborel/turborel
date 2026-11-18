//! Circular arcs through three points, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §7.3.1 D8: the center is where the perpendicular bisectors of
//! the two chords meet, in the plane of the three points, so 3D arcs are
//! handled as 2D ones. Collinear points make a straight segment.

use super::numeric::{HALF, V3, add, cross, dot, norm, scale, sub};
use std::f64::consts::TAU;

/// Points whose chords' cross product is below this fraction of the
/// chords' lengths' product are collinear (§7.3.1 D8b), an
/// implementation-defined tolerance.
pub const COLLINEAR: f64 = 1e-12;

/// The circle through three points, oriented so they follow in turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// The center.
    pub center: V3,
    /// The radius.
    pub radius: f64,
    /// The unit vector from the center to the first point.
    pub e1: V3,
    /// The unit vector a quarter turn on from `e1`, the direction of travel.
    pub e2: V3,
}

/// The circle through a, b and c, none when they are collinear or two
/// coincide.
pub fn circle(a: V3, b: V3, c: V3) -> Option<Circle> {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let perpendicular = cross(ab, ac);
    let area = norm(perpendicular);
    let lengths = norm(ab) * norm(ac);
    if !area.is_finite() || area <= COLLINEAR * lengths {
        return None;
    }
    let numerator = add(scale(cross(ac, perpendicular), dot(ab, ab)), scale(cross(perpendicular, ab), dot(ac, ac)));
    let center = add(a, scale(numerator, HALF / (area * area)));
    let to_a = sub(a, center);
    let radius = norm(to_a);
    let e1 = scale(to_a, 1.0 / radius);
    let normal = scale(perpendicular, 1.0 / area);
    let e2 = cross(normal, e1);
    Some(Circle { center, radius, e1, e2 })
}

impl Circle {
    /// The angle of p about the center from `e1`, in [0, 2π).
    pub fn angle(&self, p: V3) -> f64 {
        let d = sub(p, self.center);
        let angle = dot(d, self.e2).atan2(dot(d, self.e1));
        if angle < 0.0 { angle + TAU } else { angle }
    }

    /// The point at `angle`.
    pub fn point(&self, angle: f64) -> V3 {
        let (sin, cos) = angle.sin_cos();
        add(self.center, add(scale(self.e1, self.radius * cos), scale(self.e2, self.radius * sin)))
    }

    /// The unit normal of the circle's plane, about which angles turn.
    pub fn normal(&self) -> V3 {
        cross(self.e1, self.e2)
    }
}

/// The arc from `a` through `b` to `c` as its circle and the angles of
/// `b` and `c` (that of `a` is zero), none when the points are collinear.
pub fn arc(a: V3, b: V3, c: V3) -> Option<(Circle, f64, f64)> {
    let circle = circle(a, b, c)?;
    Some((circle, circle.angle(b), circle.angle(c)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn close(a: V3, b: V3) -> bool {
        norm(sub(a, b)) < 1e-12
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D8a] The
    /// center lies where the chords' bisectors meet, and the arc's angles
    /// follow the points' order, counterclockwise or clockwise.
    #[test]
    fn arc_through_three_points() {
        let (circle, mid, end) = arc([1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [-1.0_f64, 0.0_f64, 0.0_f64]).unwrap();
        assert!(close(circle.center, [0.0_f64; 3]));
        assert!((circle.radius - 1.0).abs() < 1e-15_f64);
        assert!((mid - FRAC_PI_2).abs() < 1e-15_f64 && (end - PI).abs() < 1e-15_f64);
        assert!(close(circle.normal(), [0.0_f64, 0.0_f64, 1.0_f64]));
        let (clockwise, _, last) = arc([1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, -1.0_f64, 0.0_f64], [-1.0_f64, 0.0_f64, 0.0_f64]).unwrap();
        assert!(close(clockwise.normal(), [0.0_f64, 0.0_f64, -1.0_f64]));
        assert!((last - PI).abs() < 1e-15_f64);
        assert!(close(clockwise.point(FRAC_PI_2), [0.0_f64, -1.0_f64, 0.0_f64]));
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D8] Arcs
    /// in a tilted plane keep their radius; collinear or coincident
    /// points have no circle (D8b).
    #[test]
    fn arcs_in_space_and_degenerate_arcs() {
        let (circle, _, end) = arc([1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 1.0_f64], [-1.0_f64, 0.0_f64, 0.0_f64]).unwrap();
        assert!((circle.radius - 1.0).abs() < 1e-15_f64);
        assert!((end - PI).abs() < 1e-15_f64);
        assert!(close(circle.point(FRAC_PI_2), [0.0_f64, 0.0_f64, 1.0_f64]));
        assert!(arc([0.0_f64; 3], [1.0_f64, 1.0_f64, 0.0_f64], [2.0_f64, 2.0_f64, 0.0_f64]).is_none());
        assert!(arc([0.0_f64; 3], [0.0_f64; 3], [2.0_f64, 2.0_f64, 0.0_f64]).is_none());
        assert!(arc([0.0_f64; 3], [f64::NAN, 0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64, 0.0_f64]).is_none());
    }
}
