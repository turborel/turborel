//! The SQL/MM Spatial exception conditions the curve, surface and format
//! engines raise, ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §22 Table 16,
//! each with its class 2F subclass. Every error is SQLCODE -301.

use crate::error::{Code, SqlError, SqlState};

/// A condition of §22 Table 16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// F02 invalid argument.
    InvalidArgument,
    /// F03 null argument.
    NullArgument,
    /// F07 null exterior ring.
    NullExteriorRing,
    /// F06 element is an empty set.
    ElementEmpty,
    /// F11 non-contiguous curves.
    NonContiguousCurves,
    /// F14 unsupported unit specified.
    UnsupportedUnit,
    /// F22 invalid well-known text representation.
    InvalidText,
    /// F23 invalid well-known binary representation.
    InvalidBinary,
    /// F24 invalid GML representation.
    InvalidGml,
    /// F25 mixed coordinate dimensions.
    MixedDimensions,
    /// F59 unknown spatial reference system.
    UnknownSrs,
    /// F67 polygon value is not a triangle value.
    NotATriangle,
    /// F66 triangles cannot have holes.
    TrianglesHaveNoHoles,
    /// F71 at least 3 points are required.
    ThreePointsRequired,
    /// F72 both geometries must be 3D.
    BothNot3D,
    /// F74 invalid geometry.
    InvalidGeometry,
    /// F75 exterior ring must have exactly 4 points.
    FourPointRing,
    /// F76 curve has multiple segments.
    MultipleSegments,
    /// F77 exactly three points are required.
    ExactlyThreePoints,
    /// F78 points are collinear.
    Collinear,
    /// F79 the given distance is longer than curve.
    DistanceTooLong,
    /// F80 the point is not on the curve.
    PointNotOnCurve,
    /// F84 missing measure value(s).
    MissingMeasures,
    /// F85 non-contiguous surfaces.
    NonContiguousSurfaces,
    /// F86 incorrect number of vectors.
    VectorCount,
}

impl Fault {
    /// The SQLSTATE: class 2F, subclass F01 to F98.
    pub fn sqlstate(self) -> &'static str {
        match self {
            Fault::InvalidArgument => "2FF02",
            Fault::NullArgument => "2FF03",
            Fault::NullExteriorRing => "2FF07",
            Fault::ElementEmpty => "2FF06",
            Fault::NonContiguousCurves => "2FF11",
            Fault::UnsupportedUnit => "2FF14",
            Fault::InvalidText => "2FF22",
            Fault::InvalidBinary => "2FF23",
            Fault::InvalidGml => "2FF24",
            Fault::MixedDimensions => "2FF25",
            Fault::UnknownSrs => "2FF59",
            Fault::NotATriangle => "2FF67",
            Fault::TrianglesHaveNoHoles => "2FF66",
            Fault::ThreePointsRequired => "2FF71",
            Fault::BothNot3D => "2FF72",
            Fault::InvalidGeometry => "2FF74",
            Fault::FourPointRing => "2FF75",
            Fault::MultipleSegments => "2FF76",
            Fault::ExactlyThreePoints => "2FF77",
            Fault::Collinear => "2FF78",
            Fault::DistanceTooLong => "2FF79",
            Fault::PointNotOnCurve => "2FF80",
            Fault::MissingMeasures => "2FF84",
            Fault::NonContiguousSurfaces => "2FF85",
            Fault::VectorCount => "2FF86",
        }
    }

    /// The condition's words in Table 16.
    pub fn words(self) -> &'static str {
        match self {
            Fault::InvalidArgument => "invalid argument",
            Fault::NullArgument => "null argument",
            Fault::NullExteriorRing => "null exterior ring",
            Fault::ElementEmpty => "element is an empty set",
            Fault::NonContiguousCurves => "non-contiguous curves",
            Fault::UnsupportedUnit => "unsupported unit specified",
            Fault::InvalidText => "invalid well-known text representation",
            Fault::InvalidBinary => "invalid well-known binary representation",
            Fault::InvalidGml => "invalid GML representation",
            Fault::MixedDimensions => "mixed coordinate dimensions",
            Fault::UnknownSrs => "unknown spatial reference system",
            Fault::NotATriangle => "polygon value is not a triangle value",
            Fault::TrianglesHaveNoHoles => "triangles cannot have holes",
            Fault::ThreePointsRequired => "at least 3 points are required",
            Fault::BothNot3D => "both geometries must be 3D",
            Fault::InvalidGeometry => "invalid geometry",
            Fault::FourPointRing => "exterior ring must have exactly 4 points",
            Fault::MultipleSegments => "curve has multiple segments",
            Fault::ExactlyThreePoints => "exactly three points are required",
            Fault::Collinear => "points are collinear",
            Fault::DistanceTooLong => "the given distance is longer than curve",
            Fault::PointNotOnCurve => "the point is not on the curve",
            Fault::MissingMeasures => "missing measure value(s)",
            Fault::NonContiguousSurfaces => "non-contiguous surfaces",
            Fault::VectorCount => "incorrect number of vectors",
        }
    }

    /// The error, its message the rule `rule` cites and then `detail`.
    pub fn at(self, rule: &str, detail: &str) -> SqlError {
        let error = SqlError::new(Code::Data, format!("[{rule}] SQL/MM Spatial exception - {}: {detail}", self.words()));
        match SqlState::parse(self.sqlstate()) {
            Some(state) => error.with_state(state),
            None => error,
        }
    }
}

/// The standard every citation in these engines names, as printed.
pub const STANDARD: &str = "ISO/IEC 13249-3:2016 (DIS, SC32 N2593)";

/// A citation of `clause` of [`STANDARD`].
pub fn rule(clause: &str) -> String {
    format!("{STANDARD} {clause}")
}

/// [`Fault::at`] for `clause` of [`STANDARD`].
pub fn fault(kind: Fault, clause: &str, detail: &str) -> SqlError {
    kind.at(&rule(clause), detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §22 Table 16] Each
    /// condition carries its class 2F subclass and words, and the error
    /// cites the clause it breaks.
    #[test]
    fn faults_carry_their_table_16_states() {
        let error = fault(Fault::MixedDimensions, "§5.1.67", "a point lacks z");
        assert_eq!(error.code, Code::Data);
        assert_eq!(error.sqlstate().as_str(), "2FF25");
        assert!(error.msg.contains("mixed coordinate dimensions"));
        assert!(error.msg.contains("§5.1.67"));
        for kind in [Fault::InvalidArgument, Fault::VectorCount, Fault::DistanceTooLong, Fault::Collinear] {
            assert!(kind.sqlstate().starts_with("2FF"));
            assert_ne!(kind.words(), "");
        }
    }
}
