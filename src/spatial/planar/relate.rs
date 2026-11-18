//! The Dimensionally Extended 9 Intersection Model and the spatial
//! relationships defined on it [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §4.2.2, §5.1.43 to §5.1.54]. The operands are noded into one graph;
//! each node, edge and face is located in both, and each adds its own
//! dimension to the cell of its two locations. The graph partitions the
//! plane, so each cell's value is the greatest dimension met.

use super::graph::{Graph, INPUTS};
use super::label::{self, Location};
use super::operand::paths;
use super::shape::{Dimension, Geometry};
use crate::error::{Code, Result, SqlError};

/// Rows and columns, for interior, boundary and exterior [§4.2.2.1
/// Table 5].
const SIDES: usize = 3;
const CELLS: usize = 9;
const LOCATIONS: [Location; SIDES] = [Location::Interior, Location::Boundary, Location::Exterior];

/// The pattern values of §4.2.2.1 a) to f) and Table 13.
const TRUE: char = 'T';
const FALSE: char = 'F';
const POINT: char = '0';
const CURVE: char = '1';
const SURFACE: char = '2';
const ANY: char = '*';

/// The DE-9IM of two geometries [§4.2.2.1 Table 5], the dimension of
/// the intersection of each of the first's interior, boundary and
/// exterior with each of the second's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Matrix {
    cells: [[Dimension; SIDES]; SIDES],
}

impl Matrix {
    fn new() -> Self {
        Self { cells: [[Dimension::Empty; SIDES]; SIDES] }
    }

    fn raise(&mut self, first: Location, second: Location, dimension: Dimension) {
        if let Some(cell) = self.cells.get_mut(first.index()).and_then(|row| row.get_mut(second.index()))
            && *cell < dimension
        {
            *cell = dimension;
        }
    }

    /// The dimension of the intersection of the first geometry's `first`
    /// part with the second's `second` part.
    pub fn get(&self, first: Location, second: Location) -> Dimension {
        self.cells.get(first.index()).and_then(|row| row.get(second.index())).copied().unwrap_or(Dimension::Empty)
    }

    /// The matrix of the operands in the other order.
    #[must_use]
    pub fn transposed(&self) -> Matrix {
        let mut out = Matrix::new();
        for first in LOCATIONS {
            for second in LOCATIONS {
                out.raise(second, first, self.get(first, second));
            }
        }
        out
    }

    /// The nine cells in row-major order as `F`, `0`, `1` and `2`.
    pub fn code(&self) -> String {
        let symbol = |dimension: Dimension| match dimension {
            Dimension::Empty => FALSE,
            Dimension::Point => POINT,
            Dimension::Curve => CURVE,
            Dimension::Surface => SURFACE,
        };
        self.cells.iter().flatten().map(|cell| symbol(*cell)).collect()
    }

    /// Whether every cell is in the set its pattern value allows
    /// [§5.1.45 Description 2d, Table 13].
    pub fn matches(&self, pattern: &Pattern) -> bool {
        self.cells.iter().flatten().zip(pattern.cells).all(|(cell, value)| allows(value, *cell))
    }

    fn holds(&self, pattern: &str) -> bool {
        Pattern::parse(pattern).is_ok_and(|pattern| self.matches(&pattern))
    }
}

fn allows(value: char, dimension: Dimension) -> bool {
    match value {
        TRUE => dimension != Dimension::Empty,
        FALSE => dimension == Dimension::Empty,
        POINT => dimension == Dimension::Point,
        CURVE => dimension == Dimension::Curve,
        SURFACE => dimension == Dimension::Surface,
        _ => true,
    }
}

/// An intersection pattern matrix [§4.2.2.1, §5.1.45 Table 12].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    cells: [char; CELLS],
}

impl Pattern {
    /// The pattern a nine-character string spells in row-major order.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if the string is not nine characters each `T`, `F`,
    /// `0`, `1`, `2` or `*` [§5.1.45 Description 2c].
    pub fn parse(text: &str) -> Result<Pattern> {
        let mut cells = [ANY; CELLS];
        let mut characters = text.chars();
        for cell in &mut cells {
            match characters.next() {
                Some(value @ (TRUE | FALSE | POINT | CURVE | SURFACE | ANY)) => *cell = value,
                Some(_) | None => return Err(invalid()),
            }
        }
        if characters.next().is_some() {
            return Err(invalid());
        }
        Ok(Pattern { cells })
    }
}

fn invalid() -> SqlError {
    SqlError::new(Code::Data, "[ISO/IEC 13249-3:2016 §5.1.45 Description 2c] invalid intersection matrix")
}

