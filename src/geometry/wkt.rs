//! \[EXTENSION\] The XY WKT of a planar value, OGC 06-103r4 §§7.2.1–7.2.2.
//! Text is read by the model's reader (`spatial::formats::wkt`).

use super::shape::{COLLECTION, EMPTY, Point, Shape};

pub(super) fn write(shape: &Shape) -> String {
    format!("{} {}", shape.name(), body(shape))
}

fn body(shape: &Shape) -> String {
    match shape {
        Shape::Point(point) => list(point.iter().map(point_text)),
        Shape::Line(points) => list(points.iter().map(point_text)),
        Shape::Polygon(rings) => list(rings.iter().map(|ring| list(ring.iter().map(point_text)))),
        Shape::Multi(code, parts) => {
            let tagged = *code == COLLECTION;
            list(parts.iter().map(|part| if tagged { write(part) } else { body(part) }))
        }
    }
}

fn list(parts: impl Iterator<Item = String>) -> String {
    let joined = parts.collect::<Vec<_>>().join(", ");
    if joined.is_empty() { EMPTY.to_owned() } else { format!("({joined})") }
}

fn point_text(point: &Point) -> String {
    let [x, y] = point.map(number_text);
    format!("{x} {y}")
}

fn number_text(number: f64) -> String {
    // Scientific notation keeps subnormals from expanding past MAX_BYTES.
    let ordinary = number.to_string();
    let scientific = format!("{number:e}");
    if ordinary.len() <= scientific.len() { ordinary } else { scientific }
}
