//! Writing the GML of a linear referencing value, as `lr/gml.rs` defines
//! it [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12 and its peers]:
//! the document `gml/read.rs` reads back to the same value, but for the m
//! coordinates GML does not carry.

use super::{
    CONSTRAINT, CONSTRAINTS, CURVE, DEFAULT_LRM, DEFAULT_MEASURE, DISTANCE_ALONG, DISTANCE_EXPRESSION, EDGE_ID, ELEMENT_ID, ELEMENT_TYPE,
    FEATURE_GEOMETRY, FEATURE_ID, FROM_FEATURE, FROM_NAME, LATERAL_DIRECTION, LATERAL_OFFSET, LINEAR_ELEMENT, LOCATION, LR_CURVE,
    LR_DIRECTED_EDGE, LR_FEATURE, LR_MEASURE, LRM, LRM_ID, LRM_VALUE, MEASURE, NAME, OFFSET_DESCRIPTION, OFFSET_DISTANCE, OFFSET_UNIT,
    POSITION, POSITION_EXPRESSION, REFERENT, REFERENTS, START_VALUE, START_VALUES, TOPOLOGY_NAME, TOPOLOGY_TYPE, TOWARDS_FEATURE,
    TOWARDS_NAME, TYPE, UNIT_OF_MEASURE, UOM, VECTOR, VECTOR_OFFSET, VERTICAL_DIRECTION, VERTICAL_OFFSET, X, Y, Z,
};
use crate::error::Result;
use crate::geometry::model::Geom;
use crate::spatial::formats::gml::xml::escape;
use crate::spatial::formats::gml::{Srs, write_geom};
use crate::spatial::lr::model::{DistanceExp, LinearElement, LrMeasure, Lrm, PositionExp, Referent, StartValue, VectorOffsetExp};
use crate::spatial::lr::{LrValue, deeper};

