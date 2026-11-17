//! The terminals the lexer reads character by character [XQuery Appendix
//! A.2.1]: names, numeric and string literals, and the references string
//! literals and direct constructors share.

use super::{Lexer, Tok};
use crate::error::Result;
use crate::xml::{COLON, is_char, is_name_char, is_name_start, predefined};
use crate::xquery::ast::QName;

/// [XML §4.1 \[66\]] The base of a hexadecimal character reference.
const HEX: u32 = 16;
/// The base of a decimal one.
const DECIMAL: u32 = 10;

impl Lexer {
    /// Whether `c` may start an `NCName` of the lexer's version
    /// [Namespaces 1.0 §2 \[4\]] [Namespaces 1.1 §3 \[4\]], so XQuery's names are XML's [XQuery Appendix
    /// A.1.2, xml-version].
    pub(in crate::xquery) fn is_name_start(&self, c: char) -> bool {
        c != COLON && is_name_start(self.version, c)
    }

    /// Whether `c` may follow an `NCName`'s first character.
    fn is_name_char(&self, c: char) -> bool {
        c != COLON && is_name_char(self.version, c)
    }

    /// Reads an `NCName` [XQuery Appendix A.2.1 \[155\]], if one is next.
    pub(in crate::xquery) fn ncname(&mut self) -> Option<String> {
        if !self.peek_char().is_some_and(|c| self.is_name_start(c)) {
            return None;
        }
        let mut name = String::new();
        while let Some(c) = self.peek_char().filter(|&c| self.is_name_char(c)) {
            name.push(c);
            self.at = self.at.saturating_add(1);
        }
        Some(name)
    }

    /// Reads a `QName` [XQuery Appendix A.2.1 \[154\]], if one is next: an
    /// `NCName`, then a colon and an `NCName` if they are next, with no
    /// whitespace between.
    pub(in crate::xquery) fn qname(&mut self) -> Option<QName> {
        let first = self.ncname()?;
        if self.peek_char() == Some(COLON) && self.char_at(1).is_some_and(|c| self.is_name_start(c)) {
            self.at = self.at.saturating_add(1);
            let local = self.ncname().unwrap_or_default();
            return Some(QName { prefix: Some(first), local });
        }
        Some(QName { prefix: None, local: first })
    }

    /// Reads digits, `[0-9]*` [XQuery Appendix A.2.1 \[158\]], into `text`.
    fn digits(&mut self, text: &mut String) {
        while let Some(d) = self.peek_char().filter(char::is_ascii_digit) {
            text.push(d);
            self.at = self.at.saturating_add(1);
        }
    }

    /// Reads a numeric literal [XQuery Appendix A.2.1 \[141\] to \[143\]],
    /// which is next. A name or another numeric literal right after it
    /// is a syntax error: two non-delimiting terminals need a separator
    /// between them [Appendix A.2.2], as `10div 3` shows [Appendix
    /// A.2.4.1].
    pub(super) fn numeric(&mut self) -> Result<Tok> {
        let start = self.at;
        let mut text = String::new();
        self.digits(&mut text);
        let decimal = self.peek_char() == Some('.');
        if decimal {
            text.push('.');
            self.at = self.at.saturating_add(1);
            self.digits(&mut text);
        }
        let signed = matches!(self.char_at(1), Some('+' | '-'));
        let exponent_digit = self.char_at(usize::from(signed).saturating_add(1)).is_some_and(|d| d.is_ascii_digit());
        let double = matches!(self.peek_char(), Some('e' | 'E')) && exponent_digit;
        if double {
            text.extend(self.bump());
            if signed {
                text.extend(self.bump());
            }
            self.digits(&mut text);
        }
        let next = self.peek_char();
        let adjacent_number = next == Some('.') && self.char_at(1).is_some_and(|d| d.is_ascii_digit());
        if next.is_some_and(|c| self.is_name_start(c)) || adjacent_number {
            return Err(self.error(start, "a numeric literal is separated from a name or number after it [XQuery Appendix A.2.2]"));
        }
        Ok(if double {
            Tok::Double(text)
        } else if decimal {
            Tok::Decimal(text)
        } else {
            Tok::Integer(text)
        })
    }

    /// Reads a string literal [XQuery Appendix A.2.1 \[144\]], which is next:
    /// its value, each doubled delimiter one, each reference the character
    /// it stands for [XQuery §3.1.1].
    pub(super) fn string_literal(&mut self) -> Result<String> {
        let start = self.at;
        let quote = self.bump().unwrap_or_default();
        let mut value = String::new();
        loop {
            let Some(c) = self.peek_char() else {
                return Err(self.error(start, "a string literal is not closed by its quote [XQuery Appendix A.2.1 [144]]"));
            };
            if c == '&' {
                value.push(self.reference()?);
                continue;
            }
            self.at = self.at.saturating_add(1);
            // [XQuery Appendix A.2.1 [146], [147]] The quote doubled is one
            // quote of the value; alone, it ends the literal.
            if c == quote && self.peek_char() != Some(quote) {
                return Ok(value);
            }
            if c == quote {
                self.at = self.at.saturating_add(1);
            }
            value.push(c);
        }
    }

    /// Reads a `PredefinedEntityRef` or a `CharRef` [XQuery Appendix A.2.1
    /// \[145\], \[153\]], which an `&` next begins, and gives the character
    /// it stands for. A character reference is XML's [XML §4.1 \[66\]],
    /// to a `Char` of the lexer's version [XML §4.1 WFC: Legal Character]
    /// [XQuery §3.1.1].
    pub(in crate::xquery) fn reference(&mut self) -> Result<char> {
        let start = self.at;
        self.at = self.at.saturating_add(1);
        let found = if self.eat("#") { self.char_reference() } else { self.entity_reference() };
        found.ok_or_else(|| {
            self.error(
                start,
                "'&' begins a reference: &lt;, &gt;, &amp;, &quot;, &apos; or a character reference to a Char [XQuery Appendix A.2.1 [145], [153]]",
            )
        })
    }

    /// The character a predefined entity reference stands for, read after
    /// its `&` to its `;`, if it is one [XQuery Appendix A.2.1 \[145\]].
    fn entity_reference(&mut self) -> Option<char> {
        let name = self.ncname()?;
        if !self.eat(";") {
            return None;
        }
        predefined(&name)
    }

    /// The character a character reference stands for, read after its
    /// `&#` to its `;`, if it is one.
    fn char_reference(&mut self) -> Option<char> {
        let base = if self.eat("x") { HEX } else { DECIMAL };
        let mut code: Option<u32> = Some(0);
        let mut digits = 0_usize;
        while let Some(digit) = self.peek_char().and_then(|c| c.to_digit(base)) {
            self.at = self.at.saturating_add(1);
            digits = digits.saturating_add(1);
            code = code.and_then(|c| c.checked_mul(base)).and_then(|c| c.checked_add(digit));
        }
        let closed = digits > 0 && self.eat(";");
        code.filter(|_| closed).and_then(char::from_u32).filter(|&c| is_char(self.version, c))
    }
}
