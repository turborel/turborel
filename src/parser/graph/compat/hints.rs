//! \[EXTENSION\] [GS §10.2, R16.8] A GRAPH_TABLE's hints, in a bracketed
//! comment whose contents begin with a plus sign, written just after its
//! left parenthesis, as optimizer hints are written inside GRAPH_TABLE:
//!
//! ```text
//! <graph hints> ::= /*+ <graph hint> [ { [ , ] <graph hint> }... ] */
//! <graph hint> ::= GRAPH_START ( <node variable> )
//!     | GRAPH_DIRECTION ( <node variable> , REVERSE )
//!     | NO_WCOJ | USE_ADJACENCY ( <table name> )
//!     | PATH_ALGO ( BIBFS | DFS | DIJKSTRA )
//! ```
//!
//! A word of the comment that names no hint is ignored, as a comment's
//! words are. A hint never changes what a match gives.

use crate::ast::{GraphHint, GraphTable, PathAlgorithm};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim};
use crate::parser::{Declared, Parser, exposure};

/// The opening of a hints comment.
const OPENING: &str = "/*+";
/// The closing of a bracketed comment.
const CLOSING: &str = "*/";
/// GRAPH_START.
const GRAPH_START: &str = "GRAPH_START";
/// GRAPH_DIRECTION.
const GRAPH_DIRECTION: &str = "GRAPH_DIRECTION";
/// REVERSE, of GRAPH_DIRECTION.
const REVERSE: &str = "REVERSE";
/// NO_WCOJ.
const NO_WCOJ: &str = "NO_WCOJ";
/// USE_ADJACENCY.
const USE_ADJACENCY: &str = "USE_ADJACENCY";
/// PATH_ALGO.
const PATH_ALGO: &str = "PATH_ALGO";
/// The algorithms PATH_ALGO names, by name.
const ALGORITHMS: [(&str, PathAlgorithm); 3] =
    [("BIBFS", PathAlgorithm::BiBfs), ("DFS", PathAlgorithm::Dfs), ("DIJKSTRA", PathAlgorithm::Dijkstra)];

impl Parser {
    /// The hints written between the token before this one and this one,
    /// if a hints comment is there.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if a hint does not conform to its Format.
    pub(in crate::parser) fn graph_hints(&self) -> Result<Vec<GraphHint>> {
        let from = self.pos.checked_sub(1).and_then(|p| self.toks.get(p)).map_or(0, |t| t.end);
        let gap: String = self.src.get(from..self.here()).unwrap_or_default().iter().collect();
        let Some(start) = gap.find(OPENING) else { return Ok(Vec::new()) };
        let body = gap.get(start.saturating_add(OPENING.len())..).unwrap_or_default();
        let body = body.get(..body.find(CLOSING).unwrap_or(body.len())).unwrap_or_default();
        let offset = from.saturating_add(gap.get(..start).map_or(0, |s| s.chars().count())).saturating_add(OPENING.len());
        // Placed where the comment's contents begin, so its errors are
        // placed in the statement as written.
        let placed = format!("{}{body}", " ".repeat(offset));
        Parser::new(&placed)?.hint_list()
    }

    /// The hints of a hints comment's contents, as a statement of its own.
    fn hint_list(&mut self) -> Result<Vec<GraphHint>> {
        let mut hints = Vec::new();
        while *self.peek() != Tok::End {
            if let Some(hint) = self.graph_hint()? {
                hints.push(hint);
            }
            self.accept_delim(delim::COMMA);
        }
        Ok(hints)
    }

    /// The hint here, or none of a word that names none, which is passed.
    fn graph_hint(&mut self) -> Result<Option<GraphHint>> {
        let word = if let Tok::Identifier(w) = self.peek() { w.clone() } else { String::new() };
        self.advance();
        if word == NO_WCOJ {
            return Ok(Some(GraphHint::NoWcoj));
        }
        let named = [GRAPH_START, GRAPH_DIRECTION, USE_ADJACENCY, PATH_ALGO].contains(&word.as_str());
        if !named {
            if self.is_delim(delim::LEFT_PAREN) {
                let past = self.past_closing(0).unwrap_or(0);
                for _ in 0..past {
                    self.advance();
                }
            }
            return Ok(None);
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let hint = if word == USE_ADJACENCY {
            GraphHint::UseAdjacency(self.table_name()?)
        } else if word == PATH_ALGO {
            let name = self.identifier()?;
            let found = ALGORITHMS.iter().find(|(n, _)| *n == name);
            GraphHint::PathAlgo(found.map(|(_, a)| *a).ok_or_else(|| self.syntax("[GS R16.8] expected BIBFS, DFS or DIJKSTRA"))?)
        } else if word == GRAPH_START {
            GraphHint::Start(self.identifier()?)
        } else {
            let node = self.identifier()?;
            self.expect_delim(delim::COMMA)?;
            self.expect_word(REVERSE)?;
            GraphHint::Reverse(node)
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(hint))
    }
}

/// `table`'s hints, each node variable named one of its pattern.
///
/// # Errors
///
/// SQLCODE -103 if GRAPH_START or GRAPH_DIRECTION names no node variable.
pub(in crate::parser) fn checked_hints(table: &GraphTable) -> Result<()> {
    let exposed = exposure(&table.pattern)?;
    for hint in &table.hints {
        let node = match hint {
            GraphHint::Start(n) | GraphHint::Reverse(n) => n,
            GraphHint::NoWcoj | GraphHint::UseAdjacency(_) | GraphHint::PathAlgo(_) => continue,
        };
        if exposed.get(node).is_none_or(|e| e.kind != Declared::Vertex) {
            let msg = format!("[GS R16.8] the hint names {node}, no node variable of the pattern");
            return Err(SqlError::at(Code::SyntaxRule, table.at, msg));
        }
    }
    Ok(())
}