/// The document of `v`.
///
/// # Errors
///
/// As `gml::as_gml`.
pub(crate) fn value(v: &LrValue, srs: &dyn Srs) -> Result<String> {
    let w = Writer { srs };
    Ok(match v {
        LrValue::Lrm(l) => lrm(l),
        LrValue::LinearElement(e) => w.element(e, 0)?,
        LrValue::PositionExp(p) => w.posexp(p, 0)?,
        LrValue::LrMeasure(m) => measure(LR_MEASURE, m),
        LrValue::StartValue(s) => start_value(s),
        LrValue::DistanceExp(d) => w.disexp(d)?,
        LrValue::Referent(r) => w.referent(r, 0)?,
        LrValue::LatOffsetExp(o) => w.offset(LATERAL_OFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?,
        LrValue::VerOffsetExp(o) => w.offset(VERTICAL_OFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?,
        LrValue::VectorOffsetExp(o) => vectors(o),
    })
}

/// `<name>content</name>`, the content already markup.
fn wrap(name: &str, content: &str) -> String {
    format!("<{name}>{content}</{name}>")
}

/// `<name>text</name>`, the text escaped.
fn leaf(name: &str, text: &str) -> String {
    wrap(name, &escape(text))
}

/// `leaf` of a text that may be the null value: nothing if it is.
fn opt_leaf(name: &str, text: Option<&str>) -> String {
    text.map(|t| leaf(name, t)).unwrap_or_default()
}

/// [§15.7.1] A measure: its number, its unit the attribute `uom`.
fn measure(name: &str, m: &LrMeasure) -> String {
    match &m.units {
        Some(u) => format!("<{name} {UOM}=\"{}\">{}</{name}>", escape(u), m.measure),
        None => format!("<{name}>{}</{name}>", m.measure),
    }
}

/// [§15.1.1] An ST_LRM.
fn lrm(l: &Lrm) -> String {
    let mut out = leaf(LRM_ID, &l.id.to_string());
    out.push_str(&leaf(NAME, &l.name));
    out.push_str(&leaf(TYPE, &l.lrm_type));
    out.push_str(&leaf(UNIT_OF_MEASURE, &l.units));
    if let Some(cs) = &l.constraints {
        out.push_str(&wrap(CONSTRAINTS, &cs.iter().map(|c| leaf(CONSTRAINT, c)).collect::<String>()));
    }
    out.push_str(&opt_leaf(OFFSET_UNIT, l.offset_units.as_deref()));
    out.push_str(&opt_leaf(LATERAL_DIRECTION, l.lateral_direction.as_deref()));
    out.push_str(&opt_leaf(VERTICAL_DIRECTION, l.vertical_direction.as_deref()));
    wrap(LRM, &out)
}

/// [§15.8.1] An ST_StartValue.
fn start_value(s: &StartValue) -> String {
    wrap(START_VALUE, &format!("{}{}", leaf(LRM_ID, &s.lrm.to_string()), measure(MEASURE, &s.measure)))
}

/// [§15.13.1] An ST_VectorOffsetExp.
fn vectors(o: &VectorOffsetExp) -> String {
    let items: String = o
        .vectors
        .iter()
        .map(|v| {
            let coords = [(X, v.x), (Y, v.y), (Z, v.z)];
            wrap(VECTOR, &coords.iter().filter_map(|&(n, c)| c.map(|value| leaf(n, &value.to_string()))).collect::<String>())
        })
        .collect();
    wrap(VECTOR_OFFSET, &items)
}

/// What writing a geometry needs.
struct Writer<'s> {
    /// The reference systems, naming a geometry's srsName.
    srs: &'s dyn Srs,
}

impl Writer<'_> {
    /// A geometry inside the element `name`.
    fn geometry(&self, name: &str, g: &Geom) -> Result<String> {
        Ok(wrap(name, &write_geom(g, self.srs)?))
    }

    /// [§15.2.1 to §15.5.1] A linear element.
    fn element(&self, e: &LinearElement, depth: usize) -> Result<String> {
        let next = deeper(depth)?;
        let b = e.base();
        let mut out = leaf(ELEMENT_ID, &b.id.to_string());
        out.push_str(&leaf(DEFAULT_LRM, &b.default_lrm.to_string()));
        out.push_str(&measure(DEFAULT_MEASURE, &b.default_measure));
        out.push_str(&opt_leaf(ELEMENT_TYPE, b.element_type.as_deref()));
        if let Some(svs) = &b.start_values {
            out.push_str(&wrap(START_VALUES, &svs.iter().map(start_value).collect::<String>()));
        }
        let name = match e {
            LinearElement::Feature(f) => {
                out.push_str(&leaf(FEATURE_ID, &f.feature_id));
                if let Some(rs) = &f.referents {
                    let items = rs.iter().map(|r| self.referent(r, next)).collect::<Result<String>>()?;
                    out.push_str(&wrap(REFERENTS, &items));
                }
                LR_FEATURE
            }
            LinearElement::Curve(c) => {
                out.push_str(&self.geometry(CURVE, &c.curve)?);
                LR_CURVE
            }
            LinearElement::Edge(d) => {
                out.push_str(&leaf(TOPOLOGY_TYPE, &d.topology_type.to_string()));
                out.push_str(&leaf(TOPOLOGY_NAME, &d.topology_name));
                out.push_str(&leaf(EDGE_ID, &d.edge_id.to_string()));
                LR_DIRECTED_EDGE
            }
        };
        Ok(wrap(name, &out))
    }

    /// [§15.10.1] An ST_Referent.
    fn referent(&self, r: &Referent, depth: usize) -> Result<String> {
        let next = deeper(depth)?;
        let mut out = leaf(NAME, &r.name);
        out.push_str(&leaf(TYPE, &r.referent_type));
        if let Some(p) = &r.position {
            out.push_str(&self.geometry(POSITION, p)?);
        }
        if let Some(l) = &r.location {
            out.push_str(&wrap(LOCATION, &self.posexp(l, next)?));
        }
        Ok(wrap(REFERENT, &out))
    }

    /// [§15.6.1] An ST_PositionExp.
    fn posexp(&self, p: &PositionExp, depth: usize) -> Result<String> {
        let next = deeper(depth)?;
        let mut out = match &p.element {
            Some(e) => wrap(LINEAR_ELEMENT, &self.element(e, next)?),
            None => leaf(ELEMENT_ID, &p.element_id.to_string()),
        };
        out.push_str(&p.lrm.as_ref().map_or_else(|| leaf(LRM_ID, &p.lrm_id.to_string()), |l| wrap(LRM_VALUE, &lrm(l))));
        out.push_str(&self.disexp(&p.distance)?);
        Ok(wrap(POSITION_EXPRESSION, &out))
    }

    /// [§15.9.1] An ST_DistanceExp.
    fn disexp(&self, d: &DistanceExp) -> Result<String> {
        let mut out = measure(DISTANCE_ALONG, &d.along);
        out.push_str(&opt_leaf(FROM_FEATURE, d.from_feature.as_deref()));
        out.push_str(&opt_leaf(FROM_NAME, d.from_name.as_deref()));
        out.push_str(&opt_leaf(TOWARDS_FEATURE, d.towards_feature.as_deref()));
        out.push_str(&opt_leaf(TOWARDS_NAME, d.towards_name.as_deref()));
        if let Some(o) = &d.lateral {
            out.push_str(&self.offset(LATERAL_OFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?);
        }
        if let Some(o) = &d.vertical {
            out.push_str(&self.offset(VERTICAL_OFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?);
        }
        if let Some(o) = &d.vector {
            out.push_str(&vectors(o));
        }
        Ok(wrap(DISTANCE_EXPRESSION, &out))
    }

    /// [§15.11.1, §15.12.1] An offset expression, of element `name`.
    fn offset(&self, name: &str, distance: Option<&LrMeasure>, geometry: Option<&Geom>, description: Option<&str>) -> Result<String> {
        let mut out = distance.map(|m| measure(OFFSET_DISTANCE, m)).unwrap_or_default();
        if let Some(g) = geometry {
            out.push_str(&self.geometry(FEATURE_GEOMETRY, g)?);
        }
        out.push_str(&opt_leaf(OFFSET_DESCRIPTION, description));
        Ok(wrap(name, &out))
    }
}
