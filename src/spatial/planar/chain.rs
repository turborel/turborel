//! \[DESIGN\] Monotone-chain indexing of polylines. A monotone chain is a
//! maximal run of segments heading into one closed quadrant, so its
//! envelope is the box of its end vertices and two of its segments meet
//! only where they are adjacent. A sweep over chain and pixel envelopes
//! finds overlapping pairs; bisection inside a pair finds the segments.

use super::Point;
use super::segment::compare;

/// A segment, or a lone vertex for a one-point polyline. `index` is the
/// segment's first vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Piece {
    pub(crate) line: usize,
    pub(crate) index: usize,
}

/// A candidate found by the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Candidate {
    /// Two pieces whose envelopes overlap.
    Pair(Piece, Piece),
    /// A piece whose envelope overlaps a pixel, by the pixel's index.
    Pixel(Piece, usize),
}

/// An envelope, its lower and upper corners.
pub(crate) type Envelope = (Point, Point);

/// A run of vertices `start..=end` of one polyline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Chain {
    pub(crate) line: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Chain {
    /// The run's envelope, exact for a monotone run.
    pub(crate) fn envelope(self, lines: &[Vec<Point>]) -> Envelope {
        envelope_of(lines, self.line, self.start, self.end)
    }

    /// Whether the run is a single segment or a lone vertex.
    pub(crate) fn single(self) -> bool {
        self.end.saturating_sub(self.start) <= 1
    }
}

enum Item {
    Chain(Chain),
    Pixel(usize),
}

/// The quadrant a segment heads into, zero components counting as
/// positive so a run along an axis stays in one chain.
fn quadrant(a: Point, b: Point) -> (bool, bool) {
    let [ax, ay] = a;
    let [bx, by] = b;
    (bx >= ax, by >= ay)
}

/// The monotone chains of the polylines; a one-point polyline is a
/// chain of its lone vertex.
pub(crate) fn chains(lines: &[Vec<Point>]) -> Vec<Chain> {
    let mut chains = Vec::new();
    for (line, vertices) in lines.iter().enumerate() {
        if vertices.len() == 1 {
            chains.push(Chain { line, start: 0, end: 0 });
            continue;
        }
        let mut start = 0_usize;
        let mut heading = None;
        for (index, pair) in vertices.windows(2).enumerate() {
            let [a, b] = [pair.first(), pair.last()].map(|point| point.copied().unwrap_or_default());
            let next = quadrant(a, b);
            if heading.is_some_and(|current| current != next) {
                chains.push(Chain { line, start, end: index });
                start = index;
            }
            heading = Some(next);
        }
        if heading.is_some() {
            chains.push(Chain { line, start, end: vertices.len().saturating_sub(1) });
        }
    }
    chains
}

fn vertex(lines: &[Vec<Point>], line: usize, index: usize) -> Point {
    lines.get(line).and_then(|vertices| vertices.get(index)).copied().unwrap_or_default()
}

/// The envelope of vertices `start..=end`, exact for a monotone run.
fn envelope_of(lines: &[Vec<Point>], line: usize, start: usize, end: usize) -> Envelope {
    let [ax, ay] = vertex(lines, line, start);
    let [bx, by] = vertex(lines, line, end);
    ([ax.min(bx), ay.min(by)], [ax.max(bx), ay.max(by)])
}

/// The envelope of some points; inverted, so it overlaps nothing, for
/// none.
pub(crate) fn envelope(points: &[Point]) -> Envelope {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in points {
        for ((least, most), value) in low.iter_mut().zip(high.iter_mut()).zip(point) {
            *least = least.min(*value);
            *most = most.max(*value);
        }
    }
    (low, high)
}

/// Whether the first envelope contains the second.
pub(crate) fn encloses((low, high): Envelope, (inner_low, inner_high): Envelope) -> bool {
    let [lx, ly] = low;
    let [hx, hy] = high;
    let [ix, iy] = inner_low;
    let [jx, jy] = inner_high;
    lx <= ix && ly <= iy && hx >= jx && hy >= jy
}

