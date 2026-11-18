//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.1 to §15.13.1] The
//! attributes of each linear referencing type's CREATE TYPE: one struct
//! per type, one field per ST_Private attribute.
//!
//! An attribute every constructor and mutator keeps from the null value
//! (each raises "null argument" for it, or is null-call) is a plain value;
//! one some routine may set to the null value is an `Option`. An ARRAY
//! attribute is an `Option<Vec<_>>`, as a constructor may set it to the
//! null value, and its elements are never null. A geometry attribute is
//! a `Geom` of the geometry model. ST_LinearElement, not instantiable
//! [§15.2.1], is the enum of its three subtypes, each holding the
//! supertype's attributes in an `ElementBase`.

use super::LrKind;
use crate::geometry::model::Geom;

/// [§15.1.1] ST_LRM: a linear referencing method.
#[derive(Debug, Clone, PartialEq)]
pub struct Lrm {
    /// ST_PrivateLRMID: the lrmid [Description 2].
    pub id: i32,
    /// ST_PrivateLRMName: the name [Description 3].
    pub name: String,
    /// ST_PrivateLRMType: 'absolute', 'relative', 'interpolative',
    /// 'local interpolative' or another [Description 4, 13].
    pub lrm_type: String,
    /// ST_PrivateUnits: the unit of measure [Description 5].
    pub units: String,
    /// ST_PrivateConstraints: the constraints [Description 6].
    pub constraints: Option<Vec<String>>,
    /// ST_PrivateOffsetUnits: the offset unit, of an LRM allowing offsets
    /// [Description 7, 10].
    pub offset_units: Option<String>,
    /// ST_PrivatePositiveLateralOffsetDirection: 'right' or 'left'
    /// [Description 8, 14].
    pub lateral_direction: Option<String>,
    /// ST_PrivatePositiveVerticalOffsetDirection: 'up' or 'down'
    /// [Description 9, 15].
    pub vertical_direction: Option<String>,
}

/// [§15.2.1] The attributes of ST_LinearElement every subtype has.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementBase {
    /// ST_PrivateLinearElementID: the leid [Description 2].
    pub id: i32,
    /// ST_PrivateDefaultLRM: the default LRM's lrmid [Description 3, 7].
    pub default_lrm: i32,
    /// ST_PrivateDefaultMeasure: the default length [Description 4, 8].
    pub default_measure: LrMeasure,
    /// ST_PrivateLinearElementType: the linear element type
    /// [Description 5, 10].
    pub element_type: Option<String>,
    /// ST_PrivateStartValues: the start values [Description 6, 11].
    pub start_values: Option<Vec<StartValue>>,
}

/// [§15.2.1] ST_LinearElement: a value of one of its instantiable
/// subtypes.
#[derive(Debug, Clone, PartialEq)]
pub enum LinearElement {
    /// [§15.3] An ST_LRFeature.
    Feature(LrFeature),
    /// [§15.4] An ST_LRCurve.
    Curve(LrCurve),
    /// [§15.5] An ST_LRDirectedEdge.
    Edge(LrDirectedEdge),
}

/// [§15.3.1] ST_LRFeature: a feature that can be linearly measured.
#[derive(Debug, Clone, PartialEq)]
pub struct LrFeature {
    /// The ST_LinearElement attributes.
    pub base: ElementBase,
    /// ST_PrivateFeatureID: the feature ID [Description 2].
    pub feature_id: String,
    /// ST_PrivateReferents: the referents [Description 3].
    pub referents: Option<Vec<Referent>>,
}

/// [§15.4.1] ST_LRCurve: a curve that can be linearly measured.
#[derive(Debug, Clone, PartialEq)]
pub struct LrCurve {
    /// The ST_LinearElement attributes.
    pub base: ElementBase,
    /// ST_PrivateCurve: the curve, a value of ST_Curve [Description 2].
    pub curve: Geom,
}

