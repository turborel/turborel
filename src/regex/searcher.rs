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
//! \[DESIGN\] (RULES.md D1189) A search leaves where its match's groups
//! are in the searcher (`bounded::Scratch`), which a caller reads into a
//! match vector it keeps from one match to the next (`find_into`), so
//! a match of a vector's subject takes no memory of its own.
//!
//! Before a search that asks what was captured, the automaton is a pass
//! more over a text that matches, and the whole search of one that does
//! not. On `bench/regex.rs` what the pass costs the texts that match and
//! what it saves those that do not balance where about half the texts
//! have no match. So the searcher counts what its
//! searches for captures found, a window of them at a time, and asks the
//! automaton first only while at least half of the last window found
//! none. A search that asks only whether there is a match always asks it.
//!
//! \[DESIGN\] (RULES.md D1292) A searcher done leaves what it kept, its
//! automaton, its count of what its searches found and its scratch, with
//! its expression (`Leavings`), and the next searcher of that expression
//! takes them up. A call over vectors makes a searcher a vector, so the
//! automaton's states and the window's verdict now last the column, not
//! the vector.

use std::sync::Mutex;

use super::{Ascii, Captures, Matcher, Regex, Text, Utf8, backtrack, bounded, dfa, pike};
use crate::error::Result;
use crate::sync::lock_builtin;

/// Searches of a regular expression, and its automaton, if it has one and
/// has not outgrown its states.
pub struct Searcher<'r> {
    pub(super) regex: &'r Regex,
    pub(super) dfa: Option<dfa::Dfa>,
    /// What its searches for captures found, of the window being counted.
    pub(super) found: Found,
    /// What the matcher that marks keeps from one search to the next.
    pub(super) kept: bounded::Scratch,
    /// Whether it searches text after text, so what it keeps is worth
    /// leaving for the next (`Regex::searcher`), not one text alone.
    pub(super) lasting: bool,
}

/// \[DESIGN\] (RULES.md D1292) What a searcher done leaves for the next
/// of its expression: its automaton, if it has one still, what its last
/// window of searches found, and its scratch.
pub(super) struct Left {
    pub(super) dfa: Option<dfa::Dfa>,
    pub(super) found: Found,
    pub(super) kept: bounded::Scratch,
}

/// \[DESIGN\] (RULES.md D1292) What the searchers of an expression left,
/// one `Left` of each that was at work at once and is done. A searcher
/// takes one up as it is made and leaves one as it is dropped, so there
/// are never more than the searchers that were at work at one time, the
/// threads of the statements that share the expression, and each is
/// within what one searcher may hold (`dfa::OCTETS_MOST`,
/// `bounded::MARKS_OCTETS`). An expression copied starts with none.
#[derive(Default)]
pub(super) struct Leavings(Mutex<Vec<Left>>);

impl Clone for Leavings {
    fn clone(&self) -> Leavings {
        Leavings::default()
    }
}

impl std::fmt::Debug for Leavings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Leavings")
    }
}

impl Leavings {
    /// What a searcher done left, the last to be done first, taken out.
    pub(super) fn take(&self) -> Option<Left> {
        lock_builtin(&self.0).pop()
    }

    /// Keeps `left` for the next searcher.
    fn leave(&self, left: Left) {
        lock_builtin(&self.0).push(left);
    }
}

impl Drop for Searcher<'_> {
    /// Leaves what it kept with its expression, if it searched text after
    /// text.
    fn drop(&mut self) {
        if self.lasting {
            self.regex.left.leave(Left { dfa: self.dfa.take(), found: self.found, kept: std::mem::take(&mut self.kept) });
        }
    }
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

    /// Whether its next search for captures asks the automaton first, as
    /// what its last window of them found says.
    pub fn asks_first(&self) -> bool {
        self.found.asks
    }

    /// As `Regex::is_match`, the string read where it lies, one of
    /// ASCII alone by its octets as they are (RULES.md D1188, D1189).
    ///
    /// # Errors
    ///
    /// As `Regex::is_match`.
    pub fn is_match(&mut self, text: &str) -> Result<bool> {
        if let Some(ascii) = Ascii::of(text) {
            return self.is_match_in(&ascii);
        }
        self.is_match_in(&Utf8::of(text))
    }

    /// As `Regex::is_match_in`.
    ///
    /// # Errors
    ///
    /// As `Regex::is_match_in`.
    pub fn is_match_in<T: Text + ?Sized>(&mut self, text: &T) -> Result<bool> {
        self.search(text, 0, false)
    }

    /// As `Regex::find_in`.
    ///
    /// # Errors
    ///
    /// As `Regex::find_in`.
    pub fn find_in<T: Text + ?Sized>(&mut self, text: &T, from: usize) -> Result<Option<Captures>> {
        let mut found = Captures::default();
        Ok(self.find_into(text, from, &mut found)?.then_some(found))
    }

    /// \[DESIGN\] (RULES.md D1189) As `find_in`, the match vector put in
    /// `found`, whose memory is used again. It gives whether there is a
    /// match, and with none `found` holds no span.
    ///
    /// # Errors
    ///
    /// As `Regex::find_in`.
    pub fn find_into<T: Text + ?Sized>(&mut self, text: &T, from: usize, found: &mut Captures) -> Result<bool> {
        found.spans.clear();
        if !self.search(text, from, true)? {
            return Ok(false);
        }
        let spans = self.kept.slots().chunks(2).map(|pair| if let [Some(a), Some(b)] = pair { Some((*a, *b)) } else { None });
        found.spans.extend(spans);
        Ok(true)
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
            at = end.max(text.after(start));
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

    /// Whether `text` has a match beginning at `from` or after. The first
    /// one's slots, of where its groups are, are left in what the
    /// searcher keeps if `capture`, else none of them.
    fn search<T: Text + ?Sized>(&mut self, text: &T, from: usize, capture: bool) -> Result<bool> {
        let regex = self.regex;
        let mut end = text.len();
        let asked = !capture || self.found.asks;
        if let Some(automaton) = self.dfa.as_mut().filter(|_| asked && from <= end) {
            match automaton.scan(&regex.program, text, from, !capture)? {
                dfa::Scan::None => {
                    if capture {
                        self.found.count(true);
                    }
                    return Ok(false);
                }
                dfa::Scan::Ends(at) => {
                    if !capture {
                        return Ok(true);
                    }
                    end = at;
                }
                dfa::Scan::GaveUp => self.dfa = None,
            }
        }
        let found = match regex.matcher(end, from) {
            Matcher::Backtrack => {
                let slots = backtrack::search(&regex.program, text, from, regex.caseless.then_some(&regex.table))?;
                self.kept_of(slots)
            }
            Matcher::Bounded(positions) => bounded::search(&regex.program, text, (from, positions), capture, &mut self.kept)?,
            Matcher::Pike => {
                let slots = pike::search(&regex.program, text, from, capture)?;
                self.kept_of(slots)
            }
        };
        if capture {
            self.found.count(!found);
        }
        Ok(found)
    }

    /// Keeps `slots`, what a matcher that gives its match's slots found,
    /// where the matcher that marks leaves its own, and gives whether
    /// there is a match.
    fn kept_of(&mut self, slots: Option<pike::Slots>) -> bool {
        let found = slots.is_some();
        self.kept.put(slots.unwrap_or_default());
        found
    }
}

// Unit tests: a searcher against the first matchers over a corpus
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
