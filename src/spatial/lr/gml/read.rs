//! Reading the GML of a linear referencing value, as `lr/gml.rs` defines
//! it [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12 and its peers]:
//! the document is parsed by the crate's XML reader
//! (`spatial::formats::gml::xml`, which bounds its depth and skips a
//! document type declaration without expanding it), then each element
//! produces its value through the constructor the type's text
//! representation names, so the constructor's exceptions are the
//! document's.

use super::{
    CONSTRAINT, CONSTRAINTS, CURVE, DEFAULT_LRM, DEFAULT_MEASURE, DISTANCE_ALONG, DISTANCE_EXPRESSION, EDGE_ID, ELEMENT_ID, ELEMENT_TYPE,
    FEATURE_GEOMETRY, FEATURE_ID, FROM_FEATURE, FROM_NAME, LATERAL_DIRECTION, LATERAL_OFFSET, LINEAR_ELEMENT, LOCATION, LR_CURVE,
    LR_DIRECTED_EDGE, LR_FEATURE, LR_MEASURE, LRM, LRM_ID, LRM_VALUE, MEASURE, NAME, NIL, OFFSET_DESCRIPTION, OFFSET_DISTANCE, OFFSET_UNIT,
    POSITION, POSITION_EXPRESSION, REFERENT, REFERENTS, START_VALUE, START_VALUES, TOPOLOGY_NAME, TOPOLOGY_TYPE, TOWARDS_FEATURE,
    TOWARDS_NAME, TYPE, UNIT_OF_MEASURE, UOM, VECTOR, VECTOR_OFFSET, VERTICAL_DIRECTION, VERTICAL_OFFSET, X, Y, Z, invalid,
};
use crate::error::Result;
use crate::geometry::model::{Body, Geom};
use crate::spatial::formats::gml::xml::{Node, escape, parse};
use crate::spatial::formats::gml::{Srs, read_geom};
use crate::spatial::lr::model::{
    DistanceExp, LatOffsetExp, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrMeasure, Lrm, PositionExp, Referent, StartValue,
    Vector, VectorOffsetExp, VerOffsetExp,
};
use crate::spatial::lr::{Catalog, EdgeParts, ElementParts, LrValue, LrmParts, MAX_VECTORS, OffsetParts, ReferentNames, deeper};
use std::fmt::Write as _;

/// \[DESIGN\] How deep the markup of a geometry is rewritten for the GML
/// reader: as deep as the crate's XML reader reads (RULES.md R-CF-6).
const MARKUP_DEPTH: usize = 256;

/// The value of the document `text`.
///
/// # Errors
///
/// As `gml::from_gml`.
pub(crate) fn value(text: &str, cat: &Catalog<'_>, srs: &dyn Srs) -> Result<LrValue> {
    let root = parse(text)?;
    let r = Read { cat, srs };
    Ok(match root.name.as_str() {
        LRM => LrValue::Lrm(r.lrm(&root)?),
        LR_FEATURE | LR_CURVE | LR_DIRECTED_EDGE => LrValue::LinearElement(r.element(&root, 0)?),
        POSITION_EXPRESSION => LrValue::PositionExp(r.posexp(&root, 0)?),
        LR_MEASURE => LrValue::LrMeasure(r.measure(&root)?),
        START_VALUE => LrValue::StartValue(r.start_value(&root)?),
        DISTANCE_EXPRESSION => LrValue::DistanceExp(r.disexp(&root)?),
        REFERENT => LrValue::Referent(r.referent(&root, 0)?),
        LATERAL_OFFSET => {
            let (distance, geometry, description) = r.offset(&root)?;
            LrValue::LatOffsetExp(LatOffsetExp { distance, geometry, description })
        }
        VERTICAL_OFFSET => {
            let (distance, geometry, description) = r.offset(&root)?;
            LrValue::VerOffsetExp(VerOffsetExp { distance, geometry, description })
        }
        VECTOR_OFFSET => LrValue::VectorOffsetExp(vectors(&root)?),
        other => return Err(invalid(&format!("<{other}> is no linear referencing value"))),
    })
}

/// The child `name` of `node`.
fn child<'n>(node: &'n Node, name: &str) -> Result<&'n Node> {
    node.child(name).ok_or_else(|| invalid(&format!("<{}> has no <{name}>", node.name)))
}

/// The text of the child `name`, trimmed, if it has one.
fn opt_text(node: &Node, name: &str) -> Option<String> {
    node.child(name).map(|c| c.text.trim().to_owned())
}

/// The text of the child `name`, trimmed.
fn text(node: &Node, name: &str) -> Result<String> {
    Ok(child(node, name)?.text.trim().to_owned())
}

/// The INTEGER of the child `name`.
fn integer(node: &Node, name: &str) -> Result<i32> {
    let t = text(node, name)?;
    t.parse::<i32>().map_err(|_| invalid(&format!("<{name}> holds {t}, no integer")))
}

