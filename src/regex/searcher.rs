//! \[DESIGN\] (RULES.md D1188) Searches of one regular expression over
//! text after text. A search first asks the expression's automaton
//! (`regex/dfa.rs`), if it has one, where the match ends: a text with no
//! match is done there, and so is a search that asks only whether there
//! is one. What a match's groups captured is then found by the matcher
//! that marks (`regex/bounded.rs`), reading no further than that end, or
//! by the matcher of threads past its marks. The automaton's states are
//! kept from one text to the next, so a searcher lives as long as the
//! texts it is given: a call over a vector of texts makes one for the
//! vector. A search of one text alone makes no automaton.
//!
//! Before a search that asks what was captured, the automaton is a pass
//! more over a text that matches, and the whole search of one that does
//! not. On `bench/regex.rs` its pass cost a matching text about four
//! tenths of the marking matcher's own, and saved a text with no match
//! about two thirds of that matcher's search of it, which balance where
//! about half the texts have no match. So the searcher counts what its
//! searches for captures found, a window of them at a time, and asks the
//! automaton first only while at least half of the last window found
//! none. A search that asks only whether there is a match always asks it.

use super::{Ascii, Captures, Matcher, Regex, Text, backtrack, bounded, dfa, pike};
use crate::error::Result;

/// Searches of a regular expression, and its automaton, if it has one and
/// has not outgrown its states.
pub struct Searcher<'r> {
    pub(super) regex: &'r Regex,
    pub(super) dfa: Option<dfa::Dfa>,
    /// What its searches for captures found, of the window being counted.
    pub(super) found: Found,
    /// What the matcher that marks keeps from one search to the next.
    pub(super) kept: bounded::Scratch,
}

/// \[DESIGN\] (RULES.md D1188) The searches for captures a searcher counts
/// before it decides again whether the automaton is asked first. The
/// share of them that found none is known from n searches to within
/// 1/(2√n), its standard error near one half: of 64, one sixteenth, so a
/// share under three eighths or over five eighths, two errors from the
/// half that decides, is taken for the other side of it about once in
/// forty windows, and between those the two ways cost nearly the same.
/// A window is one thirty-second of a vector's rows
/// (`storage::BATCH_ROWS`), so what the first window of a vector costs by
/// asking unasked is within a few hundredths of the vector's searches.
const WINDOW: u32 = 64;

/// How many searches for captures of the window being counted there were,
/// how many found no match, and whether the automaton is asked first: at
/// first it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Found {
    searches: u32,
    none: u32,
    asks: bool,
}

impl Default for Found {
    fn default() -> Found {
        Found { searches: 0, none: 0, asks: true }
    }
}

impl Found {
    /// Counts a search that found a match, or `none`, and at the end of a
    /// window decides whether the next asks the automaton first: if at
    /// least half found none.
    fn count(&mut self, none: bool) {
        self.searches = self.searches.saturating_add(1);
        self.none = self.none.saturating_add(u32::from(none));
        if self.searches >= WINDOW {
            *self = Found { searches: 0, none: 0, asks: self.none.saturating_mul(2) >= self.searches };
        }
    }
}

impl Searcher<'_> {
    /// The expression searched for.
    pub fn regex(&self) -> &Regex {
        self.regex
    }

    /// As `Regex::is_match`: a string of ASCII alone read where it lies.
    ///
    /// # Errors
    ///
    /// As `Regex::is_match`.
    pub fn is_match(&mut self, text: &str) -> Result<bool> {
        if let Some(ascii) = Ascii::of(text) {
            return self.is_match_in(&ascii);
        }
        let chars: Vec<char> = text.chars().collect();
        self.is_match_in(chars.as_slice())
    }

    /// As `Regex::is_match_in`.
    ///
    /// # Errors
    ///
    /// As `Regex::is_match_in`.
    pub fn is_match_in<T: Text + ?Sized>(&mut self, text: &T) -> Result<bool> {
        Ok(self.search(text, 0, false)?.is_some())
    }

    /// As `Regex::find_in`.
    ///
    /// # Errors
    ///
    /// As `Regex::find_in`.
    pub fn find_in<T: Text + ?Sized>(&mut self, text: &T, from: usize) -> Result<Option<Captures>> {
        let Some(slots) = self.search(text, from, true)? else { return Ok(None) };
        let spans = slots.chunks(2).map(|pair| if let [Some(a), Some(b)] = pair { Some((*a, *b)) } else { None }).collect();
        Ok(Some(Captures { spans }))
    }

    /// As `Regex::each`.
    ///
    /// # Errors
    ///
    /// As `Regex::each`.
    pub fn each<T, F>(&mut self, text: &T, from: usize, most: usize, mut f: F) -> Result<()>
    where
        T: Text + ?Sized,
        F: FnMut(Captures) -> Result<bool>,
    {
        let (mut at, mut given) = (from, 0_usize);
        while given < most && at <= text.len() {
            let Some(next) = self.find_in(text, at)? else { break };
            let (start, end) = next.get(0).unwrap_or((at, at));
            at = end.max(start.saturating_add(1));
            given = given.saturating_add(1);
            if !f(next)? {
                break;
            }
        }
        Ok(())
    }

    /// As `Regex::nth_in`.
    ///
    /// # Errors
    ///
    /// As `Regex::each`.
    pub fn nth_in<T: Text + ?Sized>(&mut self, text: &T, from: usize, n: usize) -> Result<Option<Captures>> {
        let (mut seen, mut last) = (0_usize, None);
        self.each(text, from, n, |mv| {
            seen = seen.saturating_add(1);
            last = (seen == n).then_some(mv);
            Ok(true)
        })?;
        Ok(last)
    }

    /// The first match in `text` beginning at `from` or after: each slot of
    /// where its groups are if `capture`, else none of them.
    fn search<T: Text + ?Sized>(&mut self, text: &T, from: usize, capture: bool) -> Result<Option<pike::Slots>> {
        let regex = self.regex;
        let mut end = text.len();
        let asked = !capture || self.found.asks;
        if let Some(automaton) = self.dfa.as_mut().filter(|_| asked && from <= end) {
            match automaton.scan(&regex.program, text, from, !capture)? {
                dfa::Scan::None => {
                    if capture {
                        self.found.count(true);
                    }
                    return Ok(None);
                }
                dfa::Scan::Ends(at) => {
                    if !capture {
                        return Ok(Some(Vec::new()));
                    }
                    end = at;
                }
                dfa::Scan::GaveUp => self.dfa = None,
            }
        }
        let slots = match regex.matcher(end, from) {
            Matcher::Backtrack => backtrack::search(&regex.program, text, from, regex.caseless.then_some(&regex.table)),
            Matcher::Bounded(positions) => bounded::search(&regex.program, text, (from, positions), capture, &mut self.kept),
            Matcher::Pike => pike::search(&regex.program, text, from, capture),
        }?;
        if capture {
            self.found.count(slots.is_none());
        }
        Ok(slots)
    }
}

// Unit tests: a searcher against the first matchers over a corpus
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