/// The DE-9IM of two geometries, or nothing if either is empty, for
/// which ST_Relate returns the null value [§5.1.45 Description 2a, 2b].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn relate(first: &Geometry, second: &Geometry) -> Result<Option<Matrix>> {
    let (one, two) = (first.parts(), second.parts());
    if one.is_empty() || two.is_empty() {
        return Ok(None);
    }
    let mut all = paths(&one, 0);
    all.extend(paths(&two, 1));
    Ok(Some(matrix(&Graph::build(&all)?)))
}

/// Every node, edge and face adds its dimension to the cell of its
/// locations; the unbounded face lies in both exteriors.
pub(crate) fn matrix(graph: &Graph) -> Matrix {
    let mut matrix = Matrix::new();
    matrix.raise(Location::Exterior, Location::Exterior, Dimension::Surface);
    for cycle in graph.cycles.iter().filter(|cycle| !cycle.outer) {
        let [first, second] = cycle.depth;
        matrix.raise(label::face(first), label::face(second), Dimension::Surface);
    }
    for id in 0..graph.edges.len() {
        matrix.raise(label::edge(graph, id, 0), label::edge(graph, id, INPUTS.saturating_sub(1)), Dimension::Curve);
    }
    for id in 0..graph.nodes.len() {
        matrix.raise(label::node(graph, id, 0), label::node(graph, id, INPUTS.saturating_sub(1)), Dimension::Point);
    }
    matrix
}

/// ST_Relate, whether the DE-9IM matches the pattern, or the null value
/// if either geometry is empty [§5.1.45].
///
/// # Errors
///
/// SQLCODE -301 for an invalid pattern, checked first; -910 if noding
/// does not settle.
pub fn relate_pattern(first: &Geometry, second: &Geometry, pattern: &str) -> Result<Option<bool>> {
    let pattern = Pattern::parse(pattern)?;
    Ok(relate(first, second)?.map(|matrix| matrix.matches(&pattern)))
}

fn holds(first: &Geometry, second: &Geometry, pattern: &str) -> Result<Option<bool>> {
    Ok(relate(first, second)?.map(|matrix| matrix.holds(pattern)))
}

/// ST_Equals, true when the symmetric difference is empty [§5.1.43], so two
/// empty sets are equal and an empty and a nonempty one are not.
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn equals(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    match (first.is_empty(), second.is_empty()) {
        (true, true) => Ok(Some(true)),
        (true, false) | (false, true) => Ok(Some(false)),
        (false, false) => holds(first, second, "**F**FFF*"),
    }
}

/// ST_Disjoint, `'FF*FF****'` [§5.1.46].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn disjoint(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    holds(first, second, "FF*FF****")
}

/// ST_Intersects, not disjoint [§5.1.48].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn intersects(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    Ok(disjoint(first, second)?.map(|disjoint| !disjoint))
}

/// ST_Touches, the null value for two 0-dimensional geometries, else
/// `'FT*******'`, `'F**T*****'` or `'F***T****'` [§5.1.50].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn touches(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    if first.dimension() == Dimension::Point && second.dimension() == Dimension::Point {
        return Ok(None);
    }
    Ok(relate(first, second)?.map(|matrix| matrix.holds("FT*******") || matrix.holds("F**T*****") || matrix.holds("F***T****")))
}

/// ST_Crosses, `'T*T******'` for a point set against a curve or surface
/// and a curve against a surface, `'0********'` for two curves, else the
/// null value [§5.1.51].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn crosses(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    match (first.dimension(), second.dimension()) {
        (Dimension::Point, Dimension::Curve | Dimension::Surface) | (Dimension::Curve, Dimension::Surface) => {
            holds(first, second, "T*T******")
        }
        (Dimension::Curve, Dimension::Curve) => holds(first, second, "0********"),
        (
            Dimension::Empty | Dimension::Point | Dimension::Curve | Dimension::Surface,
            Dimension::Empty | Dimension::Point | Dimension::Curve | Dimension::Surface,
        ) => Ok(None),
    }
}

/// ST_Within, `'T*F**F***'` [§5.1.52].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn within(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    holds(first, second, "T*F**F***")
}

/// ST_Contains, true when the second is within the first [§5.1.53].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn contains(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    within(second, first)
}

/// ST_Overlaps, `'T*T***T**'` for two point sets or two surfaces,
/// `'1*T***T**'` for two curves, else the null value [§5.1.54].
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle.
pub fn overlaps(first: &Geometry, second: &Geometry) -> Result<Option<bool>> {
    match (first.dimension(), second.dimension()) {
        (Dimension::Point, Dimension::Point) | (Dimension::Surface, Dimension::Surface) => holds(first, second, "T*T***T**"),
        (Dimension::Curve, Dimension::Curve) => holds(first, second, "1*T***T**"),
        (
            Dimension::Empty | Dimension::Point | Dimension::Curve | Dimension::Surface,
            Dimension::Empty | Dimension::Point | Dimension::Curve | Dimension::Surface,
        ) => Ok(None),
    }
}
