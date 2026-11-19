//! \[EXTENSION\] [GS §10.2, R16.8] A GRAPH_TABLE's hints written as
//! optimizer hints are written inside GRAPH_TABLE: in a bracketed comment
//! whose contents begin with a plus sign, just after its left
//! parenthesis.
//!
//! ```text
//! <graph hints comment> ::= /*+ <graph hint> [ { [ , ] <graph hint> }... ] */
//! ```
//!
//! Each `<graph hint>` is one OPTIONS takes (`graph/hints.rs`), read by
//! the same rules, and checked as those are. A word of the comment that
//! names no hint is passed over with its parenthesized arguments, as a
//! comment's words are. A hint never changes what a match gives.

use crate::ast::GraphHints;
use crate::error::Result;
use crate::lexer::{Tok, delim};
use crate::parser::Parser;

/// The opening of a hints comment.
const OPENING: &str = "/*+";
/// The closing of a bracketed comment.
const CLOSING: &str = "*/";

impl Parser {
    /// The hints written between the token before this one and this one,
    /// if a hints comment is there.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if a hint does not conform to its Format.
    pub(in crate::parser) fn graph_hints(&self) -> Result<GraphHints> {
        let from = self.pos.checked_sub(1).and_then(|p| self.toks.get(p)).map_or(0, |t| t.end);
        let gap: String = self.src.get(from..self.here()).unwrap_or_default().iter().collect();
        let Some(start) = gap.find(OPENING) else { return Ok(GraphHints::default()) };
        let body = gap.get(start.saturating_add(OPENING.len())..).unwrap_or_default();
        let body = body.get(..body.find(CLOSING).unwrap_or(body.len())).unwrap_or_default();
        let offset = from.saturating_add(gap.get(..start).map_or(0, |s| s.chars().count())).saturating_add(OPENING.len());
        // Placed where the comment's contents begin, so its errors are
        // placed in the statement as written.
        let placed = format!("{}{body}", " ".repeat(offset));
        Parser::new(&placed)?.hint_list()
    }

    /// The hints of a hints comment's contents, as a statement of its own.
    fn hint_list(&mut self) -> Result<GraphHints> {
        let mut hints = GraphHints::default();
        while *self.peek() != Tok::End {
            if !self.graph_hint(&mut hints)? {
                // A word of no hint, or a hint written twice: passed over.
                let opens = *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN);
                let past = if opens { self.past_closing(1).unwrap_or(1) } else { 1 };
                for _ in 0..past {
                    self.advance();
                }
            }
            self.accept_delim(delim::COMMA);
        }
        Ok(hints)
    }
}

/// [GS R16.8] Adds the hints of a hints comment, `written`, to those of
/// OPTIONS, `hints`: those OPTIONS writes first.
pub(in crate::parser) fn merge_hints(hints: &mut GraphHints, written: GraphHints) {
    hints.start = hints.start.take().or(written.start);
    hints.reverse.extend(written.reverse);
    hints.no_wcoj = hints.no_wcoj || written.no_wcoj;
    hints.use_adjacency.extend(written.use_adjacency);
    hints.path_algo = hints.path_algo.or(written.path_algo);
}
