//! \[EXTENSION\] [GS §16 R16.8] The hints of a GRAPH_TABLE's OPTIONS:
//! GRAPH_START ( `<variable>` ), GRAPH_DIRECTION ( `<variable>` , REVERSE ),
//! NO_WCOJ, USE_ADJACENCY ( `<edge table alias>` ) and PATH_ALGO (
//! {BFS | BIBFS | DFS | DIJKSTRA | MSBFS} ), written among its limits.

use super::super::Parser;
use crate::ast::{GraphHints, PathAlgorithm};
use crate::error::{Code, Result, SqlError};
use crate::lexer::delim;

/// GRAPH_START, of the variable a search or a join starts from.
const GRAPH_START: &str = "GRAPH_START";
/// GRAPH_DIRECTION, of an edge variable walked one way.
const GRAPH_DIRECTION: &str = "GRAPH_DIRECTION";
/// REVERSE, of GRAPH_DIRECTION: from the edge's destination.
const REVERSE: &str = "REVERSE";
/// NO_WCOJ, of a cyclic pattern joined pairwise.
const NO_WCOJ: &str = "NO_WCOJ";
/// USE_ADJACENCY, of an edge table followed through its adjacency index.
const USE_ADJACENCY: &str = "USE_ADJACENCY";
/// PATH_ALGO, of the path search algorithm.
const PATH_ALGO: &str = "PATH_ALGO";

impl Parser {
    /// A hint, if one not yet written is next, added to `hints`: whether
    /// one was.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if a hint's arguments are not as written above.
    pub(super) fn graph_hint(&mut self, hints: &mut GraphHints) -> Result<bool> {
        if hints.start.is_none() && self.accept_word(GRAPH_START) {
            hints.start = Some(self.hint_name()?);
            self.expect_delim(delim::RIGHT_PAREN)?;
        } else if self.accept_word(GRAPH_DIRECTION) {
            let name = self.hint_name()?;
            self.expect_delim(delim::COMMA)?;
            self.expect_word(REVERSE)?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            hints.reverse.push(name);
        } else if !hints.no_wcoj && self.accept_word(NO_WCOJ) {
            hints.no_wcoj = true;
        } else if self.accept_word(USE_ADJACENCY) {
            let name = self.hint_name()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            hints.use_adjacency.push(name);
        } else if hints.path_algo.is_none() && self.accept_word(PATH_ALGO) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let at = self.here();
            let word = self.identifier()?;
            let found = PathAlgorithm::WORDS.iter().find(|(w, _)| *w == word).map(|(_, a)| *a);
            let algorithm = found.ok_or_else(|| {
                SqlError::at(Code::Syntax, at, "[EXTENSION] PATH_ALGO names BFS, BIBFS, DFS, DIJKSTRA or MSBFS (GS §16 R16.8)")
            })?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            hints.path_algo = Some(algorithm);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// `(` `<identifier>` of a hint, and where the identifier is written.
    fn hint_name(&mut self) -> Result<(String, usize)> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let at = self.here();
        Ok((self.identifier()?, at))
    }
}
