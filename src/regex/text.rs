//! \[DESIGN\] What a regular expression is matched against: a text read a
//! character at a time by its position, a string's characters in memory
//! or a character large object read a chunk at a time (`regex/lob_text.rs`,
//! RULES.md D79); and what the matchers ask of one, a character, a line
//! boundary of UTS #18 RL1.6 [ISO/IEC 9075-2:2023 §9.28 GR9a i], whether
//! an instruction matches.
//!
//! A position is where a character begins, and the text says which
//! position is after a character and which before (`Text::after`,
//! `Text::before`). Of a vector of characters and of a large object the
//! positions count characters. Of a string read where it lies they
//! count its octets (`Utf8`, RULES.md D1189), so a matcher never adds
//! one to a position to pass a character.

use super::class::NEWLINES;
use super::compile::Inst;
use crate::error::Result;

/// A text the matchers read by position, its first character at 0.
pub trait Text {
    /// The position past its last character, which is how many
    /// characters it has where positions count characters.
    fn len(&self) -> usize;
    /// Whether it has none.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The character at `at`, none at or past its end.
    ///
    /// # Errors
    ///
    /// As reading the text fails: SQLCODE -910 of a large object whose
    /// chunk cannot be read.
    fn at(&self, at: usize) -> Result<Option<char>>;
    /// Characters `from` to `to`, if it holds them in memory together, so
    /// a back-reference compares them at once; none if it reads them one
    /// at a time.
    fn held(&self, _from: usize, _to: usize) -> Option<&[char]> {
        None
    }
    /// The position after the character at `at`.
    #[inline]
    fn after(&self, at: usize) -> usize {
        at.saturating_add(1)
    }
    /// The position of the character before `at`, none at the start.
    #[inline]
    fn before(&self, at: usize) -> Option<usize> {
        at.checked_sub(1)
    }
}

impl Text for [char] {
    fn len(&self) -> usize {
        <[char]>::len(self)
    }
    #[inline]
    fn at(&self, at: usize) -> Result<Option<char>> {
        Ok(self.get(at).copied())
    }
    fn held(&self, from: usize, to: usize) -> Option<&[char]> {
        self.get(from..to)
    }
}

/// \[DESIGN\] (RULES.md D1188) A string of ASCII characters alone, read
/// where it lies: its octets are its characters, so no vector of
/// characters is made of it to match it.
#[derive(Debug, Clone, Copy)]
pub struct Ascii<'t>(&'t str);

impl<'t> Ascii<'t> {
    /// `text`, if every character of it is ASCII's.
    pub fn of(text: &'t str) -> Option<Ascii<'t>> {
        text.is_ascii().then_some(Ascii(text))
    }

    /// Its characters `from` to `to`, the empty string past its end.
    pub fn span(&self, from: usize, to: usize) -> &'t str {
        self.0.get(from..to).unwrap_or_default()
    }
}

impl Text for Ascii<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    #[inline]
    fn at(&self, at: usize) -> Result<Option<char>> {
        Ok(self.0.as_bytes().get(at).map(|&octet| char::from(octet)))
    }
}

/// \[DESIGN\] (RULES.md D1189) A character string read where it lies,
/// whatever its characters. A position is the octet a character begins
/// at, so the character there is decoded as it is read, a character of
/// ASCII by one test of its octet, and what a match spans is cut out of
/// the string with no character counted.
#[derive(Debug, Clone, Copy)]
pub struct Utf8<'t>(&'t str);

/// The octets below this one are each a character of ASCII.
const PAST_ASCII: u8 = 0x80;

/// The bits that mark an octet as one that goes on a character, and
/// their value in such an octet.
const GOES_ON: (u8, u8) = (0xC0, 0x80);

impl<'t> Utf8<'t> {
    /// `text`, read where it lies.
    pub fn of(text: &'t str) -> Utf8<'t> {
        Utf8(text)
    }

    /// The string from position `from` to position `to`, the empty
    /// string if either is no position of it.
    pub fn span(&self, from: usize, to: usize) -> &'t str {
        self.0.get(from..to).unwrap_or_default()
    }

