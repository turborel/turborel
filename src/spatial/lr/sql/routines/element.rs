//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2 to §15.5] The routines
//! of ST_LinearElement and of its subtypes ST_LRFeature, ST_LRCurve and
//! ST_LRDirectedEdge. A method of ST_LinearElement takes SELF of each
//! subtype, and a type-preserving one returns SELF's declared type.

use super::super::args::{Args, P, element_out, geom_out, int, lr_array, lr_out, or_null, text};
use super::super::routine::{Routine, called, null_call};
use super::super::{FEATURE_ID_LENGTH, TOPO_OR_NET_NAME_LENGTH, TYPE_LENGTH, from_gml_arg, from_text_arg, from_text_or_gml_arg};
use super::{CURVE, DISTANCE, EDGE, ELEMENT, FEATURE, MEASURE, POSITION};
use crate::error::Result;
use crate::geometry::model::Kind as GeomKind;
use crate::spatial::lr::{EdgeParts, ElementParts, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrKind, LrValue, required};
use crate::value::Value;

const SELF: &[P] = &[ELEMENT];
const SELF_INT: &[P] = &[ELEMENT, P::Int];
const PARTS: [P; 5] = [P::Int, P::Int, MEASURE, P::Text(TYPE_LENGTH), P::LrArray(LrKind::StartValue)];
const FEATURE_SIX: &[P] = &[PARTS[0], PARTS[1], PARTS[2], PARTS[3], PARTS[4], P::Text(FEATURE_ID_LENGTH)];
const FEATURE_SEVEN: &[P] = &[PARTS[0], PARTS[1], PARTS[2], PARTS[3], PARTS[4], P::Text(FEATURE_ID_LENGTH), P::LrArray(LrKind::Referent)];
const CURVE_SIX: &[P] = &[PARTS[0], PARTS[1], PARTS[2], PARTS[3], PARTS[4], P::Geo(GeomKind::Curve)];
const EDGE_EIGHT: &[P] = &[PARTS[0], PARTS[1], PARTS[2], PARTS[3], PARTS[4], P::Char, P::Text(TOPO_OR_NET_NAME_LENGTH), P::Int];

/// SELF, a linear element or the null value.
fn this(a: &Args<'_>) -> Result<Option<LinearElement>> {
    a.element(0)
}

/// The parts every subtype's constructor takes first.
fn parts(a: &Args<'_>) -> Result<ElementParts> {
    Ok(ElementParts {
        id: a.int(0)?,
        default_lrm: a.int(1)?,
        default_measure: a.measure(2)?,
        element_type: a.text(3)?,
        start_values: a.start_values(4)?,
    })
}

/// An ST_LRFeature value or the null value.
fn feature_out(f: Option<LrFeature>) -> Result<Value> {
    element_out(f.map(LinearElement::Feature))
}

/// An ST_LRCurve value or the null value.
fn curve_out(c: Option<LrCurve>) -> Result<Value> {
    element_out(c.map(LinearElement::Curve))
}

/// An ST_LRDirectedEdge value or the null value.
fn edge_out(e: Option<LrDirectedEdge>) -> Result<Value> {
    element_out(e.map(LinearElement::Edge))
}

/// [§15.2.4 Description 4] ST_DefaultMeasure(ST_LRMeasure), raising the
/// warning it gives.
fn with_default_measure(a: &Args<'_>) -> Result<Value> {
    let Some((e, warning)) = LinearElement::with_default_measure(this(a)?.as_ref(), a.measure(1)?)? else { return Ok(Value::Null) };
    a.warn(warning);
    element_out(Some(e))
}

/// [§15.2.6 Description 2] ST_StartValue(INTEGER); of a null SELF, the
/// null value once anlrmid is not null.
fn start_value(a: &Args<'_>) -> Result<Value> {
    let id = a.int(1)?;
    let Some(e) = this(a)? else {
        required(id, "§15.2.6 Description 2a", "anlrmid")?;
        return Ok(Value::Null);
    };
    lr_out(Some(LrValue::LrMeasure(e.start_value(id, a.catalog.lrms)?)))
}

/// [§15.2.7] ST_TranslateToInst.
fn translate_to_inst(a: &Args<'_>) -> Result<Value> {
    let (Some(e), source, target) = (this(a)?, a.position(1)?, a.element(2)?) else { return Ok(Value::Null) };
    lr_out(Some(LrValue::DistanceExp(e.translate_to_inst(source.as_ref(), target.as_ref(), a.int(3)?, &a.catalog)?)))
}

