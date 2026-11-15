//! \[DESIGN\] Closed Cartesian bounds used to prune spatial searches.
//! Bounds describe where a value may be, not its exact geometry. Touching
//! edges and zero-width bounds intersect, so pruning cannot discard a
//! point or a geometry lying on a query's boundary.

use crate::error::{Code, Result, SqlError};

pub(crate) mod partition;

/// The two coordinate axes of Cartesian spatial bounds.
pub const DIMENSIONS: usize = 2;

/// A finite, closed rectangle. The constructor rejects NaN, infinity and
/// reversed bounds before they can enter an index. A point has equal
/// lower and upper coordinates; there is no empty-geometry sentinel.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

/// Finite coordinates and canonical zeros make bit identity the same as
/// numeric identity. No approximate equality belongs in an index key.
impl PartialEq for Bounds {
    fn eq(&self, other: &Self) -> bool {
        self.lower().map(f64::to_bits) == other.lower().map(f64::to_bits)
            && self.upper().map(f64::to_bits) == other.upper().map(f64::to_bits)
    }
}

impl Eq for Bounds {}

impl Bounds {
    /// The rectangle from `lower` to `upper`, both inclusive, in X/Y order.
    /// Signed zeros have one representation, as SQL compares them equal.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a coordinate is not finite or a lower bound exceeds
    /// its upper bound.
    pub fn new(lower: [f64; DIMENSIONS], upper: [f64; DIMENSIONS]) -> Result<Bounds> {
        if !lower.iter().chain(&upper).all(|v| v.is_finite()) {
            return Err(SqlError::new(Code::Api, "[DESIGN] spatial bounds need finite coordinates"));
        }
        let [min_x, min_y] = lower.map(|v| v + 0.0_f64);
        let [max_x, max_y] = upper.map(|v| v + 0.0_f64);
        if min_x > max_x || min_y > max_y {
            return Err(SqlError::new(Code::Api, "[DESIGN] a spatial lower bound exceeds its upper bound"));
        }
        Ok(Bounds { min_x, min_y, max_x, max_y })
    }

    /// The degenerate rectangle at `coordinates`.
    ///
    /// # Errors
    ///
    /// As [`Bounds::new`].
    pub fn point(coordinates: [f64; DIMENSIONS]) -> Result<Bounds> {
        Bounds::new(coordinates, coordinates)
    }

    /// The lower X and Y coordinates.
    pub fn lower(self) -> [f64; DIMENSIONS] {
        [self.min_x, self.min_y]
    }

    /// The upper X and Y coordinates.
    pub fn upper(self) -> [f64; DIMENSIONS] {
        [self.max_x, self.max_y]
    }

    /// Whether the closed rectangles have any point in common.
    pub fn intersects(self, other: Bounds) -> bool {
        self.min_x <= other.max_x && other.min_x <= self.max_x && self.min_y <= other.max_y && other.min_y <= self.max_y
    }

    /// Whether every point of `other` is inside or on this rectangle.
    /// This is rectangle containment, not a geometry's topological
    /// `ST_Contains` relationship.
    pub fn contains(self, other: Bounds) -> bool {
        self.min_x <= other.min_x && self.max_x >= other.max_x && self.min_y <= other.min_y && self.max_y >= other.max_y
    }

    /// The smallest rectangle enclosing both rectangles. It is safe to
    /// publish this before inserting a child: enlargement adds candidates.
    #[must_use]
    pub fn union(self, other: Bounds) -> Bounds {
        Bounds {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Their common rectangle, including an edge or point, or none if
    /// they are disjoint.
    pub fn intersection(self, other: Bounds) -> Option<Bounds> {
        self.intersects(other).then(|| Bounds {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        })
    }

    /// The Euclidean distance between the rectangles, in coordinate
    /// units. A distance beyond the finite range of `f64` is infinity.
    pub fn distance(self, other: Bounds) -> f64 {
        let x = (self.min_x - other.max_x).max(other.min_x - self.max_x).max(0.0_f64);
        let y = (self.min_y - other.max_y).max(other.min_y - self.max_y).max(0.0_f64);
        x.hypot(y)
    }

    /// A lower bound on Euclidean distance, for best-first traversal.
    /// Each positive arithmetic result is rounded down. Scaling keeps
    /// squares finite even near the limits of the coordinate type.
    pub fn distance_lower_bound(self, other: Bounds) -> f64 {
        let x = down((self.min_x - other.max_x).max(other.min_x - self.max_x));
        let y = down((self.min_y - other.max_y).max(other.min_y - self.max_y));
        let scale = x.max(y);
        if scale <= 0.0_f64 {
            return 0.0_f64;
        }
        let (x, y) = (down(x / scale), down(y / scale));
        let squared = down(down(x * x) + down(y * y));
        down(down(squared.sqrt()) * scale)
    }
}

/// The preceding representable nonnegative value; overflow becomes the
/// largest finite value, which is still a lower bound on a larger result.
fn down(value: f64) -> f64 {
    value.next_down().max(0.0_f64)
}

#[cfg(test)]
mod tests;
