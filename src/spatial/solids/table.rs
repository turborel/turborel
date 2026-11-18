//! \[DESIGN\] The TIN tables of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §8.6.5: a TIN as rows of points, triangles and elements, and a TIN
//! built from such rows. Points are the elements' points and, so every
//! triangle can name its corners, the triangles' corners, without
//! duplicates, sorted by x, then y, then z. A multipoint element is a
//! row of its points' numbers, as Description 5c reads them back.

use std::collections::{BTreeMap, BTreeSet};

use super::fault;
use super::tin::tin_of;
use crate::error::{Result, SqlState};
use crate::geometry::model::{Dims, Geom};
use crate::spatial::curves::tin::ElementKind;
use crate::spatial::curves::types::{Coord, Curve};
use crate::spatial::formats::model::to_geom;
use crate::spatial::formats::tree::{Body, Collection, Element, Geometry, Kind, Surface, Tin, element_kind_of};
use crate::spatial::planar;

/// The item of a point row.
pub const POINT: &str = "point";
/// The item of a triangle row.
pub const TRIANGLE: &str = "triangle";
/// What an element type's item gains for a row of one of its holes.
const HOLE: &str = " hole";

/// A row of a TIN table [§8.6.5 Description 2a].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// What the row holds: a point, a triangle, or an element's type.
    pub item: String,
    /// Its position among the rows of its item.
    pub order: i64,
    /// A point's x, y and, of a TIN with z, z.
    pub point: Option<(f64, f64, Option<f64>)>,
    /// A triangle's points' numbers, increasing.
    pub triangle: Option<[i64; 3]>,
    /// A triangle's visibility: -1 hole, 0 void, 1 visible.
    pub visibility: Option<i64>,
    /// An element's identifier.
    pub id: Option<i64>,
    /// An element's tag.
    pub tag: Option<String>,
    /// An element's points' numbers.
    pub element: Option<Vec<i64>>,
}

/// One-based `at` as a row number.
fn number(at: usize) -> i64 {
    i64::try_from(at.saturating_add(1)).unwrap_or(i64::MAX)
}

/// The order of points: x, then y, then z.
fn by_place(p: &Coord, q: &Coord) -> std::cmp::Ordering {
    p.x.total_cmp(&q.x).then(p.y.total_cmp(&q.y)).then(p.z.total_cmp(&q.z))
}

/// The points of an element's geometry, in order.
fn points_of(body: &Body) -> Vec<Vec<Coord>> {
    match body {
        Body::Collection(Collection::MultiPoint(points)) => vec![points.iter().flatten().copied().collect()],
        Body::Curve(Curve::LineString(points)) => vec![points.clone()],
        Body::Surface(Surface::Polygon(rings)) => rings.clone(),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Vec::new(),
    }
}

/// §8.6.5 ST_TINTable(): the rows of the TIN `g`.
///
/// # Errors
///
/// SQLSTATE 2FF70 if `g` is no TIN; the planar engine's.
pub fn table(g: &Geom) -> Result<Vec<Row>> {
    let tree = tin_of(g)?;
    let Body::Surface(Surface::Tin(tin)) = &tree.body else { return Ok(Vec::new()) };
    let mut points: Vec<Coord> = tin.elements.iter().flat_map(|e| points_of(&e.body)).flatten().collect();
    points.extend(tin.triangles.iter().flat_map(|r| r.iter().take(3)));
    let flat = |p: &Coord| Coord { m: 0.0_f64, z: if tree.dims.has_z() { p.z } else { 0.0_f64 }, ..*p };
    let mut points: Vec<Coord> = points.iter().map(flat).collect();
    points.sort_by(by_place);
    points.dedup_by(|p, q| by_place(p, q).is_eq());
    let index = |p: &Coord| points.binary_search_by(|q| by_place(q, &flat(p))).map_or(0, number);
    let z = |p: &Coord| tree.dims.has_z().then_some(p.z);
    let mut rows: Vec<Row> = points
        .iter()
        .enumerate()
        .map(|(at, p)| Row { item: POINT.to_owned(), order: number(at), point: Some((p.x, p.y, z(p))), ..Row::default() })
        .collect();
    rows.extend(triangle_rows(tin, &index)?);
    rows.extend(element_rows(tin, &index));
    Ok(rows)
}