/// [§15.5.1] ST_LRDirectedEdge: an edge or a link that can be linearly
/// measured.
#[derive(Debug, Clone, PartialEq)]
pub struct LrDirectedEdge {
    /// The ST_LinearElement attributes.
    pub base: ElementBase,
    /// ST_PrivateTopologyType: 'E' of an edge, 'L' of a link, the
    /// CHARACTER(1) [Description 2, 5, 6].
    pub topology_type: char,
    /// ST_PrivateTopologyOrNetworkName: the topology or network name
    /// [Description 3].
    pub topology_name: String,
    /// ST_PrivateEdgeOrLinkID: the edge ID or link ID [Description 4].
    pub edge_id: i32,
}

/// [§15.6.1] ST_PositionExp: a linearly referenced location.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionExp {
    /// ST_PrivateLinearElementID: the leid [Description 2, 8].
    pub element_id: i32,
    /// ST_PrivateLinearElement: the linear element [Description 3, 9].
    pub element: Option<Box<LinearElement>>,
    /// ST_PrivateLRMID: the lrmid [Description 4, 10].
    pub lrm_id: i32,
    /// ST_PrivateLRM: the LRM [Description 5, 11].
    pub lrm: Option<Lrm>,
    /// ST_PrivateDistanceExpression: the distance expression
    /// [Description 6].
    pub distance: DistanceExp,
}

/// [§15.7.1] ST_LRMeasure: a measure with an optional unit.
#[derive(Debug, Clone, PartialEq)]
pub struct LrMeasure {
    /// ST_PrivateMeasure: the measured value [Description 2].
    pub measure: f64,
    /// ST_PrivateUnits: the unit of measure [Description 3, 4].
    pub units: Option<String>,
}

/// [§15.8.1] ST_StartValue: an LRM's measure at the start of a linear
/// element.
#[derive(Debug, Clone, PartialEq)]
pub struct StartValue {
    /// ST_PrivateLRM: the lrmid [Description 2].
    pub lrm: i32,
    /// ST_PrivateMeasure: the measure at the start [Description 3, 4].
    pub measure: LrMeasure,
}

/// [§15.9.1] ST_DistanceExp: a measure along, and perhaps offset from, a
/// linear element.
#[derive(Debug, Clone, PartialEq)]
pub struct DistanceExp {
    /// ST_PrivateDistanceAlong: the distance along [Description 2, 14].
    pub along: LrMeasure,
    /// ST_PrivateFromReferentFeatureID [Description 3, 23].
    pub from_feature: Option<String>,
    /// ST_PrivateFromReferentName [Description 4].
    pub from_name: Option<String>,
    /// ST_PrivateTowardsReferentFeatureID [Description 5, 24].
    pub towards_feature: Option<String>,
    /// ST_PrivateTowardsReferentName [Description 6, 22].
    pub towards_name: Option<String>,
    /// ST_PrivateLateralOffsetExpression [Description 7].
    pub lateral: Option<LatOffsetExp>,
    /// ST_PrivateVerticalOffsetExpression [Description 8].
    pub vertical: Option<VerOffsetExp>,
    /// ST_PrivateVectorOffsetExpression [Description 9].
    pub vector: Option<VectorOffsetExp>,
}

/// [§15.10.1] ST_Referent: a known location along its feature.
#[derive(Debug, Clone, PartialEq)]
pub struct Referent {
    /// ST_PrivateReferentName [Description 2, 7].
    pub name: String,
    /// ST_PrivateReferentType: 'reference marker', 'intersection',
    /// 'boundary', 'landmark' or another [Description 3, 8].
    pub referent_type: String,
    /// ST_PrivatePosition: an ST_Point [Description 4].
    pub position: Option<Geom>,
    /// ST_PrivateLocation [Description 5, 11].
    pub location: Option<Box<PositionExp>>,
}

