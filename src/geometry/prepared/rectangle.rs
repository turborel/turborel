//! Recognize a single, nondegenerate rectangular ring. Its envelope is
//! its exact closed point set, so point location needs no ray crossing.
//! Diagonal edges, holes and repeated corners retain ordinary topology.

use crate::geometry::shape::{Point, Shape};

pub(super) fn recognizes(shape: &Shape) -> bool {
    let Shape::Polygon(rings) = shape else { return false };
    let [ring] = rings.as_slice() else { return false };
    let [a, b, c, d, close] = ring.as_slice() else { return false };
    let ([ax, ay], [cx, cy]) = (*a, *c);
    if !same(*a, *close) || ax.to_bits() == cx.to_bits() || ay.to_bits() == cy.to_bits() {
        return false;
    }
    let across = same(*b, [cx, ay]) && same(*d, [ax, cy]);
    let upward = same(*b, [ax, cy]) && same(*d, [cx, ay]);
    across || upward
}

fn same(a: Point, b: Point) -> bool {
    a.map(f64::to_bits) == b.map(f64::to_bits)
}

#[cfg(test)]
mod tests;
