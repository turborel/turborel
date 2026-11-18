//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14.1 to §15.14.4]
//! Writing linear referencing well-known text: the text `wkt/read.rs`
//! reads back to the same value. Numbers are written in Rust's shortest
//! form that reads back to the same DOUBLE PRECISION.

use super::{
    DEFAULT, DISEXP, E, EMPTY, FID, FROM, L, LATERALOFFSET, LEID, LETYPE, LINEARELEMENT, LOCATION, LRCURVE, LRDIRECTEDEDGE, LRFEATURE, LRM,
    LRMCONSTRAINTS, LRMID, LRMNAME, LRMOFFSETUOM, LRMTYPE, LRMUOM, MEASURE, NAME, NULL, POSEXP, POSITION, POSLATOFFDIR, POSVEROFFDIR,
    REFERENTS, STARTVALUES, STOP_WORDS, TOWARDS, TYPE, VECTOR, VECTOROFFSETS, VERTICALOFFSET, Z, write_geometry,
};
use crate::error::Result;
use crate::geometry::model::Geom;
use crate::spatial::lr::model::{DistanceExp, LinearElement, LrMeasure, Lrm, PositionExp, Referent, StartValue, Vector, VectorOffsetExp};
use crate::spatial::lr::{LrValue, deeper};
use std::fmt::Write as _;