/// The triangle rows, sorted by their points' numbers [§8.6.5 D3f].
fn triangle_rows(tin: &Tin, index: &dyn Fn(&Coord) -> i64) -> Result<Vec<Row>> {
    let mut all = Vec::with_capacity(tin.triangles.len());
    for ring in &tin.triangles {
        let [a, b, c, ..] = ring.as_slice() else { continue };
        let mut numbers = [index(a), index(b), index(c)];
        numbers.sort_unstable();
        all.push((numbers, visibility([a, b, c], &tin.elements)?));
    }
    all.sort_unstable();
    Ok(all
        .into_iter()
        .enumerate()
        .map(|(at, (numbers, seen))| Row {
            item: TRIANGLE.to_owned(),
            order: number(at),
            triangle: Some(numbers),
            visibility: Some(seen),
            ..Row::default()
        })
        .collect())
}

/// A planar shape of point lists, a polygon or a line.
fn plan(parts: &[Vec<Coord>], area: bool) -> planar::Geometry {
    let flat: Vec<Vec<[f64; 2]>> = parts.iter().map(|r| r.iter().map(|p| [p.x, p.y]).collect()).collect();
    if area {
        return planar::Geometry::Polygon(flat);
    }
    planar::Geometry::Line(flat.into_iter().next().unwrap_or_default())
}

/// The visibility of a triangle [§8.6.5 D3f i 7]: -1 within a hole, else
/// 0 within a void or crossed by a stop line, else 1.
fn visibility(corners: [&Coord; 3], elements: &[Element]) -> Result<i64> {
    let [a, b, c] = corners;
    let triangle = plan(&[vec![*a, *b, *c, *a]], true);
    let mut seen = 1_i64;
    for e in elements {
        let parts = points_of(&e.body);
        let shape = plan(&parts, e.kind.is_area());
        let hidden = match e.kind {
            ElementKind::Hole => planar::within(&triangle, &shape)?.unwrap_or(false).then_some(-1_i64),
            ElementKind::BreakVoid | ElementKind::DrapeVoid | ElementKind::Void => {
                planar::within(&triangle, &shape)?.unwrap_or(false).then_some(0_i64)
            }
            ElementKind::StopLine => planar::crosses(&shape, &triangle)?.unwrap_or(false).then_some(0_i64),
            ElementKind::RandomPoints
            | ElementKind::GroupSpot
            | ElementKind::Boundary
            | ElementKind::Breakline
            | ElementKind::SoftBreak
            | ElementKind::ControlContour => None,
        };
        seen = hidden.map_or(seen, |h| seen.min(h));
    }
    Ok(seen)
}

/// The element rows, a polygon's holes each a row of their own, sorted
/// by item and then points' numbers [§8.6.5 D3g].
fn element_rows(tin: &Tin, index: &dyn Fn(&Coord) -> i64) -> Vec<Row> {
    let mut all: Vec<(String, Vec<i64>, i64, String)> = Vec::new();
    for e in &tin.elements {
        for (at, part) in points_of(&e.body).iter().enumerate() {
            let item = if at == 0 { e.kind.name().to_owned() } else { format!("{}{HOLE}", e.kind.name()) };
            all.push((item, part.iter().map(index).collect(), i64::from(e.id), e.tag.clone()));
        }
    }
    all.sort();
    all.into_iter()
        .enumerate()
        .map(|(at, (item, numbers, id, tag))| Row {
            item,
            order: number(at),
            id: Some(id),
            tag: Some(tag),
            element: Some(numbers),
            ..Row::default()
        })
        .collect()
}

/// An element being read back: its type, identifier, tag and point lists.
type Reading = (ElementKind, i64, String, Vec<Vec<Coord>>);