/// [§15.11.1] ST_LatOffsetExp: a lateral offset.
#[derive(Debug, Clone, PartialEq)]
pub struct LatOffsetExp {
    /// ST_PrivateOffsetLateralDistance [Description 2, 5, 6].
    pub distance: Option<LrMeasure>,
    /// ST_PrivateFeatureGeometry: the lateral offset referent as a
    /// geometry [Description 3, 7].
    pub geometry: Option<Geom>,
    /// ST_PrivateOffsetReferentDescription: the lateral offset referent
    /// as a description [Description 4, 7].
    pub description: Option<String>,
}

/// [§15.12.1] ST_VerOffsetExp: a vertical offset.
#[derive(Debug, Clone, PartialEq)]
pub struct VerOffsetExp {
    /// ST_PrivateOffsetVerticalDistance [Description 2, 5, 6].
    pub distance: Option<LrMeasure>,
    /// ST_PrivateFeatureGeometry: the vertical offset referent as a
    /// geometry [Description 3, 7].
    pub geometry: Option<Geom>,
    /// ST_PrivateOffsetReferentDescription: the vertical offset referent
    /// as a description [Description 4, 7].
    pub description: Option<String>,
}

/// [§15.13.1] ST_VectorOffsetExp: up to three offset vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorOffsetExp {
    /// ST_PrivateVectors, an `ST_Vector ARRAY[3]` [Description 2 to 4].
    pub vectors: Vec<Vector>,
}

/// [§17.2.1] The value of an ST_Vector, the type of §17.2 this module
/// holds as its own, since ST_Vector belongs to another module: its
/// ST_PrivateX, ST_PrivateY and ST_PrivateZ. An empty vector has none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vector {
    /// ST_PrivateX.
    pub x: Option<f64>,
    /// ST_PrivateY.
    pub y: Option<f64>,
    /// ST_PrivateZ, of a 3D vector.
    pub z: Option<f64>,
}

impl LinearElement {
    /// Its most specific type.
    pub fn kind(&self) -> LrKind {
        match self {
            LinearElement::Feature(_) => LrKind::LrFeature,
            LinearElement::Curve(_) => LrKind::LrCurve,
            LinearElement::Edge(_) => LrKind::LrDirectedEdge,
        }
    }

    /// The ST_LinearElement attributes it has.
    pub fn base(&self) -> &ElementBase {
        match self {
            LinearElement::Feature(f) => &f.base,
            LinearElement::Curve(c) => &c.base,
            LinearElement::Edge(e) => &e.base,
        }
    }

    /// The ST_LinearElement attributes, to change.
    pub fn base_mut(&mut self) -> &mut ElementBase {
        match self {
            LinearElement::Feature(f) => &mut f.base,
            LinearElement::Curve(c) => &mut c.base,
            LinearElement::Edge(e) => &mut e.base,
        }
    }

    /// The feature it is, if it is one.
    pub fn as_feature(&self) -> Option<&LrFeature> {
        match self {
            LinearElement::Feature(f) => Some(f),
            LinearElement::Curve(_) | LinearElement::Edge(_) => None,
        }
    }

    /// The curve it is, if it is one.
    pub fn as_curve(&self) -> Option<&LrCurve> {
        match self {
            LinearElement::Curve(c) => Some(c),
            LinearElement::Feature(_) | LinearElement::Edge(_) => None,
        }
    }
}

impl Vector {
    /// A 2D vector.
    pub fn xy(x: f64, y: f64) -> Vector {
        Vector { x: Some(x), y: Some(y), z: None }
    }

    /// A 3D vector.
    pub fn xyz(x: f64, y: f64, z: f64) -> Vector {
        Vector { x: Some(x), y: Some(y), z: Some(z) }
    }

    /// [§17.2.9] Whether it is empty: it has no x or no y.
    pub fn is_empty(&self) -> bool {
        self.x.is_none() || self.y.is_none()
    }
}