/// The DOUBLE PRECISION `node` holds.
fn number(node: &Node) -> Result<f64> {
    let t = node.text.trim();
    t.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| invalid(&format!("<{}> holds {t}, no number", node.name)))
}

/// [§15.13.1] An ST_VectorOffsetExp.
fn vectors(node: &Node) -> Result<VectorOffsetExp> {
    let vectors = node.all(VECTOR).map(vector).collect::<Result<Vec<_>>>()?;
    if vectors.len() > MAX_VECTORS {
        return Err(invalid("more than three vectors"));
    }
    VectorOffsetExp::new(vectors)
}

/// [§17.2.1] An ST_Vector: x and y, and z of a 3D one; none of an empty
/// one.
fn vector(node: &Node) -> Result<Vector> {
    let coordinate = |name: &str| node.child(name).map(number).transpose();
    let v = Vector { x: coordinate(X)?, y: coordinate(Y)?, z: coordinate(Z)? };
    if v.x.is_some() == v.y.is_some() && (v.z.is_none() || v.x.is_some()) {
        return Ok(v);
    }
    Err(invalid("a vector has an x without a y, or a z without both"))
}

/// The markup of `node`, for the GML reader of geometries.
fn markup(node: &Node, depth: usize) -> Result<String> {
    if depth > MARKUP_DEPTH {
        return Err(invalid("a geometry is nested too deep"));
    }
    let mut attrs = String::new();
    for (k, v) in &node.attrs {
        let _ = write!(attrs, " {k}=\"{}\"", escape(v));
    }
    let children = node.children.iter().map(|c| markup(c, depth.saturating_add(1))).collect::<Result<String>>()?;
    Ok(format!("<{n}{attrs}>{}{children}</{n}>", escape(&node.text), n = node.name))
}

/// What reading needs beyond the document.
struct Read<'c, 's> {
    /// The catalog constructors check against.
    cat: &'c Catalog<'c>,
    /// The reference systems a geometry's srsName names.
    srs: &'s dyn Srs,
}

impl Read<'_, '_> {
    /// [§15.7.1] An ST_LRMeasure: the number `node` holds, of the unit its
    /// attribute `uom` names.
    fn measure(&self, node: &Node) -> Result<LrMeasure> {
        LrMeasure::new_with_units(Some(number(node)?), node.attr(UOM).map(str::to_owned), self.cat.units)
    }

    /// The geometry inside the child `name`, if there is one.
    fn geometry(&self, node: &Node, name: &str) -> Result<Option<Geom>> {
        let Some(holder) = node.child(name) else { return Ok(None) };
        let inner = holder.children.first().ok_or_else(|| invalid(&format!("<{name}> holds no geometry")))?;
        read_geom(&markup(inner, 0)?, self.srs).map(Some)
    }

    /// [§15.1.1] An ST_LRM.
    fn lrm(&self, node: &Node) -> Result<Lrm> {
        let element = |k: &Node| (k.attr(NIL) != Some("true")).then(|| k.text.trim().to_owned());
        let constraints = node.child(CONSTRAINTS).map(|c| c.all(CONSTRAINT).map(element).collect());
        let parts = LrmParts {
            id: Some(integer(node, LRM_ID)?),
            name: Some(text(node, NAME)?),
            lrm_type: Some(text(node, TYPE)?),
            units: Some(text(node, UNIT_OF_MEASURE)?),
            constraints,
        };
        let offsets = OffsetParts {
            units: opt_text(node, OFFSET_UNIT),
            lateral: opt_text(node, LATERAL_DIRECTION),
            vertical: opt_text(node, VERTICAL_DIRECTION),
        };
        Lrm::new_with_offsets(parts, offsets, self.cat.units)
    }

    /// [§15.8.1] An ST_StartValue.
    fn start_value(&self, node: &Node) -> Result<StartValue> {
        Ok(StartValue::new(integer(node, LRM_ID)?, self.measure(child(node, MEASURE)?)?))
    }

