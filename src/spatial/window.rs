//! \[DESIGN\] What an R-tree probe of a spatial call reads beside the
//! geometry it compares (RULES.md D85, D135): ST_Relate's pattern, or a
//! distance comparison's limit and unit scale. They follow the geometry
//! in the probe's value as a ROW, as a json index's PASSING values do.
//! The planner writes them and the index reads them. A value the index
//! cannot read makes it find every entry, so the complete condition, which
//! the executor always evaluates, raises what a scan would.

use std::sync::Arc;

use crate::spatial::planar::Pattern;
use crate::value::Value;

/// Relative room added to a converted distance limit. Unit conversion
/// rounds twice and decimal limits round once; each is far below this,
/// so a geometry whose computed distance meets the limit is never pruned.
const RELATIVE_SLACK: f64 = 1.0e-9;
/// The least radius searched. A smaller one would bring subnormal
/// arithmetic, whose relative rounding is unbounded, into the slack above.
const RADIUS_FLOOR: f64 = 1.0e-280;

/// ST_Relate's probe: its geometry and its pattern.
pub(crate) fn related(geometry: Value, pattern: Value) -> Value {
    Value::Row(Arc::from(vec![geometry, pattern]))
}

/// A distance comparison's probe: its geometry, the limit it is compared
/// with, and the named unit's length in the column's units, one without
/// a unit.
pub(crate) fn within(geometry: Value, limit: Value, scale: f64) -> Value {
    Value::Row(Arc::from(vec![geometry, limit, Value::Approx(scale)]))
}

/// The fields of a probe's ROW value: its geometry and what follows.
pub(crate) fn fields(value: &Value) -> Option<&[Value]> {
    let Value::Row(fields) = value else { return None };
    Some(fields)
}

/// Whether a pattern value implies that its geometries meet, so that an
/// envelope window holds every row it is true of (`Pattern::meets`).
pub(crate) fn pattern_meets(pattern: &Value) -> bool {
    let Value::Char(text) = pattern else { return false };
    Pattern::parse(text).is_ok_and(|p| p.meets())
}

/// How far around a geometry a distance comparison's rows can be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Radius {
    /// No distance compares true: the limit is null or negative.
    None,
    /// The limit is not a finite number the index can bound.
    All,
    /// Every row compared true lies within this many coordinate units.
    Of(f64),
}

/// The radius of `limit` in the named unit `scale` coordinate units
/// long. The computed distance is the coordinate distance times the
/// scale (`geometry::routines::units`), monotone in it, so a geometry
/// beyond the radius has a converted distance beyond the limit.
pub(crate) fn radius(limit: &Value, scale: &Value) -> Radius {
    if limit.is_null() {
        return Radius::None;
    }
    let (Some(limit), Value::Approx(scale)) = (limit.as_f64(), scale) else { return Radius::All };
    if limit.is_nan() || !scale.is_finite() || *scale <= 0.0_f64 {
        return Radius::All;
    }
    if limit < 0.0_f64 {
        return Radius::None;
    }
    let units = limit.max(RADIUS_FLOOR) / scale * (1.0_f64 + RELATIVE_SLACK);
    if units.is_finite() { Radius::Of(units.max(RADIUS_FLOOR).next_up()) } else { Radius::All }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    /// Verifies: [DESIGN] D135: a null or negative limit compares true of
    /// no distance; NaN, infinity, a non-numeric limit or a scale that is
    /// not a positive finite number finds every row; a radius is never less
    /// than the limit in coordinate units, nor than the floor that keeps
    /// subnormal rounding out of the slack.
    #[test]
    fn radii_never_fall_short_of_their_limits() {
        let one = Value::Approx(1.0_f64);
        assert_eq!(radius(&Value::Null, &one), Radius::None);
        assert_eq!(radius(&Value::Approx(-0.5_f64), &one), Radius::None);
        for limit in [Value::Approx(f64::NAN), Value::Approx(f64::INFINITY), Value::Char("x".into())] {
            assert_eq!(radius(&limit, &one), Radius::All);
        }
        for scale in [Value::Approx(0.0_f64), Value::Approx(-1.0_f64), Value::Approx(f64::INFINITY), Value::Null] {
            assert_eq!(radius(&Value::Approx(1.0_f64), &scale), Radius::All);
        }
        assert_eq!(radius(&Value::Approx(f64::MAX), &Value::Approx(0.5_f64)), Radius::All);
        let Radius::Of(r) = radius(&Value::Exact(Decimal::new(3, 1)), &one) else { panic!("no radius") };
        assert!((0.3_f64..0.300_001_f64).contains(&r));
        let Radius::Of(feet) = radius(&Value::Approx(10.0_f64), &Value::Approx(0.3048_f64)) else { panic!("no radius") };
        assert!(feet * 0.3048_f64 > 10.0_f64);
        let Radius::Of(zero) = radius(&Value::Approx(0.0_f64), &one) else { panic!("no radius") };
        assert!(zero >= RADIUS_FLOOR);
    }

    /// Verifies: [DESIGN] D135: a pattern implies meeting only through
    /// an interior or boundary cell asking for a nonempty intersection;
    /// exterior cells, F and *, a malformed pattern or a non-string do not.
    /// MC/DC: each of the four meeting cells alone makes it true.
    #[test]
    fn patterns_meet_only_through_interiors_and_boundaries() {
        for meeting in ["T********", "*T*******", "***T*****", "****0****", "1********", "***2*****"] {
            assert!(pattern_meets(&Value::Char(meeting.into())), "{meeting}");
        }
        for apart in ["FF*FF****", "**T******", "*****T***", "******T**", "********T", "*********", "T*******", "x********"] {
            assert!(!pattern_meets(&Value::Char(apart.into())), "{apart}");
        }
        assert!(!pattern_meets(&Value::Null));
        let row = related(Value::Null, Value::Char("T********".into()));
        assert_eq!(fields(&row).map(<[Value]>::len), Some(2));
        assert!(fields(&Value::Null).is_none());
        assert_eq!(fields(&within(Value::Null, Value::Null, 1.0_f64)).map(<[Value]>::len), Some(3));
    }
}
