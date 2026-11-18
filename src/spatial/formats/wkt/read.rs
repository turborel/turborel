//! Reads the well-known text representation, ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5.1.67: the value types, surfaces with their patches and
//! TIN elements, the solid and the collections. Curves are in
//! [`super::curves`].

use super::cursor::{Cursor, mixed};
use super::curves::{curve, curve_text};
use super::lexer::{Token, invalid, tokens};
use crate::error::Result;
use crate::spatial::curves::tin::ElementKind;
use crate::spatial::curves::types::{Coord, Dims};
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Kind, Ring, Rings, Surface, Tin, element_kind_of};

/// The value `text` represents.
///
/// # Errors
///
/// SQLSTATE 2FF22 for text the grammar does not produce, 2FF25 for mixed
/// coordinate dimensions, 2FF84 for missing measures and 2FF02 for an
/// impossible part.
pub fn read(text: &str) -> Result<Geometry> {
    let mut c = Cursor::new(tokens(text)?);
    let kind = c.keyword(true)?;
    if kind == Kind::BRepSolid && c.dims != Dims::Xyz {
        return Err(mixed("a BREPSOLID is Z"));
    }
    let body = body(&mut c, kind)?;
    if !c.done() {
        return Err(invalid("text follows the value"));
    }
    Ok(Geometry { dims: c.dims, body })
}

/// A `<well-known text representation>` nested in another.
fn representation(c: &mut Cursor) -> Result<Body> {
    let kind = c.keyword(false)?;
    body(c, kind)
}

/// The body of a value of type `kind`, after its keyword.
fn body(c: &mut Cursor, kind: Kind) -> Result<Body> {
    Ok(match kind {
        Kind::Point => Body::Point(c.point_text(c.dims)?),
        Kind::LineString
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::Elliptical
        | Kind::Nurbs
        | Kind::Clothoid
        | Kind::Spiral
        | Kind::CompoundCurve => Body::Curve(curve(c, kind)?),
        Kind::CurvePolygon | Kind::Polygon | Kind::Triangle | Kind::Polyhedral | Kind::Tin | Kind::CompoundSurface => {
            Body::Surface(surface(c, kind)?)
        }
        Kind::BRepSolid => Body::Solid(c.list(shell)?),
        Kind::GeometryCollection => Body::Collection(Collection::Geometry(c.list(representation)?)),
        Kind::MultiPoint => Body::Collection(Collection::MultiPoint(c.list(multipoint_member)?)),
        Kind::MultiCurve => Body::Collection(Collection::MultiCurve(c.list(curve_text)?)),
        Kind::MultiLineString => Body::Collection(Collection::MultiLineString(c.list(Cursor::points)?)),
        Kind::MultiSurface => Body::Collection(Collection::MultiSurface(c.list(surface_text)?)),
        Kind::MultiPolygon => Body::Collection(Collection::MultiPolygon(c.list(polygon)?)),
    })
}

/// A multipoint's `<point text>`; a bare point is read too.
fn multipoint_member(c: &mut Cursor) -> Result<Option<Coord>> {
    if c.peek() == Some(&Token::Open) || c.is_word("EMPTY") { c.point_text(c.dims) } else { c.point(c.dims).map(Some) }
}

/// The body of a surface of type `kind`.
fn surface(c: &mut Cursor, kind: Kind) -> Result<Surface> {
    Ok(match kind {
        Kind::CurvePolygon => Surface::CurvePolygon(c.list(curve_text)?),
        Kind::Polygon => Surface::Polygon(polygon(c)?),
        Kind::Triangle => Surface::Triangle(triangle(c)?),
        Kind::Polyhedral => Surface::Polyhedral(patches(c, patch)?),
        Kind::Tin => Surface::Tin(tin(c)?),
        Kind::CompoundSurface => Surface::Compound(c.list(surface_text)?),
        Kind::Point
        | Kind::LineString
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::Elliptical
        | Kind::Nurbs
        | Kind::Clothoid
        | Kind::Spiral
        | Kind::CompoundCurve
        | Kind::BRepSolid
        | Kind::GeometryCollection
        | Kind::MultiPoint
        | Kind::MultiCurve
        | Kind::MultiLineString
        | Kind::MultiSurface
        | Kind::MultiPolygon => return Err(invalid(&format!("{} is not a surface type", kind.keyword()))),
    })
}

