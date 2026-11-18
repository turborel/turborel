//! \[EXTENSION\] What the vector design document adds to a `<fetch first
//! clause>` (§9.1, §9.2, R13.4), in the standard's notation:
//!
//! `<fetch first clause>` ::= FETCH \[`<search mode>`\] {FIRST | NEXT}
//! \[`<fetch first quantity>`\] {ROW | ROWS} {ONLY | WITH TIES}
//! \[`<target accuracy clause>`\] \[`<vector hints>`\]
//!
//! `<search mode>` ::= APPROX | APPROXIMATE | EXACT
//!
//! `<target accuracy clause>` ::= WITH TARGET ACCURACY `<unsigned integer>`
//! | WITH TARGET ACCURACY PARAMETERS ( `<search parameter>` \[{,
//! `<search parameter>`}...\] )
//!
//! `<search parameter>` ::= EFSEARCH `<unsigned integer>` | NEIGHBOR
//! PARTITION PROBES `<unsigned integer>` | SEARCH_LIST `<unsigned
//! integer>` | BEAM_WIDTH `<unsigned integer>`
//!
//! `<vector hints>` ::= HINT ( `<vector hint>` \[{, `<vector hint>`}...\] )
//!
//! `<vector hint>` ::= VECTOR_INDEX ( `<identifier>` `<identifier>` ) |
//! NO_VECTOR_INDEX ( `<identifier>` ) | VECTOR_PREFILTER |
//! VECTOR_INFILTER | VECTOR_POSTFILTER | VECTOR_WAIT_FRESH
//!
//! None of these words is reserved, and each is read as one only here:
//! the standard has FIRST or NEXT after FETCH, and WITH after ONLY only
//! as a view's WITH CHECK OPTION, which TARGET never begins.

use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::logical::{Hints, Search, SearchTarget, Strategy};
use crate::parser::Parser;

// ---------------------------------------------------------------------------
// The words, from the vector design document §9.2 and R13.4
// ---------------------------------------------------------------------------

/// The `<search mode>` of an approximate search.
const APPROX: &str = "APPROX";
/// Its longer spelling.
const APPROXIMATE: &str = "APPROXIMATE";
/// The `<search mode>` of an exact search.
const EXACT: &str = "EXACT";
/// TARGET, after WITH.
const TARGET: &str = "TARGET";
/// ACCURACY, after TARGET.
const ACCURACY: &str = "ACCURACY";
/// PARAMETERS, after ACCURACY.
const PARAMETERS: &str = "PARAMETERS";
/// HNSW's search list, Oracle's name.
const EFSEARCH: &str = "EFSEARCH";
/// NEIGHBOR, of NEIGHBOR PARTITION PROBES.
const NEIGHBOR: &str = "NEIGHBOR";
/// PARTITION, of NEIGHBOR PARTITION PROBES.
const PARTITION: &str = "PARTITION";
/// PROBES, of NEIGHBOR PARTITION PROBES.
const PROBES: &str = "PROBES";
/// DiskANN's search list.
const SEARCH_LIST: &str = "SEARCH_LIST";
/// DiskANN's beam width.
const BEAM_WIDTH: &str = "BEAM_WIDTH";
/// The word that begins `<vector hints>`.
const HINT: &str = "HINT";
/// The hint naming the index of a table.
const VECTOR_INDEX: &str = "VECTOR_INDEX";
/// The hint naming a table read by no approximate index.
const NO_VECTOR_INDEX: &str = "NO_VECTOR_INDEX";
/// The hint of the pre-filter strategy.
const VECTOR_PREFILTER: &str = "VECTOR_PREFILTER";
/// The hint of the in-filter strategy.
const VECTOR_INFILTER: &str = "VECTOR_INFILTER";
/// The hint of the post-filter strategy.
const VECTOR_POSTFILTER: &str = "VECTOR_POSTFILTER";
/// The hint that the read sees every committed change.
const VECTOR_WAIT_FRESH: &str = "VECTOR_WAIT_FRESH";
/// [R9.4] The greatest target accuracy, a percentage.
const MOST_ACCURACY: u32 = 100;