    /// The string itself.
    pub fn text(&self) -> &'t str {
        self.0
    }
}

impl Text for Utf8<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    #[inline]
    fn at(&self, at: usize) -> Result<Option<char>> {
        let Some(&first) = self.0.as_bytes().get(at) else { return Ok(None) };
        if first < PAST_ASCII {
            return Ok(Some(char::from(first)));
        }
        Ok(self.0.get(at..).and_then(|rest| rest.chars().next()))
    }
    #[inline]
    fn after(&self, at: usize) -> usize {
        let octets = self.0.as_bytes();
        let mut next = at.saturating_add(1);
        while octets.get(next).is_some_and(|octet| octet & GOES_ON.0 == GOES_ON.1) {
            next = next.saturating_add(1);
        }
        next
    }
    #[inline]
    fn before(&self, at: usize) -> Option<usize> {
        let octets = self.0.as_bytes();
        let mut last = at.checked_sub(1)?;
        while last > 0 && octets.get(last).is_some_and(|octet| octet & GOES_ON.0 == GOES_ON.1) {
            last = last.saturating_sub(1);
        }
        Some(last)
    }
}

/// Characters `from` to `to` of a text, a text of their own: what
/// fn:replace replaces within one match [ISO/IEC 9075-2:2023 §9.29].
pub struct Window<'t, T: Text + ?Sized> {
    text: &'t T,
    from: usize,
    to: usize,
}

impl<'t, T: Text + ?Sized> Window<'t, T> {
    /// Characters `from` to `to` of `text`, cut to its end.
    pub fn new(text: &'t T, from: usize, to: usize) -> Window<'t, T> {
        let to = to.min(text.len());
        Window { text, from: from.min(to), to }
    }
}

impl<T: Text + ?Sized> Text for Window<'_, T> {
    fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }
    fn at(&self, at: usize) -> Result<Option<char>> {
        if at >= self.len() {
            return Ok(None);
        }
        self.text.at(self.from.saturating_add(at))
    }
    fn held(&self, from: usize, to: usize) -> Option<&[char]> {
        if to > self.len() {
            return None;
        }
        self.text.held(self.from.saturating_add(from), self.from.saturating_add(to))
    }
    fn after(&self, at: usize) -> usize {
        self.text.after(self.from.saturating_add(at)).saturating_sub(self.from)
    }
    fn before(&self, at: usize) -> Option<usize> {
        let before = self.text.before(self.from.saturating_add(at))?;
        before.checked_sub(self.from)
    }
}

/// The character before `at`, none at the start.
fn before<T: Text + ?Sized>(text: &T, at: usize) -> Result<Option<char>> {
    match text.before(at) {
        Some(b) => text.at(b),
        None => Ok(None),
    }
}

/// Whether `c` is one of a newline sequence's characters.
fn newline(c: Option<char>) -> bool {
    c.is_some_and(|c| NEWLINES.contains(&c))
}

/// Whether `at` is between a CR and an LF, no line boundary \[UTS #18
/// RL1.6\].
fn within_crlf<T: Text + ?Sized>(text: &T, at: usize) -> Result<bool> {
    Ok(before(text, at)? == Some('\r') && text.at(at)? == Some('\n'))
}

/// Where `^` and `$` match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lines {
    /// At the string's start and end alone.
    Whole,
    /// Multi-line, at each line's, a line ended by a newline sequence
    /// [§9.28 GR9a i; UTS #18 RL1.6].
    Each,
    /// Multi-line, at each line's, a line ended by a line feed alone
    /// [XQueryFO §7.6.1, §7.6.1.1].
    EachLineFeed,
}