/// §8.6.5 Description 5: the TIN of `rows`, its sides held to `max_side`,
/// of SRID 0. Rows alike count once.
///
/// # Errors
///
/// SQLSTATE 2FF71 for fewer than three points; 2FF02 for a number that
/// names no point, an element type no row's item names, or a maximum
/// side length not above 0; the model's checks'.
pub fn from_rows(rows: &[Row], max_side: f64) -> Result<Geom> {
    if !max_side.is_finite() || max_side <= 0.0_f64 {
        return Err(fault(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.6.4 Description 4b", "the maximum side length is not above 0"));
    }
    let points: BTreeMap<i64, Coord> = rows
        .iter()
        .filter(|r| r.item == POINT)
        .filter_map(|r| r.point.map(|(x, y, z)| (r.order, Coord { x, y, z: z.unwrap_or_default(), m: 0.0_f64 })))
        .collect();
    if points.len() < 3 {
        return Err(fault(SqlState::SPATIAL_AT_LEAST_THREE_POINTS, "§8.6.5 Description 5a i", "at least 3 points are required"));
    }
    let has_z = rows.iter().filter(|r| r.item == POINT).all(|r| r.point.is_some_and(|(_, _, z)| z.is_some()));
    let at = |n: &i64| points.get(n).copied().ok_or_else(|| invalid(&format!("no point is number {n}")));
    let mut triangles: Vec<[i64; 3]> = rows.iter().filter(|r| r.item == TRIANGLE).filter_map(|r| r.triangle).collect();
    triangles.sort_unstable();
    triangles.dedup();
    let rings = triangles.iter().map(|[a, b, c]| Ok(vec![at(a)?, at(b)?, at(c)?, at(a)?])).collect::<Result<Vec<_>>>()?;
    let elements = read_elements(rows, &at)?;
    let tin = Tin { triangles: rings, elements, max_side: Some(max_side) };
    to_geom(&Geometry { dims: if has_z { Dims::Xyz } else { Dims::Xy }, body: Body::Surface(Surface::Tin(tin)) }, 0)
}

fn invalid(detail: &str) -> crate::error::SqlError {
    fault(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.6.5 Description 5", detail)
}

/// The elements of `rows` in order, each hole row a ring of the element
/// of its type, identifier and tag.
fn read_elements(rows: &[Row], at: &dyn Fn(&i64) -> Result<Coord>) -> Result<Vec<Element>> {
    let mut read: BTreeSet<(i64, String, i64, String, Vec<i64>)> = BTreeSet::new();
    for r in rows {
        if let Some(numbers) = &r.element {
            read.insert((r.order, r.item.clone(), r.id.unwrap_or_default(), r.tag.clone().unwrap_or_default(), numbers.clone()));
        }
    }
    let mut out: Vec<Reading> = Vec::new();
    let (holes, outers): (Vec<_>, Vec<_>) = read.into_iter().partition(|k| k.1.ends_with(HOLE));
    for (_, item, id, tag, numbers) in outers {
        let kind = ElementKind::from_name(&item).ok_or_else(|| invalid(&format!("{item} is no TIN element type")))?;
        out.push((kind, id, tag, vec![numbers.iter().map(at).collect::<Result<_>>()?]));
    }
    for (_, item, id, tag, numbers) in holes {
        let name = item.strip_suffix(HOLE).unwrap_or_default();
        let owner = out.iter_mut().find(|(k, i, t, _)| k.name().eq_ignore_ascii_case(name.trim()) && k.is_area() && *i == id && *t == tag);
        let Some((_, _, _, parts)) = owner else { return Err(invalid(&format!("no {name} element holds a hole of {id}"))) };
        parts.push(numbers.iter().map(at).collect::<Result<_>>()?);
    }
    out.into_iter().map(element).collect()
}

/// The element `reading` describes.
fn element((kind, id, tag, mut parts): Reading) -> Result<Element> {
    let id = i32::try_from(id).map_err(|_| invalid("an element identifier is out of range"))?;
    let shape = element_kind_of(kind);
    let body = if shape == Kind::MultiPoint {
        Body::Collection(Collection::MultiPoint(parts.into_iter().flatten().map(Some).collect()))
    } else if shape == Kind::Polygon {
        Body::Surface(Surface::Polygon(parts))
    } else {
        Body::Curve(Curve::LineString(parts.pop().unwrap_or_default()))
    };
    Ok(Element { kind, id, tag, body })
}
