//! \[DESIGN\] The planar computational-geometry engine under SQL/MM
//! Spatial's relationships and operations, ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §4.2.2 and §5.1. It works on binary64 XY coordinates and
//! knows no SQL type. Callers hand it points, line strings and polygons
//! and read back the same shapes. Predicates are exact; operands are
//! noded together by snap rounding and labelled in one planar graph,
//! from which the DE-9IM and the point-set operations are read.

mod assemble;
mod boundary;
mod buffer;
mod chain;
mod distance;
mod dyadic;
mod graph;
mod hull;
mod interval;
mod label;
mod measure;
mod meetings;
mod noding;
mod operand;
mod overlay;
mod polygonize;
mod predicates;
mod predicates3d;
mod relate;
mod ring;
mod segment;
mod shape;
mod simple;
mod valid;

pub use boundary::boundary;
pub use buffer::buffer;
pub use distance::distance;
pub use hull::convex_hull;
pub use label::Location;
pub use measure::{area, centroid, length, perimeter, point_on_surface};
pub use overlay::{Operation, difference, intersection, overlay, sym_difference, union};
pub use polygonize::{multipolygon_from_rings, polygon_from_rings};
pub use predicates::{incircle, orient2d};
pub use predicates3d::{Point3, insphere, orient3d};
pub use relate::{Matrix, Pattern, contains, crosses, disjoint, equals, intersects, overlaps, relate, relate_pattern, touches, within};
pub use shape::{Dimension, Geometry};
pub use simple::is_simple;
pub use valid::{Invalid, Reason, is_valid, validate};

/// A position, x then y.
pub type Point = [f64; 2];

#[cfg(test)]
mod tests;