    /// [§15.2.1 to §15.5.1] A linear element, of the subtype its element
    /// names.
    fn element(&self, node: &Node, depth: usize) -> Result<LinearElement> {
        let next = deeper(depth)?;
        let start_values = match node.child(START_VALUES) {
            Some(s) => Some(s.all(START_VALUE).map(|v| self.start_value(v)).collect::<Result<Vec<_>>>()?),
            None => None,
        };
        let parts = ElementParts {
            id: Some(integer(node, ELEMENT_ID)?),
            default_lrm: Some(integer(node, DEFAULT_LRM)?),
            default_measure: Some(self.measure(child(node, DEFAULT_MEASURE)?)?),
            element_type: opt_text(node, ELEMENT_TYPE),
            start_values,
        };
        match node.name.as_str() {
            LR_FEATURE => {
                let referents = match node.child(REFERENTS) {
                    Some(rs) => Some(rs.all(REFERENT).map(|r| self.referent(r, next)).collect::<Result<Vec<_>>>()?),
                    None => None,
                };
                Ok(LinearElement::Feature(LrFeature::new_with_referents(parts, Some(text(node, FEATURE_ID)?), referents)?))
            }
            LR_CURVE => Ok(LinearElement::Curve(LrCurve::new(parts, self.geometry(node, CURVE)?)?)),
            LR_DIRECTED_EDGE => {
                let topology_type = text(node, TOPOLOGY_TYPE)?.chars().next();
                let edge =
                    EdgeParts { topology_type, topology_name: Some(text(node, TOPOLOGY_NAME)?), edge_id: Some(integer(node, EDGE_ID)?) };
                Ok(LinearElement::Edge(LrDirectedEdge::new(parts, edge)?))
            }
            other => Err(invalid(&format!("<{other}> is no linear element"))),
        }
    }

    /// [§15.10.1] An ST_Referent.
    fn referent(&self, node: &Node, depth: usize) -> Result<Referent> {
        let next = deeper(depth)?;
        let position = self.geometry(node, POSITION)?;
        if position.as_ref().is_some_and(|p| !matches!(p.body, Body::Point(_))) {
            return Err(invalid("a referent's position is not a point"));
        }
        let location = match node.child(LOCATION) {
            Some(l) => Some(self.posexp(child(l, POSITION_EXPRESSION)?, next)?),
            None => None,
        };
        Referent::new(Some(text(node, NAME)?), Some(text(node, TYPE)?), position, location, self.cat)
    }

    /// [§15.6.1] An ST_PositionExp: its linear element or its id, its LRM
    /// or its id, and its distance expression.
    fn posexp(&self, node: &Node, depth: usize) -> Result<PositionExp> {
        let next = deeper(depth)?;
        let d = self.disexp(child(node, DISTANCE_EXPRESSION)?)?;
        let lrm = match node.child(LRM_VALUE) {
            Some(l) => Some(self.lrm(child(l, LRM)?)?),
            None => None,
        };
        let element = match node.child(LINEAR_ELEMENT) {
            Some(e) => Some(self.element(e.children.first().ok_or_else(|| invalid("<linearElement> is empty"))?, next)?),
            None => None,
        };
        Ok(match (element, lrm) {
            (Some(e), Some(l)) => PositionExp::of_element_and_lrm(e, l, d),
            (Some(e), None) => PositionExp::of_element_and_id(e, integer(node, LRM_ID)?, d),
            (None, Some(l)) => PositionExp::of_id_and_lrm(integer(node, ELEMENT_ID)?, l, d),
            (None, None) => PositionExp::of_ids(integer(node, ELEMENT_ID)?, integer(node, LRM_ID)?, d),
        })
    }

    /// [§15.9.1] An ST_DistanceExp, through the constructor of
    /// §15.9.2 Description 30 or, of a vector offset, 32.
    fn disexp(&self, node: &Node) -> Result<DistanceExp> {
        let along = self.measure(child(node, DISTANCE_ALONG)?)?;
        let names = ReferentNames {
            from_feature: opt_text(node, FROM_FEATURE),
            from_name: opt_text(node, FROM_NAME),
            towards_feature: opt_text(node, TOWARDS_FEATURE),
            towards_name: opt_text(node, TOWARDS_NAME),
        };
        if let Some(v) = node.child(VECTOR_OFFSET) {
            return DistanceExp::new_vector(Some(along), names, Some(vectors(v)?));
        }
        let lateral = match node.child(LATERAL_OFFSET) {
            Some(o) => {
                let (distance, geometry, description) = self.offset(o)?;
                Some(LatOffsetExp { distance, geometry, description })
            }
            None => None,
        };
        let vertical = match node.child(VERTICAL_OFFSET) {
            Some(o) => {
                let (distance, geometry, description) = self.offset(o)?;
                Some(VerOffsetExp { distance, geometry, description })
            }
            None => None,
        };
        DistanceExp::new(Some(along), names, lateral, vertical)
    }

    /// [§15.11.1, §15.12.1] An offset expression's attributes.
    fn offset(&self, node: &Node) -> Result<(Option<LrMeasure>, Option<Geom>, Option<String>)> {
        let distance = match node.child(OFFSET_DISTANCE) {
            Some(m) => Some(self.measure(m)?),
            None => None,
        };
        let geometry = self.geometry(node, FEATURE_GEOMETRY)?;
        let description = opt_text(node, OFFSET_DESCRIPTION);
        if geometry.is_some() && description.is_some() {
            return Err(invalid("an offset has both a feature geometry and an offset referent description"));
        }
        Ok((distance, geometry, description))
    }
}
