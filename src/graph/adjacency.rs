//! The adjacency an index keeps of a graph's edges, read by path search.
use crate::error::Result;
use crate::index::Key;
use crate::storage::RowId;

/// Which way an edge is followed from a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From its source to its destination.
    Out,
    /// From its destination to its source.
    In,
}

/// One edge followed from a vertex: the edge's row and the other endpoint's key.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The edge's row in its table.
    pub edge: RowId,
    /// The key of the vertex at the edge's other end.
    pub vertex: Key,
}

/// The edges of one edge table, by endpoint.
pub trait Adjacency: Send + Sync {
    /// Pushes onto `out` each edge leaving (`Out`) or entering (`In`) the vertex of key `vertex`.
    /// Candidates may include versions the reader cannot see; the caller rechecks each edge row.
    ///
    /// # Errors
    ///
    /// The index's storage fails, or `vertex` is not a key of the vertex's columns.
    fn steps(&self, vertex: &Key, direction: Direction, out: &mut Vec<Step>) -> Result<()>;
    /// Pushes onto `out`, for each of `vertices` in order, its steps as
    /// `steps` gives them: a frontier's lists read at once, which an index
    /// may read in one pass over what it holds.
    ///
    /// # Errors
    ///
    /// As `steps`.
    fn steps_many(&self, vertices: &[Key], direction: Direction, out: &mut Vec<Vec<Step>>) -> Result<()> {
        for vertex in vertices {
            let mut steps = Vec::new();
            self.steps(vertex, direction, &mut steps)?;
            out.push(steps);
        }
        Ok(())
    }
    /// The number of entries for `vertex` in `direction`, an estimate under concurrent writes.
    ///
    /// # Errors
    ///
    /// As `steps`.
    fn degree(&self, vertex: &Key, direction: Direction) -> Result<u64>;
    /// The number of edge entries held, for the planner.
    fn edges(&self) -> u64;
}