/// Whether two closed envelopes overlap, touching included.
pub(crate) fn overlap((low, high): Envelope, (other_low, other_high): Envelope) -> bool {
    let [lx, ly] = low;
    let [hx, hy] = high;
    let [ox, oy] = other_low;
    let [px, py] = other_high;
    lx <= px && ox <= hx && ly <= py && oy <= hy
}

/// Every pair of pieces of `lines` whose envelopes overlap, and every
/// piece overlapping one of `pixels`. A pair of pieces is reported once.
pub(crate) fn candidates(lines: &[Vec<Point>], pixels: &[Envelope]) -> Vec<Candidate> {
    let mut items: Vec<(Item, Envelope)> = chains(lines).into_iter().map(|chain| (Item::Chain(chain), chain.envelope(lines))).collect();
    items.extend(pixels.iter().enumerate().map(|(index, pixel)| (Item::Pixel(index), *pixel)));
    items.sort_by(|(_, ([left, _], _)), (_, ([right, _], _))| compare(*left, *right));
    let mut found = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    for (position, (item, bounds)) in items.iter().enumerate() {
        let ([low_x, _], _) = *bounds;
        active.retain(|other| items.get(*other).is_some_and(|(_, (_, [high_x, _]))| *high_x >= low_x));
        for other in &active {
            let Some((other_item, other_bounds)) = items.get(*other) else { continue };
            if overlap(*bounds, *other_bounds) {
                refine(lines, [(other_item, *other_bounds), (item, *bounds)], &mut found);
            }
        }
        active.push(position);
    }
    found
}

fn refine(lines: &[Vec<Point>], [(left, left_bounds), (right, right_bounds)]: [(&Item, Envelope); 2], found: &mut Vec<Candidate>) {
    match (left, right) {
        (Item::Chain(first), Item::Chain(second)) => pieces(lines, *first, *second, found),
        (Item::Chain(chain), Item::Pixel(pixel)) => pixel_pieces(lines, *chain, (*pixel, right_bounds), found),
        (Item::Pixel(pixel), Item::Chain(chain)) => pixel_pieces(lines, *chain, (*pixel, left_bounds), found),
        (Item::Pixel(_), Item::Pixel(_)) => (),
    }
}

/// Splits the longer run in two until both are single pieces.
fn pieces(lines: &[Vec<Point>], left: Chain, right: Chain, found: &mut Vec<Candidate>) {
    let mut stack = vec![(left, right)];
    while let Some((a, b)) = stack.pop() {
        if !overlap(a.envelope(lines), b.envelope(lines)) {
            continue;
        }
        let (a_span, b_span) = (a.end.saturating_sub(a.start), b.end.saturating_sub(b.start));
        if a_span <= 1 && b_span <= 1 {
            found.push(Candidate::Pair(Piece { line: a.line, index: a.start }, Piece { line: b.line, index: b.start }));
        } else if a_span >= b_span {
            let (first, second) = halves(a);
            stack.push((first, b));
            stack.push((second, b));
        } else {
            let (first, second) = halves(b);
            stack.push((a, first));
            stack.push((a, second));
        }
    }
}

fn pixel_pieces(lines: &[Vec<Point>], chain: Chain, (pixel, bounds): (usize, Envelope), found: &mut Vec<Candidate>) {
    let mut stack = vec![chain];
    while let Some(run) = stack.pop() {
        if !overlap(run.envelope(lines), bounds) {
            continue;
        }
        if run.end.saturating_sub(run.start) <= 1 {
            found.push(Candidate::Pixel(Piece { line: run.line, index: run.start }, pixel));
        } else {
            let (first, second) = halves(run);
            stack.push(first);
            stack.push(second);
        }
    }
}

/// The run split at its middle vertex.
pub(crate) fn halves(chain: Chain) -> (Chain, Chain) {
    let middle = chain.start.saturating_add(chain.end.saturating_sub(chain.start).div_ceil(2));
    (Chain { end: middle, ..chain }, Chain { start: middle, ..chain })
}