impl Parser {
    /// `<search mode>`, if written: APPROX first as a request, with no
    /// target yet, or EXACT.
    pub(super) fn search_mode(&mut self) -> Search {
        if self.accept_word(APPROX) || self.accept_word(APPROXIMATE) {
            return Search::Approximate(SearchTarget::default());
        }
        if self.accept_word(EXACT) { Search::Exact } else { Search::Unstated }
    }

    /// `<target accuracy clause>`, if written: what it asks, and where it
    /// begins.
    pub(super) fn target_accuracy_clause(&mut self) -> Result<Option<(SearchTarget, usize)>> {
        let at = self.here();
        let written = self.is_kw(kw::WITH) && matches!(self.peek_at(1), Tok::Identifier(w) if w == TARGET);
        if !written {
            return Ok(None);
        }
        self.advance();
        self.advance();
        self.expect_word(ACCURACY)?;
        let mut target = SearchTarget::default();
        if !self.accept_word(PARAMETERS) {
            target.accuracy = Some(self.accuracy()?);
            return Ok(Some((target, at)));
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            self.search_parameter(&mut target)?;
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some((target, at)))
    }

    /// [R9.4] A target accuracy, from 1 to 100.
    fn accuracy(&mut self) -> Result<u8> {
        let n = self.unsigned_integer()?;
        if n == 0 || n > MOST_ACCURACY {
            return Err(self.rule("[EXTENSION] a target accuracy is a percentage from 1 to 100"));
        }
        u8::try_from(n).map_err(|_| self.rule("[EXTENSION] a target accuracy is a percentage from 1 to 100"))
    }

    /// A `<search parameter>`, into `target`.
    fn search_parameter(&mut self, target: &mut SearchTarget) -> Result<()> {
        if self.accept_word(EFSEARCH) || self.accept_word(SEARCH_LIST) {
            target.search_list = Some(self.positive_count()?);
        } else if self.accept_word(BEAM_WIDTH) {
            target.beam_width = Some(self.positive_count()?);
        } else if self.accept_word(NEIGHBOR) {
            self.expect_word(PARTITION)?;
            self.expect_word(PROBES)?;
            target.probes = Some(self.positive_count()?);
        } else {
            return Err(self.syntax("expected EFSEARCH, NEIGHBOR PARTITION PROBES, SEARCH_LIST or BEAM_WIDTH"));
        }
        Ok(())
    }

    /// An `<unsigned integer>` above 0.
    fn positive_count(&mut self) -> Result<u32> {
        let n = self.unsigned_integer()?;
        if n == 0 {
            return Err(self.rule("[EXTENSION] a search parameter is at least 1"));
        }
        Ok(n)
    }

    /// `<vector hints>`, if written; none if not.
    pub(super) fn vector_hints(&mut self) -> Result<Hints> {
        let mut hints = Hints::default();
        let written = self.is_word(HINT) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN);
        if !written {
            return Ok(hints);
        }
        self.advance();
        self.advance();
        loop {
            self.vector_hint(&mut hints)?;
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(hints)
    }

    /// A `<vector hint>`, into `hints`.
    fn vector_hint(&mut self, hints: &mut Hints) -> Result<()> {
        let strategy = [(VECTOR_PREFILTER, Strategy::Pre), (VECTOR_INFILTER, Strategy::In), (VECTOR_POSTFILTER, Strategy::Post)];
        if let Some((_, s)) = strategy.into_iter().find(|(word, _)| self.is_word(word)) {
            self.advance();
            hints.filter = Some(s);
        } else if self.accept_word(VECTOR_WAIT_FRESH) {
            hints.wait_fresh = true;
        } else if self.accept_word(VECTOR_INDEX) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let table = self.identifier()?;
            let index = self.identifier()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            hints.index = Some((table, index));
        } else if self.accept_word(NO_VECTOR_INDEX) {
            self.expect_delim(delim::LEFT_PAREN)?;
            hints.no_index = Some(self.identifier()?);
            self.expect_delim(delim::RIGHT_PAREN)?;
        } else {
            return Err(self.syntax("expected a <vector hint>"));
        }
        Ok(())
    }
}
