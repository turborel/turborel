//! \[DESIGN\] What a regular expression is matched against: a text read a
//! character at a time by its position, a string's characters in memory
//! or a character large object read a chunk at a time (`regex/lob_text.rs`,
//! RULES.md D79); and what the matchers ask of one, a character, a line
//! boundary of UTS #18 RL1.6 [ISO/IEC 9075-2:2023 §9.28 GR9a i], whether
//! an instruction matches.

use super::class::NEWLINES;
use super::compile::Inst;
use crate::error::Result;

/// A text the matchers read by position, characters from 0.
pub trait Text {
    /// How many characters it has.
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
}

/// The character before `at`, none at the start.
fn before<T: Text + ?Sized>(text: &T, at: usize) -> Result<Option<char>> {
    match at.checked_sub(1) {
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

/// \[F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i\] Whether `^` matches at `at`:
/// the string's start; and multi-line, after a newline sequence but at the
/// string's end.
pub(super) fn line_start<T: Text + ?Sized>(text: &T, at: usize, multiline: bool) -> Result<bool> {
    if at == 0 {
        return Ok(true);
    }
    if !multiline || at >= text.len() || !newline(before(text, at)?) {
        return Ok(false);
    }
    Ok(!within_crlf(text, at)?)
}

/// \[F&O 3.1 §5.6.1.1, §5.6.2; §9.28 GR9a i\] Whether `$` matches at `at`:
/// the string's end; and multi-line, before a newline sequence, the
/// string's end only if no newline ends it.
pub(super) fn line_end<T: Text + ?Sized>(text: &T, at: usize, multiline: bool) -> Result<bool> {
    let end = text.len();
    if !multiline {
        return Ok(at == end);
    }
    if at == end {
        return Ok(!newline(before(text, end)?));
    }
    Ok(newline(text.at(at)?) && !within_crlf(text, at)?)
}

/// Whether `inst`, one that matches a character, matches the one at `at`.
#[inline]
pub(super) fn one<T: Text + ?Sized>(inst: &Inst, text: &T, at: usize) -> Result<bool> {
    let Some(c) = text.at(at)? else { return Ok(false) };
    Ok(match inst {
        Inst::Char(want) => c == *want,
        Inst::Set(set) => set.contains(c),
        Inst::AnyButCrlf => !(c == '\r' && text.at(at.saturating_add(1))? == Some('\n')),
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
