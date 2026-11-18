//! \[DESIGN\] Noding by snap rounding to the binary64 grid (Hobby, 1999).
//! Polylines are split wherever they meet, so that afterwards two
//! segments meet only at a shared vertex. Every vertex and every rounded
//! proper crossing is a hot pixel, the box of reals nearer its centre
//! than any other representable position, so pixels tile the plane.
//! Each input segment is routed through the centre of every hot pixel it
//! meets, in order along it. Routed segments then meet only at pixel
//! centres, which are exact touches and collinear overlaps, split
//! without moving anything. Should a routed segment still cross another,
//! its rounded crossing joins the hot pixels and the input is routed
//! again. Input vertices are pixel centres and never move; any other
//! position moves by less than one unit in the last place.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::Point;
use super::chain::{Candidate, Piece, candidates};
use super::segment::{Meeting, compare, crossing, lexicographic, meet, meets_pixel, on_segment, pixel, same};
use crate::error::{Code, Result, SqlError};

/// Rounds of routing before noding gives up. Routed segments cross
/// only in pathological inputs; ordinary ones settle in two rounds.
const MAX_ROUNDS: usize = 64;

/// The split points found for each segment, by polyline and first vertex.
type Splits = HashMap<(usize, usize), Vec<Point>>;

/// Removes repeated consecutive positions, which add no point to a
/// polyline's point set.
pub(crate) fn deduplicated(line: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(line.len());
    for point in line {
        if out.last().is_none_or(|last| !same(*last, *point)) {
            out.push(*point);
        }
    }
    out
}

/// The polylines, split so any two of their segments meet only at a
/// shared vertex. A one-point polyline stays a point; the others keep
/// their order, ends and closure.
///
/// # Errors
///
/// SQLCODE -910 if rounding has not settled after its bound.
pub(crate) fn node(input: &[Vec<Point>]) -> Result<Vec<Vec<Point>>> {
    let lines: Vec<Vec<Point>> = input.iter().map(|line| deduplicated(line)).collect();
    let mut pixels: Vec<Point> = lines.iter().flatten().copied().collect();
    let mut routed = lines.clone();
    for _ in 0..MAX_ROUNDS {
        let (splits, crossings) = splits(&routed);
        if crossings.is_empty() {
            return Ok(apply(&routed, &splits));
        }
        pixels.extend(crossings);
        pixels.sort_by(|left, right| lexicographic(*left, *right));
        pixels.dedup_by(|left, right| same(*left, *right));
        routed = apply(&lines, &route(&lines, &pixels));
    }
    Err(SqlError::new(Code::Api, "[DESIGN] snap rounding did not settle"))
}

fn piece(lines: &[Vec<Point>], piece: Piece) -> (Point, Option<Point>) {
    let vertices = lines.get(piece.line).map_or(&[][..], Vec::as_slice);
    let start = vertices.get(piece.index).copied().unwrap_or_default();
    (start, vertices.get(piece.index.saturating_add(1)).copied())
}

/// The centres of the hot pixels each segment meets.
fn route(lines: &[Vec<Point>], pixels: &[Point]) -> Splits {
    let boxes: Vec<_> = pixels.iter().copied().map(pixel).collect();
    let mut splits = Splits::new();
    for candidate in candidates(lines, &boxes) {
        let Candidate::Pixel(segment, index) = candidate else { continue };
        let (a, b) = piece(lines, segment);
        let (Some(b), Some(centre)) = (b, pixels.get(index).copied()) else { continue };
        if meets_pixel(a, b, centre) {
            splits.entry((segment.line, segment.index)).or_default().push(centre);
        }
    }
    splits
}

/// Every exact split point of the segments, and the rounded positions
/// of their proper crossings.
fn splits(lines: &[Vec<Point>]) -> (Splits, Vec<Point>) {
    let mut splits = Splits::new();
    let mut crossings = Vec::new();
    for candidate in candidates(lines, &[]) {
        if let Candidate::Pair(first, second) = candidate {
            pair(lines, (first, second), &mut splits, &mut crossings);
        }
    }
    (splits, crossings)
}

fn pair(lines: &[Vec<Point>], (first, second): (Piece, Piece), splits: &mut Splits, crossings: &mut Vec<Point>) {
    let (a, b) = piece(lines, first);
    let (c, d) = piece(lines, second);
    let mut split = |piece: Piece, point: Point| splits.entry((piece.line, piece.index)).or_default().push(point);
    match (b, d) {
        (None, None) => (),
        (Some(b), None) => {
            if on_segment(c, a, b) {
                split(first, c);
            }
        }
        (None, Some(d)) => {
            if on_segment(a, c, d) {
                split(second, a);
            }
        }
        (Some(b), Some(d)) => match meet(a, b, c, d) {
            Meeting::Apart => (),
            Meeting::Touch(point) => {
                split(first, point);
                split(second, point);
            }
            Meeting::Overlap(low, high) => {
                for point in [low, high] {
                    split(first, point);
                    split(second, point);
                }
            }
            Meeting::Cross => {
                let point = crossing(a, b, c, d);
                split(first, point);
                split(second, point);
                crossings.push(point);
            }
        },
    }
}

/// Rebuilds each polyline with its segments' split points in order.
fn apply(lines: &[Vec<Point>], splits: &Splits) -> Vec<Vec<Point>> {
    let mut out = Vec::with_capacity(lines.len());
    for (line, vertices) in lines.iter().enumerate() {
        let mut built: Vec<Point> = Vec::with_capacity(vertices.len());
        for (index, pair) in vertices.windows(2).enumerate() {
            let (Some(a), Some(b)) = (pair.first().copied(), pair.last().copied()) else { continue };
            built.push(a);
            if let Some(points) = splits.get(&(line, index)) {
                let mut points: Vec<Point> = points.iter().copied().filter(|point| !same(*point, a) && !same(*point, b)).collect();
                points.sort_by(|left, right| along(a, b, *left, *right));
                built.extend(points);
            }
        }
        built.extend(vertices.last().copied());
        out.push(deduplicated(&built));
    }
    out
}

/// The order of two points along the segment from `a` to `b`, by the
/// coordinate on its longer axis, the other breaking ties.
fn along([ax, ay]: Point, [bx, by]: Point, [lx, ly]: Point, [rx, ry]: Point) -> Ordering {
    let forward = |order: Ordering, ascending: bool| if ascending { order } else { order.reverse() };
    let by_x = forward(compare(lx, rx), bx >= ax);
    let by_y = forward(compare(ly, ry), by >= ay);
    if (bx - ax).abs() >= (by - ay).abs() { by_x.then(by_y) } else { by_y.then(by_x) }
}

#[cfg(test)]
mod tests;
