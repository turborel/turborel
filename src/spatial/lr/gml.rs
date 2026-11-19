//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12, §15.2.10, §15.3.6,
//! §15.4.7, §15.5.7, §15.6.10, §15.9.12] The GML representation of the
//! linear referencing types: reading it (the ST_...FromGML functions and
//! the CHARACTER LARGE OBJECT constructors) and writing it (what
//! ST_AsGML of §15.1.1 Description 1 and its peers return).
//!
//! The text names these functions and the exception "invalid GML
//! representation" but gives no schema, so the representation is
//! implementation-defined. Each element is named from the wording of the
//! attribute's Description, and holds its value as text, so a document
//! reads as the value's attributes do:
//!
//! - A value is an element named for its type, `LRM`, `LRFeature`,
//!   `LRCurve`, `LRDirectedEdge`, `PositionExpression`,
//!   `DistanceExpression`, `LRMeasure`, `StartValue`, `Referent`,
//!   `LateralOffsetExpression`, `VerticalOffsetExpression` and
//!   `VectorOffsetExpression`; a value inside another is that element
//!   inside the element of its attribute.
//! - An attribute is a child element: `lrmID`, `name`, `type`,
//!   `unitOfMeasure`, `constraints` of `constraint`s, `offsetUnitOfMeasure`,
//!   `positiveLateralOffsetDirection`, `positiveVerticalOffsetDirection`;
//!   `linearElementID`, `defaultLRM`, `defaultMeasure`,
//!   `linearElementType`, `startValues` of `StartValue`s, `featureID`,
//!   `referents` of `Referent`s, `curve`, `topologyType`,
//!   `topologyOrNetworkName`, `edgeOrLinkID`; `linearElement`, `lrm`;
//!   `distanceAlong`, `fromReferentFeatureID`, `fromReferentName`,
//!   `towardsReferentFeatureID`, `towardsReferentName`; `offsetDistance`,
//!   `featureGeometry`, `offsetReferentDescription`; `position`,
//!   `location`; `measure`; `Vector` of `x`, `y` and `z`.
//! - An attribute that is the null value has no element. An empty ARRAY
//!   is its element with no children. A null element of the constraints
//!   is a `constraint` with the attribute `nil="true"`.
//! - A measure is its number as text, its unit an attribute `uom`.
//! - A geometry is the GML of §5.1.61 (`spatial::formats::gml`), which
//!   carries no m coordinate, inside its attribute's element.
//! - Element names are matched without their namespace prefix, as the
//!   crate's XML reader drops it; the writer writes none.

mod read;
mod write;

use super::{Catalog, LrKind, LrValue, MAX_TEXT, failed};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::formats::gml::Srs;

// ---------------------------------------------------------------------------
// Element names, implementation-defined
// ---------------------------------------------------------------------------

