//! [ISO/IEC 9075-2:2023 §5.2] `<bracketed comment>` ::= /* \[`<bracketed
//! comment contents>`\] */ (Feature T351): a `<separator>`, as a `<simple
//! comment>` is, equivalent to a `<newline>` \[SR12\]. A /* within one
//! begins one nested in it \[SR11\], so each needs its own */.
//!
//! \[EXTENSION\] A statement whose text begins with a bracketed comment
//! whose contents begin with a plus sign is hinted by the words of it
//! (`hinted`), as `/*+ VECTOR_WAIT_FRESH */` asks for fresh embeddings
//! (the vector design's R12.6).
//!
//! \[EXTENSION\] [GS §10.3] After MATCH, "--" is no `<simple comment>`
//! where it is one of Cypher's relationships `)--(`, `)-->(`, `)<--(`
//! and `)<-->(`, written with no space: just after ")" or ")<", and just
//! before "(" or ">(". There a comment would be glued to both a closing
//! and an opening parenthesis, which no SQL text writes.

use super::{Scanner, error};
use crate::error::{Code, Result};

impl Scanner {
    /// Whether the "--" next is part of a Cypher relationship, as this
    /// module describes, not a comment.
    pub(super) fn at_relationship(&self) -> bool {
        let before = |k: usize| self.at.checked_sub(k).and_then(|i| self.c.get(i)).copied();
        let after = |k: usize| self.peek(k.saturating_add(2));
        let opened = before(1) == Some(')') || (before(1) == Some('<') && before(2) == Some(')'));
        let closed = after(0) == Some('(') || (after(0) == Some('>') && after(1) == Some('('));
        self.matched && opened && closed
    }

    /// Skips a `<bracketed comment>`, if one is next, and says whether
    /// there was one.
    ///
    /// # Errors
    ///
    /// SQLCODE -101 if one does not end.
    pub(super) fn bracketed_comment(&mut self) -> Result<bool> {
        if self.peek(0) != Some('/') || self.peek(1) != Some('*') {
            return Ok(false);
        }
        let start = self.at;
        let mut depth = 0_usize;
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some('/'), Some('*')) => depth = depth.saturating_add(1),
                (Some('*'), Some('/')) => depth = depth.saturating_sub(1),
                (Some(_), _) => {
                    self.advance(1);
                    continue;
                }
                (None, _) => return Err(error(Code::Lexical, start, "[ISO/IEC 9075-2:2023 §5.2] a <bracketed comment> must end with */")),
            }
            self.advance(2);
            if depth == 0 {
                return Ok(true);
            }
        }
    }
}

/// \[EXTENSION\] Whether `sql` begins with the hint `word`: a `<bracketed
/// comment>` first, after spaces and newlines only, whose contents begin
/// with a plus sign and hold `word`, in any case, as a word of its own.
pub fn hinted(sql: &str, word: &str) -> bool {
    let Some(body) = sql.trim_start().strip_prefix("/*+") else { return false };
    let Some(end) = body.find("*/") else { return false };
    body.get(..end).is_some_and(|hint| hint.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w.eq_ignore_ascii_case(word)))
}