/// [§15.2.8] ST_TranslateToType.
fn translate_to_type(a: &Args<'_>) -> Result<Value> {
    let (Some(e), source, element_type) = (this(a)?, a.position(1)?, a.text(2)?) else { return Ok(Value::Null) };
    let found = e.translate_to_type(source.as_ref(), element_type.as_deref(), a.int(3)?, &a.catalog)?;
    lr_array(found.into_iter().map(LrValue::PositionExp).collect())
}

/// The routines.
pub static ROUTINES: &[Routine] = &[
    // [§15.2.2 to §15.2.6] ST_LinearElement's observers and mutators.
    null_call("ST_LINEARELEMENTID", SELF, P::Int, |a| Ok(or_null(this(a)?.map(|e| e.linear_element_id()), int))),
    called("ST_LINEARELEMENTID", SELF_INT, P::SelfType, |a| {
        element_out(LinearElement::with_linear_element_id(this(a)?.as_ref(), a.int(1)?)?)
    }),
    null_call("ST_DEFAULTLRM", SELF, P::Int, |a| Ok(or_null(this(a)?.map(|e| e.default_lrm()), int))),
    called("ST_DEFAULTLRM", SELF_INT, P::SelfType, |a| element_out(LinearElement::with_default_lrm(this(a)?.as_ref(), a.int(1)?)?)),
    null_call("ST_DEFAULTMEASURE", SELF, MEASURE, |a| lr_out(this(a)?.map(|e| LrValue::LrMeasure(e.default_measure().clone())))),
    called("ST_DEFAULTMEASURE", &[ELEMENT, MEASURE], P::SelfType, with_default_measure),
    null_call("ST_LETYPE", SELF, P::Text(TYPE_LENGTH), |a| Ok(or_null(this(a)?.as_ref().and_then(LinearElement::le_type), text))),
    called("ST_LETYPE", &[ELEMENT, P::Text(TYPE_LENGTH)], P::SelfType, |a| {
        element_out(LinearElement::with_le_type(this(a)?.as_ref(), a.text(1)?))
    }),
    called("ST_STARTVALUE", SELF_INT, MEASURE, start_value).reading(),
    called("ST_STARTVALUE", &[ELEMENT, P::Int, MEASURE], P::SelfType, |a| {
        element_out(LinearElement::with_start_value(this(a)?.as_ref(), a.int(1)?, a.measure(2)?)?)
    }),
    // [§15.2.7, §15.2.8] Translating.
    null_call("ST_TRANSLATETOINST", &[ELEMENT, POSITION, ELEMENT, P::Int], DISTANCE, translate_to_inst).reading(),
    null_call("ST_TRANSLATETOTYPE", &[ELEMENT, POSITION, P::Text(TYPE_LENGTH), P::Int], P::LrArray(LrKind::PositionExp), translate_to_type)
        .reading(),
    // [§15.2.9, §15.2.10] The functions of the text and GML.
    null_call("ST_LEFROMTEXT", &[P::Clob], ELEMENT, |a| from_text_arg(a, LrKind::LinearElement)),
    null_call("ST_LEFROMGML", &[P::Clob], ELEMENT, |a| from_gml_arg(a, LrKind::LinearElement)),
    // [§15.3] ST_LRFeature.
    null_call("ST_LRFEATURE", &[P::Clob], FEATURE, |a| from_text_or_gml_arg(a, LrKind::LrFeature)),
    called("ST_LRFEATURE", FEATURE_SIX, FEATURE, |a| feature_out(Some(LrFeature::new(parts(a)?, a.text(5)?)?))),
    called("ST_LRFEATURE", FEATURE_SEVEN, FEATURE, |a| {
        feature_out(Some(LrFeature::new_with_referents(parts(a)?, a.text(5)?, a.referents(6)?)?))
    }),
    null_call("ST_FEATUREID", &[FEATURE], P::Text(FEATURE_ID_LENGTH), |a| {
        Ok(or_null(a.feature(0)?.as_ref().map(LrFeature::feature_id), text))
    }),
    called("ST_FEATUREID", &[FEATURE, P::Text(FEATURE_ID_LENGTH)], FEATURE, |a| {
        feature_out(LrFeature::with_feature_id(a.feature(0)?.as_ref(), a.text(1)?)?)
    }),
    null_call("ST_REFERENTS", &[FEATURE], P::LrArray(LrKind::Referent), |a| {
        let Some(f) = a.feature(0)? else { return Ok(Value::Null) };
        f.referents().map_or(Ok(Value::Null), |rs| lr_array(rs.iter().cloned().map(LrValue::Referent).collect()))
    }),
    called("ST_REFERENTS", &[FEATURE, P::LrArray(LrKind::Referent)], FEATURE, |a| {
        feature_out(LrFeature::with_referents(a.feature(0)?.as_ref(), a.referents(1)?))
    }),
    null_call("ST_LRFEATFROMTEXT", &[P::Clob], FEATURE, |a| from_text_arg(a, LrKind::LrFeature)),
    null_call("ST_LRFEATFROMGML", &[P::Clob], FEATURE, |a| from_gml_arg(a, LrKind::LrFeature)),
    // [§15.4] ST_LRCurve.
    null_call("ST_LRCURVE", &[P::Clob], CURVE, |a| from_text_or_gml_arg(a, LrKind::LrCurve)),
    called("ST_LRCURVE", CURVE_SIX, CURVE, |a| curve_out(Some(LrCurve::new(parts(a)?, a.geom(5, GeomKind::Curve)?)?))),
    null_call("ST_CURVE", &[CURVE], P::Geo(GeomKind::Curve), |a| Ok(geom_out(a.curve(0)?.as_ref().map(LrCurve::curve)))),
    called("ST_CURVE", &[CURVE, P::Geo(GeomKind::Curve)], CURVE, |a| {
        curve_out(LrCurve::with_curve(a.curve(0)?.as_ref(), a.geom(1, GeomKind::Curve)?)?)
    }),
    null_call("ST_POINT", &[CURVE, POSITION], P::Geo(GeomKind::Point), |a| {
        let Some(c) = a.curve(0)? else { return Ok(Value::Null) };
        Ok(c.point(a.position(1)?.as_ref(), &a.catalog)?.to_value())
    })
    .reading(),
    null_call("ST_LRPOSITION", &[CURVE, P::Geo(GeomKind::Point)], POSITION, |a| {
        let Some(c) = a.curve(0)? else { return Ok(Value::Null) };
        lr_out(Some(LrValue::PositionExp(c.lr_position(a.geom(1, GeomKind::Point)?.as_ref(), &a.catalog)?)))
    })
    .reading(),
    null_call("ST_LRCURVEFROMTEXT", &[P::Clob], CURVE, |a| from_text_arg(a, LrKind::LrCurve)),
    null_call("ST_LRCURVEFROMGML", &[P::Clob], CURVE, |a| from_gml_arg(a, LrKind::LrCurve)),
    // [§15.5] ST_LRDirectedEdge.
    null_call("ST_LRDIRECTEDEDGE", &[P::Clob], EDGE, |a| from_text_or_gml_arg(a, LrKind::LrDirectedEdge)),
    called("ST_LRDIRECTEDEDGE", EDGE_EIGHT, EDGE, |a| {
        let edge = EdgeParts { topology_type: a.character(5)?, topology_name: a.text(6)?, edge_id: a.int(7)? };
        edge_out(Some(LrDirectedEdge::new(parts(a)?, edge)?))
    }),
    null_call("ST_TOPOLOGYTYPE", &[EDGE], P::Char, |a| Ok(or_null(a.edge(0)?.map(|e| e.topology_type()), |c| Value::Char(c.to_string())))),
    called("ST_TOPOLOGYTYPE", &[EDGE, P::Char], EDGE, |a| {
        edge_out(LrDirectedEdge::with_topology_type(a.edge(0)?.as_ref(), a.character(1)?)?)
    }),
    null_call("ST_TOPOORNETNAME", &[EDGE], P::Text(TOPO_OR_NET_NAME_LENGTH), |a| {
        Ok(or_null(a.edge(0)?.as_ref().map(LrDirectedEdge::topo_or_net_name), text))
    }),
    called("ST_TOPOORNETNAME", &[EDGE, P::Text(TOPO_OR_NET_NAME_LENGTH)], EDGE, |a| {
        edge_out(LrDirectedEdge::with_topo_or_net_name(a.edge(0)?.as_ref(), a.text(1)?)?)
    }),
    null_call("ST_EDGEORLINKID", &[EDGE], P::Int, |a| Ok(or_null(a.edge(0)?.map(|e| e.edge_or_link_id()), int))),
    called("ST_EDGEORLINKID", &[EDGE, P::Int], EDGE, |a| edge_out(LrDirectedEdge::with_edge_or_link_id(a.edge(0)?.as_ref(), a.int(1)?)?)),
    null_call("ST_LREDGEFROMTEXT", &[P::Clob], EDGE, |a| from_text_arg(a, LrKind::LrDirectedEdge)),
    null_call("ST_LREDGEFROMGML", &[P::Clob], EDGE, |a| from_gml_arg(a, LrKind::LrDirectedEdge)),
];