/// The text of `v`.
///
/// # Errors
///
/// As `wkt::as_text`.
pub(crate) fn value(v: &LrValue) -> Result<String> {
    Ok(match v {
        LrValue::Lrm(l) => format!("{LRM} {}", lrm(l)),
        LrValue::LinearElement(e) => format!("{LINEARELEMENT} {}", element(e, 0)?),
        LrValue::PositionExp(p) => posexp(p, 0)?,
        LrValue::LrMeasure(m) => format!("{MEASURE} {}", measure(m)),
        LrValue::StartValue(s) => start_value(s),
        LrValue::DistanceExp(d) => disexp(d, 0)?,
        LrValue::Referent(r) => referent(r, 0)?,
        LrValue::LatOffsetExp(o) => offset(LATERALOFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?,
        LrValue::VerOffsetExp(o) => offset(VERTICALOFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?,
        LrValue::VectorOffsetExp(o) => format!("{VECTOROFFSETS} {}", vectors(o)),
    })
}

/// [§15.14.5 `<text>`] `s` double-quoted, \[EXTENSION\] a double quote in
/// it doubled.
fn text(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// [§15.14.2 `<measure text>`] A measure value and its unit: bare if its
/// words read back as one `<unit of measure>`, else \[EXTENSION\] a
/// `<text>`.
fn measure(m: &LrMeasure) -> String {
    let Some(u) = &m.units else { return m.measure.to_string() };
    if bare(u) { format!("{} {u}", m.measure) } else { format!("{} {}", m.measure, text(u)) }
}

/// Whether a unit name reads back from its bare words: each of the
/// characters a unit word takes, none a key word ending a unit, none
/// NULL.
fn bare(unit: &str) -> bool {
    let words: Vec<&str> = unit.split(' ').collect();
    let plain = |w: &&str| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '\''));
    let keyword = |w: &&str| STOP_WORDS.iter().chain([NULL].iter()).any(|k| k.eq_ignore_ascii_case(w));
    let starts_numeric = unit.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.'));
    words.iter().all(plain) && !words.iter().any(keyword) && !starts_numeric
}

/// `items` in parentheses, separated by commas.
fn listed(items: &[String]) -> String {
    format!("({})", items.join(", "))
}

/// [§15.14.3] `<lrm text body>` of an `<lrm representation>`.
fn lrm(l: &Lrm) -> String {
    let mut out = format!("{LRMID} {}, {LRMNAME} {}, {LRMTYPE} {}, {LRMUOM} {}", l.id, text(&l.name), text(&l.lrm_type), text(&l.units));
    if let Some(cs) = &l.constraints {
        let items: Vec<String> = cs.iter().map(|c| text(c)).collect();
        let _ = write!(out, ", {LRMCONSTRAINTS} {}", listed(&items));
    }
    if let Some(u) = &l.offset_units {
        let _ = write!(out, ", {LRMOFFSETUOM} {}", text(u));
        if let Some(d) = &l.lateral_direction {
            let _ = write!(out, ", {POSLATOFFDIR} {}", text(d));
        }
        if let Some(d) = &l.vertical_direction {
            let _ = write!(out, ", {POSVEROFFDIR} {}", text(d));
        }
    }
    out
}

/// [§15.14.3 Description 1a] `<start value>`.
fn start_value(s: &StartValue) -> String {
    format!("({LRMID} {}, {})", s.lrm, measure(&s.measure))
}

/// [§15.14.2] `<linear element representation>`.
fn element(e: &LinearElement, depth: usize) -> Result<String> {
    let next = deeper(depth)?;
    let b = e.base();
    let word = match e {
        LinearElement::Feature(_) => LRFEATURE,
        LinearElement::Curve(_) => LRCURVE,
        LinearElement::Edge(_) => LRDIRECTEDEDGE,
    };
    let element_type = b.element_type.as_deref().map_or_else(|| NULL.to_owned(), text);
    let mut out = format!(
        "{word} {LEID} {}, {DEFAULT} {LRMID} {}, {DEFAULT} {MEASURE} {}, {LETYPE} {element_type}",
        b.id,
        b.default_lrm,
        measure(&b.default_measure)
    );
    if let Some(svs) = &b.start_values {
        let items: Vec<String> = svs.iter().map(start_value).collect();
        let _ = write!(out, ", {STARTVALUES} {}", listed(&items));
    }
    out.push_str(", ");
    match e {
        LinearElement::Feature(f) => {
            let _ = write!(out, "{FID} {}", text(&f.feature_id));
            if let Some(rs) = &f.referents {
                let items = rs.iter().map(|r| referent(r, next)).collect::<Result<Vec<_>>>()?;
                let _ = write!(out, " {REFERENTS} {}", listed(&items));
            }
        }
        LinearElement::Curve(c) => out.push_str(&write_geometry(&c.curve)?),
        LinearElement::Edge(d) => {
            let topology = if d.topology_type.eq_ignore_ascii_case(&'L') { L } else { E };
            let _ = write!(out, "{topology} {} {}", text(&d.topology_name), d.edge_id);
        }
    }
    Ok(out)
}

/// [§15.14.2 Description 1o] `<referent>`.
fn referent(r: &Referent, depth: usize) -> Result<String> {
    let next = deeper(depth)?;
    let mut out = format!("({NAME} {}, {TYPE} {}", text(&r.name), text(&r.referent_type));
    if let Some(p) = &r.position {
        let _ = write!(out, ", {POSITION} {}", write_geometry(p)?);
    }
    if let Some(l) = &r.location {
        let _ = write!(out, ", {LOCATION} {}", posexp(l, next)?);
    }
    out.push(')');
    Ok(out)
}

/// [§15.14.1] `<position expression text representation>`.
fn posexp(p: &PositionExp, depth: usize) -> Result<String> {
    let next = deeper(depth)?;
    let element_text = match &p.element {
        Some(e) => element(e, next)?,
        None => format!("{LEID} {}", p.element_id),
    };
    let lrm_text = p.lrm.as_ref().map_or_else(|| format!("{LRMID} {}", p.lrm_id), lrm);
    Ok(format!("{POSEXP} ({LINEARELEMENT} {element_text}), ({LRM} {lrm_text}), ({})", disexp(&p.distance, next)?))
}

/// [§15.14.4] `<distance expression text representation>`. A vector
/// offset is written only of a value of no lateral and no vertical offset,
/// the values §15.9.10 Description 4 lets have one.
fn disexp(d: &DistanceExp, depth: usize) -> Result<String> {
    deeper(depth)?;
    let mut out = format!("{DISEXP} {MEASURE} {}", measure(&d.along));
    if let Some(name) = &d.from_name {
        let _ = write!(out, " {FROM} {}", referent_names(d.from_feature.as_deref(), name));
    }
    if let Some(name) = &d.towards_name {
        let _ = write!(out, " {TOWARDS} {}", referent_names(d.towards_feature.as_deref(), name));
    }
    if let Some(o) = &d.lateral {
        let _ = write!(out, ", {}", offset(LATERALOFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?);
    }
    if let Some(o) = &d.vertical {
        let _ = write!(out, ", {}", offset(VERTICALOFFSET, o.distance.as_ref(), o.geometry.as_ref(), o.description.as_deref())?);
    }
    if let (Some(o), None, None) = (&d.vector, &d.lateral, &d.vertical) {
        let _ = write!(out, ", {VECTOROFFSETS} {}", vectors(o));
    }
    Ok(out)
}

/// [§15.14.4] `[<fid representation>] <referent name>`.
fn referent_names(feature: Option<&str>, name: &str) -> String {
    match feature {
        Some(f) => format!("{} {}", text(f), text(name)),
        None => text(name),
    }
}

/// [§15.14.4 Description 1h, 1l] An offset expression of key word
/// `word`.
fn offset(word: &str, distance: Option<&LrMeasure>, geometry: Option<&Geom>, description: Option<&str>) -> Result<String> {
    let mut out = format!("{word} {}", distance.map_or_else(|| NULL.to_owned(), measure));
    if let Some(g) = geometry {
        let _ = write!(out, " {FROM} {}", write_geometry(g)?);
    } else if let Some(desc) = description {
        let _ = write!(out, " {FROM} {}", text(desc));
    }
    Ok(out)
}

/// [§15.14.4 Description 1n] `<vectors>`.
fn vectors(o: &VectorOffsetExp) -> String {
    let items: Vec<String> = o.vectors.iter().map(vector).collect();
    listed(&items)
}

/// [§17.2.22] `<vector text representation>`.
fn vector(v: &Vector) -> String {
    match (v.x, v.y, v.z) {
        (Some(x), Some(y), Some(z)) => format!("{VECTOR} {Z} ({x} {y} {z})"),
        (Some(x), Some(y), None) => format!("{VECTOR} ({x} {y})"),
        (None, _, Some(_)) | (_, None, Some(_)) => format!("{VECTOR} {Z} {EMPTY}"),
        (None, _, None) | (_, None, None) => format!("{VECTOR} {EMPTY}"),
    }
}