/// A `<surface text>`: a bare `<polygon text body>` or a surface with
/// its keyword.
fn surface_text(c: &mut Cursor) -> Result<Surface> {
    if c.is_word("EMPTY") || c.peek() == Some(&Token::Open) {
        return Ok(Surface::Polygon(polygon(c)?));
    }
    let kind = c.keyword(false)?;
    surface(c, kind)
}

/// A `<shell text>`: a polyhedral or compound surface.
fn shell(c: &mut Cursor) -> Result<Surface> {
    let kind = c.keyword(false)?;
    if !matches!(kind, Kind::Polyhedral | Kind::CompoundSurface) {
        return Err(invalid("a BREPSOLID's shell is a POLYHEDRALSURFACE or a COMPOUNDSURFACE"));
    }
    surface(c, kind)
}

/// `<polygon text>`: `<empty set>` or a list of rings.
fn polygon(c: &mut Cursor) -> Result<Rings> {
    c.list(Cursor::points)
}

/// `<triangle text>`: three points, kept as a closed ring of four.
fn triangle(c: &mut Cursor) -> Result<Ring> {
    let dims = c.dims;
    let mut ring = c.list(|c| c.point(dims))?;
    if ring.is_empty() {
        return Ok(ring);
    }
    if ring.len() != 3 {
        return Err(invalid("a triangle has three points"));
    }
    if let Some(first) = ring.first().copied() {
        ring.push(first);
    }
    Ok(ring)
}

/// A `<polygon text representation>` patch: a POLYGON or a TRIANGLE.
fn patch(c: &mut Cursor) -> Result<Rings> {
    let kind = c.keyword(false)?;
    if kind == Kind::Polygon {
        polygon(c)
    } else if kind == Kind::Triangle {
        Ok(vec![triangle(c)?])
    } else {
        Err(invalid(&format!("a polyhedral surface's patch is no {}", kind.keyword())))
    }
}

/// `<empty set>` or `( PATCHES ( item, ... ) ...`, leaving the outer
/// parenthesis open for what follows the patches.
fn patches_open<T>(c: &mut Cursor, item: impl FnMut(&mut Cursor) -> Result<T>) -> Result<Option<Vec<T>>> {
    if !c.opens()? {
        return Ok(None);
    }
    c.word("PATCHES")?;
    c.expect(&Token::Open)?;
    Ok(Some(c.items(item)?))
}

/// `<polyhedralsurface text>`.
fn patches<T>(c: &mut Cursor, item: impl FnMut(&mut Cursor) -> Result<T>) -> Result<Vec<T>> {
    let Some(found) = patches_open(c, item)? else {
        return Ok(Vec::new());
    };
    c.expect(&Token::Close)?;
    Ok(found)
}

/// `<tin text>`: triangle patches, then optional elements and maximum
/// side length.
fn tin(c: &mut Cursor) -> Result<Tin> {
    let Some(triangles) = patches_open(c, triangle)? else {
        return Ok(Tin { triangles: Vec::new(), elements: Vec::new(), max_side: None });
    };
    let elements = if c.take_word("ELEMENTS") {
        c.expect(&Token::Open)?;
        c.items(element)?
    } else {
        Vec::new()
    };
    let max_side = if c.take_word("MAXSIDELENGTH") { Some(c.number()?) } else { None };
    c.expect(&Token::Close)?;
    Ok(Tin { triangles, elements, max_side })
}

/// A `<tinelementtype text>`: its keyword, label and geometry.
fn element(c: &mut Cursor) -> Result<Element> {
    let Token::Word(word) = c.token()? else {
        return Err(invalid("expected a TIN element type"));
    };
    let kind = ElementKind::from_keyword(&word).ok_or_else(|| invalid(&format!("{word} names no TIN element type")))?;
    let id =
        if c.take_word("ID") { i32::try_from(c.integer(i64::from(i32::MIN), i64::from(i32::MAX))?).unwrap_or_default() } else { 0_i32 };
    let tag = if c.take_word("TAG") {
        let Token::Quoted(tag) = c.token()? else {
            return Err(invalid("expected a double-quoted tag"));
        };
        tag
    } else {
        String::new()
    };
    let body = representation(c)?;
    if body.kind() != element_kind_of(kind) {
        return Err(invalid(&format!("a {word} element holds a {}", element_kind_of(kind).keyword())));
    }
    Ok(Element { kind, id, tag, body })
}

#[cfg(test)]
mod tests;
