//! Moves values between the formats' tree and the geometry model, so each
//! format reads to and writes from a checked [`Geom`] (ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §5.1.62 to §5.1.64). The well-known
//! binary representation is the model's own.

use super::tree::{Body, Collection, Element, Geometry, Rings, Surface, Tin};
use crate::error::Result;
use crate::geometry::model::{self, Geom};
use crate::spatial::curves::adapt::{coords, flat, from_model, push, to_model};
use crate::spatial::curves::fault::{Fault, fault};
use crate::spatial::curves::tin::ElementKind;
use crate::spatial::curves::types::{Coord, Dims};

/// The checked model value of `value` in reference system `srid`.
///
/// # Errors
///
/// The model's checks' errors.
pub fn to_geom(value: &Geometry, srid: i32) -> Result<Geom> {
    Geom::new(srid, value.dims, body(&value.body, value.dims))
}

/// The tree of a model value.
///
/// # Errors
///
/// SQLSTATE 2FF02 for a TIN element of a type §17.1.1 does not define.
pub fn from_geom(g: &Geom) -> Result<Geometry> {
    Ok(Geometry { dims: g.dims, body: tree(&g.body, g.dims)? })
}

fn rings(r: &Rings, dims: Dims) -> model::Rings {
    r.iter().map(|ring| flat(ring, dims)).collect()
}

fn point(p: Option<Coord>, dims: Dims) -> model::Coords {
    let mut out = Vec::new();
    if let Some(found) = p {
        push(&mut out, found, dims);
    }
    out
}

fn body(b: &Body, dims: Dims) -> model::Body {
    match b {
        Body::Point(p) => model::Body::Point(point(*p, dims)),
        Body::Curve(c) => model::Body::Curve(to_model(c, dims)),
        Body::Surface(s) => model::Body::Surface(surface(s, dims)),
        Body::Solid(shells) => model::Body::Solid(shells.iter().map(|s| surface(s, dims)).collect()),
        Body::Collection(c) => model::Body::Collection(match c {
            Collection::Geometry(parts) => model::Collection::Geometry(parts.iter().map(|p| body(p, dims)).collect()),
            Collection::MultiPoint(points) => model::Collection::MultiPoint(points.iter().map(|p| point(*p, dims)).collect()),
            Collection::MultiCurve(curves) => model::Collection::MultiCurve(curves.iter().map(|one| to_model(one, dims)).collect()),
            Collection::MultiLineString(lines) => model::Collection::MultiLineString(rings(lines, dims)),
            Collection::MultiSurface(surfaces) => model::Collection::MultiSurface(surfaces.iter().map(|s| surface(s, dims)).collect()),
            Collection::MultiPolygon(polygons) => model::Collection::MultiPolygon(polygons.iter().map(|p| rings(p, dims)).collect()),
        }),
    }
}

fn surface(s: &Surface, dims: Dims) -> model::Surface {
    match s {
        Surface::CurvePolygon(curves) => model::Surface::CurvePolygon(curves.iter().map(|c| to_model(c, dims)).collect()),
        Surface::Polygon(r) => model::Surface::Polygon(rings(r, dims)),
        Surface::Triangle(ring) => model::Surface::Triangle(flat(ring, dims)),
        Surface::Polyhedral(patches) => model::Surface::PolyhdrlSurface(patches.iter().map(|p| rings(p, dims)).collect()),
        Surface::Tin(tin) => model::Surface::Tin(model::Tin {
            triangles: tin.triangles.iter().map(|t| flat(t, dims)).collect(),
            elements: tin
                .elements
                .iter()
                .map(|e| model::TinElement {
                    element_type: e.kind.name().to_owned(),
                    id: e.id,
                    tag: e.tag.clone(),
                    geometry: Box::new(body(&e.body, dims)),
                })
                .collect(),
            max_side_length: tin.max_side,
        }),
        Surface::Compound(parts) => model::Surface::Compound(parts.iter().map(|p| surface(p, dims)).collect()),
    }
}

fn tree_rings(r: &model::Rings, dims: Dims) -> Rings {
    r.iter().map(|ring| coords(ring, dims)).collect()
}