/// [§15.1] An ST_LRM.
pub(crate) const LRM: &str = "LRM";
/// [§15.3] An ST_LRFeature.
pub(crate) const LR_FEATURE: &str = "LRFeature";
/// [§15.4] An ST_LRCurve.
pub(crate) const LR_CURVE: &str = "LRCurve";
/// [§15.5] An ST_LRDirectedEdge.
pub(crate) const LR_DIRECTED_EDGE: &str = "LRDirectedEdge";
/// [§15.6] An ST_PositionExp.
pub(crate) const POSITION_EXPRESSION: &str = "PositionExpression";
/// [§15.7] An ST_LRMeasure.
pub(crate) const LR_MEASURE: &str = "LRMeasure";
/// [§15.8] An ST_StartValue.
pub(crate) const START_VALUE: &str = "StartValue";
/// [§15.9] An ST_DistanceExp.
pub(crate) const DISTANCE_EXPRESSION: &str = "DistanceExpression";
/// [§15.10] An ST_Referent.
pub(crate) const REFERENT: &str = "Referent";
/// [§15.11] An ST_LatOffsetExp.
pub(crate) const LATERAL_OFFSET: &str = "LateralOffsetExpression";
/// [§15.12] An ST_VerOffsetExp.
pub(crate) const VERTICAL_OFFSET: &str = "VerticalOffsetExpression";
/// [§15.13] An ST_VectorOffsetExp.
pub(crate) const VECTOR_OFFSET: &str = "VectorOffsetExpression";
/// [§17.2] An ST_Vector.
pub(crate) const VECTOR: &str = "Vector";
/// [§15.1.1 Description 2, §15.6.1, §15.8.1] An lrmid.
pub(crate) const LRM_ID: &str = "lrmID";
/// [§15.1.1 Description 3, §15.10.1] A name.
pub(crate) const NAME: &str = "name";
/// [§15.1.1 Description 4, §15.10.1] A type.
pub(crate) const TYPE: &str = "type";
/// [§15.1.1 Description 5] A unit of measure.
pub(crate) const UNIT_OF_MEASURE: &str = "unitOfMeasure";
/// [§15.1.1 Description 6] The constraints.
pub(crate) const CONSTRAINTS: &str = "constraints";
/// One constraint.
pub(crate) const CONSTRAINT: &str = "constraint";
/// [§15.1.1 Description 6] The attribute marking a null constraint.
pub(crate) const NIL: &str = "nil";
/// [§15.1.1 Description 7] The offset unit of measure.
pub(crate) const OFFSET_UNIT: &str = "offsetUnitOfMeasure";
/// [§15.1.1 Description 8] The positive lateral offset direction.
pub(crate) const LATERAL_DIRECTION: &str = "positiveLateralOffsetDirection";
/// [§15.1.1 Description 9] The positive vertical offset direction.
pub(crate) const VERTICAL_DIRECTION: &str = "positiveVerticalOffsetDirection";
/// [§15.2.1, §15.6.1] A linear element id.
pub(crate) const ELEMENT_ID: &str = "linearElementID";
/// [§15.2.1] The default LRM.
pub(crate) const DEFAULT_LRM: &str = "defaultLRM";
/// [§15.2.1] The default measure.
pub(crate) const DEFAULT_MEASURE: &str = "defaultMeasure";
/// [§15.2.1] The linear element type.
pub(crate) const ELEMENT_TYPE: &str = "linearElementType";
/// [§15.2.1] The start values.
pub(crate) const START_VALUES: &str = "startValues";
/// [§15.3.1] The feature id.
pub(crate) const FEATURE_ID: &str = "featureID";
/// [§15.3.1] The referents.
pub(crate) const REFERENTS: &str = "referents";
/// [§15.4.1] The curve.
pub(crate) const CURVE: &str = "curve";
/// [§15.5.1] The topology type.
pub(crate) const TOPOLOGY_TYPE: &str = "topologyType";
/// [§15.5.1] The topology or network name.
pub(crate) const TOPOLOGY_NAME: &str = "topologyOrNetworkName";
/// [§15.5.1] The edge or link id.
pub(crate) const EDGE_ID: &str = "edgeOrLinkID";
/// [§15.6.1] The linear element.
pub(crate) const LINEAR_ELEMENT: &str = "linearElement";
/// [§15.6.1] The LRM.
pub(crate) const LRM_VALUE: &str = "lrm";
/// [§15.9.1 Description 2] The distance along.
pub(crate) const DISTANCE_ALONG: &str = "distanceAlong";
/// [§15.9.1 Description 3] The from referent's feature id.
pub(crate) const FROM_FEATURE: &str = "fromReferentFeatureID";
/// [§15.9.1 Description 4] The from referent's name.
pub(crate) const FROM_NAME: &str = "fromReferentName";
/// [§15.9.1 Description 5] The towards referent's feature id.
pub(crate) const TOWARDS_FEATURE: &str = "towardsReferentFeatureID";
/// [§15.9.1 Description 6] The towards referent's name.
pub(crate) const TOWARDS_NAME: &str = "towardsReferentName";
/// [§15.11.1, §15.12.1] The offset distance.
pub(crate) const OFFSET_DISTANCE: &str = "offsetDistance";
/// [§15.11.1, §15.12.1] The feature geometry.
pub(crate) const FEATURE_GEOMETRY: &str = "featureGeometry";
/// [§15.11.1, §15.12.1] The offset referent description.
pub(crate) const OFFSET_DESCRIPTION: &str = "offsetReferentDescription";
/// [§15.10.1] The position.
pub(crate) const POSITION: &str = "position";
/// [§15.10.1] The location.
pub(crate) const LOCATION: &str = "location";
/// [§15.8.1] The measure.
pub(crate) const MEASURE: &str = "measure";
/// [§17.2.1] x.
pub(crate) const X: &str = "x";
/// [§17.2.1] y.
pub(crate) const Y: &str = "y";
/// [§17.2.1] z.
pub(crate) const Z: &str = "z";
/// [§15.7.1] The attribute holding a measure's unit.
pub(crate) const UOM: &str = "uom";

/// [§15.1.12, §15.2.10, §15.3.6, §15.4.7, §15.5.7, §15.6.10, §15.9.12]
/// The value of type `expected`, or of a subtype of it, whose GML is
/// `text`. Units are checked against `catalog.units`, a referent's
/// position against its location in `catalog`, and a geometry's srsName
/// is resolved by `srs`.
///
/// # Errors
///
/// SQLSTATE 2FF24 (invalid GML representation) of a document that is not
/// this representation or is of another type; the exceptions of the
/// constructors it calls; SQLCODE -900 past `MAX_TEXT` or `MAX_DEPTH`.
pub fn from_gml(text: &str, expected: LrKind, catalog: &Catalog<'_>, srs: &dyn Srs) -> Result<LrValue> {
    if text.len() > MAX_TEXT {
        return Err(too_long());
    }
    let v = read::value(text, catalog, srs)?;
    if v.kind().is_subtype_of(expected) {
        return Ok(v);
    }
    Err(invalid(&format!("the document is of an {}, not an {}", v.kind().sql_name(), expected.sql_name())))
}

/// The GML of `v`, a geometry's srsName named by `srs`.
///
/// # Errors
///
/// SQLCODE -900 of a value nested past `MAX_DEPTH` or of GML past
/// `MAX_TEXT`; as `spatial::formats::gml::write_geom`.
pub fn as_gml(v: &LrValue, srs: &dyn Srs) -> Result<String> {
    let text = write::value(v, srs)?;
    if text.len() > MAX_TEXT {
        return Err(too_long());
    }
    Ok(text)
}

/// SQLSTATE 2FF24, saying why.
pub(crate) fn invalid(detail: &str) -> SqlError {
    failed(SqlState::SPATIAL_INVALID_GML, "§15.1.12", &format!("invalid GML representation: {detail}"))
}

/// SQLCODE -900: GML past ST_MaxLRAsGML.
fn too_long() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a linear referencing GML document is longer than ST_MaxLRAsGML (RULES.md D85)")
}
