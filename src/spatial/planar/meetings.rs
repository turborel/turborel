//! \[DESIGN\] Every meeting of the segments and points of a set of
//! polylines, classified exactly and without noding. Simplicity and
//! validity are decided from them.

use super::Point;
use super::chain::{Candidate, Piece, candidates};
use super::segment::{Meeting, meet, on_segment, same};

/// Two pieces that meet, and how.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub(crate) first: Piece,
    pub(crate) second: Piece,
    pub(crate) meeting: Meeting,
}

fn piece(lines: &[Vec<Point>], piece: Piece) -> (Point, Option<Point>) {
    let vertices = lines.get(piece.line).map_or(&[][..], Vec::as_slice);
    (vertices.get(piece.index).copied().unwrap_or_default(), vertices.get(piece.index.saturating_add(1)).copied())
}

/// Every pair of pieces that meet, each pair once, the lower piece
/// first. Polylines must have no repeated consecutive positions.
pub(crate) fn hits(lines: &[Vec<Point>]) -> Vec<Hit> {
    let mut found = Vec::new();
    for candidate in candidates(lines, &[]) {
        let Candidate::Pair(left, right) = candidate else { continue };
        let (first, second) = if left <= right { (left, right) } else { (right, left) };
        let meeting = match (piece(lines, first), piece(lines, second)) {
            ((a, Some(b)), (c, Some(d))) => meet(a, b, c, d),
            ((a, Some(b)), (point, None)) | ((point, None), (a, Some(b))) => touching(point, on_segment(point, a, b)),
            ((point, None), (other, None)) => touching(point, same(point, other)),
        };
        if meeting != Meeting::Apart {
            found.push(Hit { first, second, meeting });
        }
    }
    found
}

fn touching(point: Point, meets: bool) -> Meeting {
    if meets { Meeting::Touch(point) } else { Meeting::Apart }
}