fn tree_point(p: &model::Coords, dims: Dims) -> Option<Coord> {
    coords(p, dims).first().copied()
}

fn tree(b: &model::Body, dims: Dims) -> Result<Body> {
    Ok(match b {
        model::Body::Point(p) => Body::Point(tree_point(p, dims)),
        model::Body::Curve(c) => Body::Curve(from_model(c, dims)),
        model::Body::Surface(s) => Body::Surface(tree_surface(s, dims)?),
        model::Body::Solid(shells) => Body::Solid(shells.iter().map(|s| tree_surface(s, dims)).collect::<Result<_>>()?),
        model::Body::Collection(c) => Body::Collection(match c {
            model::Collection::Geometry(parts) => Collection::Geometry(parts.iter().map(|p| tree(p, dims)).collect::<Result<_>>()?),
            model::Collection::MultiPoint(points) => Collection::MultiPoint(points.iter().map(|p| tree_point(p, dims)).collect()),
            model::Collection::MultiCurve(curves) => Collection::MultiCurve(curves.iter().map(|one| from_model(one, dims)).collect()),
            model::Collection::MultiLineString(lines) => Collection::MultiLineString(tree_rings(lines, dims)),
            model::Collection::MultiSurface(surfaces) => {
                Collection::MultiSurface(surfaces.iter().map(|s| tree_surface(s, dims)).collect::<Result<_>>()?)
            }
            model::Collection::MultiPolygon(polygons) => Collection::MultiPolygon(polygons.iter().map(|p| tree_rings(p, dims)).collect()),
        }),
    })
}

fn tree_surface(s: &model::Surface, dims: Dims) -> Result<Surface> {
    Ok(match s {
        model::Surface::CurvePolygon(curves) => Surface::CurvePolygon(curves.iter().map(|c| from_model(c, dims)).collect()),
        model::Surface::Polygon(r) => Surface::Polygon(tree_rings(r, dims)),
        model::Surface::Triangle(ring) => Surface::Triangle(coords(ring, dims)),
        model::Surface::PolyhdrlSurface(patches) => Surface::Polyhedral(patches.iter().map(|p| tree_rings(p, dims)).collect()),
        model::Surface::Tin(tin) => Surface::Tin(Tin {
            triangles: tin.triangles.iter().map(|t| coords(t, dims)).collect(),
            elements: tin.elements.iter().map(|e| element(e, dims)).collect::<Result<_>>()?,
            max_side: tin.max_side_length,
        }),
        model::Surface::Compound(parts) => Surface::Compound(parts.iter().map(|p| tree_surface(p, dims)).collect::<Result<_>>()?),
    })
}

fn element(e: &model::TinElement, dims: Dims) -> Result<Element> {
    let kind = ElementKind::from_name(&e.element_type)
        .or_else(|| ElementKind::from_keyword(&e.element_type))
        .ok_or_else(|| fault(Fault::InvalidArgument, "§17.1.1", &format!("'{}' is no TIN element type", e.element_type)))?;
    Ok(Element { kind, id: e.id, tag: e.tag.clone(), body: tree(&e.geometry, dims)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::model::{read_wkb, write_wkb};
    use crate::spatial::formats::samples::{DIMS, values};

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] Every type
    /// in every dimension passes the model's checks and its well-known
    /// binary representation, and comes back unchanged.
    #[test]
    fn every_type_through_wkb() {
        for dims in DIMS {
            for value in values(dims) {
                let g = to_geom(&value, 4326).unwrap_or_else(|e| panic!("{value:?}: {e:?}"));
                let back = read_wkb(&write_wkb(&g), 4326).unwrap();
                assert_eq!(from_geom(&back).unwrap(), value);
            }
        }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1] A TIN
    /// element of an undefined type has no tree form.
    #[test]
    fn unknown_element_type() {
        let e = model::TinElement {
            element_type: "wall".into(),
            id: 0,
            tag: String::new(),
            geometry: Box::new(model::Body::Point(Vec::new())),
        };
        assert_eq!(element(&e, Dims::Xy).unwrap_err().sqlstate().as_str(), "2FF02");
    }
}