/// \[F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i\] Whether `^` matches at `at`:
/// the string's start; and multi-line, after a newline sequence but at the
/// string's end, or of `EachLineFeed` after a line feed [XQueryFO
/// §7.6.1.1].
pub(super) fn line_start<T: Text + ?Sized>(text: &T, at: usize, lines: Lines) -> Result<bool> {
    if at == 0 {
        return Ok(true);
    }
    match lines {
        Lines::Whole => Ok(false),
        Lines::EachLineFeed => Ok(before(text, at)? == Some('\n')),
        Lines::Each => {
            if at >= text.len() || !newline(before(text, at)?) {
                return Ok(false);
            }
            Ok(!within_crlf(text, at)?)
        }
    }
}

/// \[F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i\] Whether `$` matches at `at`:
/// the string's end; and multi-line, before a newline sequence, the
/// string's end only if no newline ends it; or of `EachLineFeed` before a
/// line feed, and at the string's end [XQueryFO §7.6.1.1].
pub(super) fn line_end<T: Text + ?Sized>(text: &T, at: usize, lines: Lines) -> Result<bool> {
    let end = text.len();
    match lines {
        Lines::Whole => Ok(at == end),
        Lines::EachLineFeed => Ok(at == end || text.at(at)? == Some('\n')),
        Lines::Each => {
            if at == end {
                return Ok(!newline(before(text, end)?));
            }
            Ok(newline(text.at(at)?) && !within_crlf(text, at)?)
        }
    }
}

/// Whether `inst`, one that matches a character, matches the one at `at`.
#[inline]
pub(super) fn one<T: Text + ?Sized>(inst: &Inst, text: &T, at: usize) -> Result<bool> {
    let Some(c) = text.at(at)? else { return Ok(false) };
    Ok(match inst {
        Inst::Char(want) => c == *want,
        Inst::Set(set) => set.contains(c),
        Inst::AnyButCrlf => !(c == '\r' && text.at(text.after(at))? == Some('\n')),
        Inst::LineStart(_)
        | Inst::LineEnd(_)
        | Inst::Split(..)
        | Inst::Jump(_)
        | Inst::Save(_)
        | Inst::BackRef(_)
        | Inst::LoopEnter(_)
        | Inst::LoopCheck(_)
        | Inst::Match => false,
    })
}

// Unit tests: a string read where it lies against its characters
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1189: a string read where it lies
    /// gives at each position the character that begins there, of one,
    /// two, three or four octets, and none within a character or past
    /// its end; the position after a character and the one before a
    /// position step over a character's octets; what two positions span
    /// is cut out as it is, and nothing where one is within a character;
    /// a window of it reads and steps from its own start; and a vector
    /// of characters steps by one.
    #[test]
    fn a_string_read_where_it_lies_is_read_by_the_octets_its_characters_begin_at() {
        let string = "aé€\u{10348}b";
        let (text, starts) = (Utf8::of(string), [0_usize, 1, 3, 6, 10]);
        assert_eq!((text.len(), text.text()), (11, string));
        for (n, c) in string.chars().enumerate() {
            assert_eq!(text.at(starts[n]).unwrap(), Some(c), "character {n}");
            assert_eq!(text.after(starts[n]), starts.get(n + 1).copied().unwrap_or(11), "after character {n}");
            assert_eq!(text.before(starts[n]), n.checked_sub(1).map(|before| starts[before]), "before character {n}");
        }
        assert_eq!(text.before(11), Some(10));
        for within in [2_usize, 4, 5, 7, 8, 9, 11, 12] {
            assert_eq!(text.at(within).unwrap(), None, "position {within}");
        }
        assert_eq!((text.span(1, 6), text.span(2, 6), text.span(6, 12)), ("é€", "", ""));
        let window = Window::new(&text, 1, 10);
        assert_eq!((window.len(), window.at(0).unwrap(), window.at(2).unwrap(), window.at(9).unwrap()), (9, Some('é'), Some('€'), None));
        assert_eq!((window.after(0), window.after(2), window.before(2), window.before(0)), (2, 5, Some(0), None));
        let chars: Vec<char> = string.chars().collect();
        assert_eq!((chars.as_slice().after(2), chars.as_slice().before(2), chars.as_slice().before(0)), (3, Some(1), None));
    }
}
