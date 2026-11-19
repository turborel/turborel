//! [ISO/IEC 9075:1992 §5.3] `<character string literal>` and [ISO/IEC
//! 9075-2:2023 §5.3] `<binary string literal>`: their quoted parts.

use super::{QUOTE, QUOTE_REPRESENTATION_LEN, Scanner, Tok, error};
use crate::error::{Code, Result};
use crate::value::Binary;

impl Scanner {
    /// [ISO/IEC 9075:1992 §5.3] `<character string literal>` ::= '
    /// \[`<character representation>`...\] ' \[{`<separator>`... '
    /// \[`<character representation>`...\] '}...\], where `<quote
    /// representation>` ::= ''. The parts are one literal [SR1] if each
    /// separator has a `<newline>` [SR4]; it may be empty [SR9].
    pub(super) fn character_string(&mut self) -> Result<Tok> {
        self.quoted_parts().map(Tok::CharacterString)
    }

    /// The characters of a literal's quoted parts, one after another if
    /// each separator between them has a `<newline>` [ISO/IEC 9075:1992
    /// §5.3 SR1-SR4].
    pub(super) fn quoted_parts(&mut self) -> Result<String> {
        let mut value = String::new();
        loop {
            self.quoted_part(&mut value)?;
            let after = self.at;
            while self.skip_separator()? {}
            let newline = self.c.get(after..self.at).unwrap_or_default().contains(&'\n');
            if !(newline && self.peek(0) == Some(QUOTE)) {
                self.at = after;
                return Ok(value);
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §5.3] `<binary string literal>` ::= X ' pairs
    /// of `<hexit>`s, spaces between them, ' and further quoted parts, each
    /// `<hexit>` four bits \[SR20\]. X, in either case \[SR6\], is the next
    /// character. Spaces are passed over \[SR4\], and the parts are one
    /// literal as a character string literal's are \[SR5, SR7\].
    pub(super) fn binary_string(&mut self) -> Result<Tok> {
        let start = self.at;
        self.advance(1);
        let hexits = self.quoted_parts()?;
        Binary::of_hex(&hexits).map(Tok::BinaryString).ok_or_else(|| {
            let msg = "[ISO/IEC 9075-2:2023 §5.3] a <binary string literal> holds pairs of <hexit>s and spaces, and nothing else";
            error(Code::Lexical, start, msg)
        })
    }

    /// One quoted part of a `<character string literal>`, its characters
    /// added to `value`.
    fn quoted_part(&mut self, value: &mut String) -> Result<()> {
        let start = self.at;
        self.advance(1);
        loop {
            let Some(ch) = self.peek(0) else {
                return Err(error(Code::Lexical, start, "[X3.135-1986 §5.2 F] unterminated <character string literal>"));
            };
            // [X3.135-1986 §5.1 SR1] the end-of-line indicator is not a <character>.
            if self.newline_len().is_some() {
                return Err(error(
                    Code::Lexical,
                    self.at,
                    "[X3.135-1986 §5.1 SR1] a <newline> cannot appear inside a <character string literal>",
                ));
            }
            if ch == QUOTE {
                if self.peek(1) != Some(QUOTE) {
                    self.advance(1);
                    break;
                }
                value.push(QUOTE);
                self.advance(QUOTE_REPRESENTATION_LEN);
                continue;
            }
            value.push(ch);
            self.advance(1);
        }
        Ok(())
    }
}
