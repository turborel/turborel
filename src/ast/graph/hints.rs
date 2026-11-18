//! \[EXTENSION\] [GS §16 R16.8] The optimizer hints a GRAPH_TABLE writes in
//! its OPTIONS: where a search starts, which way it walks an edge
//! variable, whether a cyclic pattern may take the worst-case optimal
//! join, which adjacency index an edge table is followed through, and
//! which path search algorithm runs. A hint names a variable or an edge
//! table the binder checks; it changes no result, only the plan.

/// \[GS §16 R16.8\] A path search algorithm PATH_ALGO names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAlgorithm {
    /// BFS: breadth first from each start.
    Bfs,
    /// BIBFS: bidirectional breadth first, of a pair of ends.
    Bibfs,
    /// DFS: depth first, of a restrictor.
    Dfs,
    /// DIJKSTRA: least cost first.
    Dijkstra,
    /// MSBFS: breadth first from many starts at once.
    Msbfs,
}

impl PathAlgorithm {
    /// Each algorithm and the word PATH_ALGO names it by.
    pub const WORDS: [(&'static str, PathAlgorithm); 5] = [
        ("BFS", PathAlgorithm::Bfs),
        ("BIBFS", PathAlgorithm::Bibfs),
        ("DFS", PathAlgorithm::Dfs),
        ("DIJKSTRA", PathAlgorithm::Dijkstra),
        ("MSBFS", PathAlgorithm::Msbfs),
    ];

    /// The word that names it.
    #[must_use]
    pub fn word(self) -> &'static str {
        Self::WORDS.iter().find(|(_, a)| *a == self).map_or("BFS", |(w, _)| w)
    }
}

/// \[GS §16 R16.8\] The hints of a GRAPH_TABLE's OPTIONS, each at most
/// once but USE_ADJACENCY and GRAPH_DIRECTION, once per name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphHints {
    /// GRAPH_START ( `<variable>` ): the element variable a search or a
    /// join starts from, and where it is written.
    pub start: Option<(String, usize)>,
    /// GRAPH_DIRECTION ( `<variable>` , REVERSE ): the edge variables
    /// walked from their destination, each where it is written.
    pub reverse: Vec<(String, usize)>,
    /// NO_WCOJ: a cyclic pattern is joined pairwise, not by the worst-case
    /// optimal join.
    pub no_wcoj: bool,
    /// USE_ADJACENCY ( `<edge table alias>` ): the edge tables followed
    /// through their adjacency index, each where it is written.
    pub use_adjacency: Vec<(String, usize)>,
    /// PATH_ALGO ( `<algorithm>` ): the path search algorithm, which also
    /// keeps a rigid pattern's search from being planned as joins.
    pub path_algo: Option<PathAlgorithm>,
}

impl GraphHints {
    /// Whether none is written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == GraphHints::default()
    }
}
