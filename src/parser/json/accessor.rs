//! [ISO/IEC 9075-2:2023 §6.36] The `<JSON simplified accessor>` ops SQL
//! has no reading of: a wildcard member accessor `.*` [Feature T862], but
//! where it may end an `<all fields reference>` [§7.16], a
//! member accessor of a `<JSON path string literal>` `.'key'` or `."key"`
//! [T863], a wildcard array accessor `[*]`, and an array accessor of `to`
//! or `last`. From the first of them, every op that follows is read as
//! written, its keys' case kept \[NOTE 198\], and the chain made a
//! `JsonTyped::Accessor` of what precedes it. The ops SQL reads too, an
//! identifier's member, an element reference and a method invocation,
//! stay SQL's, which the binder makes accessors of a value of the JSON
//! type (`binder/json/accessor.rs`).

use super::super::Parser;
use crate::ast::{JsonExpression, JsonTyped, ValueExpression};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The words that make an array accessor one SQL has no reading of.
const JSON_SUBSCRIPT_WORDS: [&str; 2] = ["TO", "LAST"];

impl Parser {
    /// Whether an op SQL has no reading of is next.
    pub(in crate::parser) fn at_json_only_op(&self) -> bool {
        let star = |p: &Parser, n: usize| matches!(p.peek_at(n), Tok::Delimiter(d) if *d == delim::ASTERISK);
        let quoted = matches!(self.peek_at(1), Tok::CharacterString(_));
        let wildcard = star(self, 1) && !self.all_fields_end();
        let member = self.is_delim(delim::PERIOD) && (wildcard || quoted);
        member || (self.is_delim(delim::LEFT_BRACKET) && (star(self, 1) || self.json_subscript()))
    }

    /// [ISO/IEC 9075-2:2023 §7.16] Whether the `.*` next may end an `<all
    /// fields reference>`, a `<select sublist>` SQL reads: a comma, AS,
    /// FROM or the statement's end follows it. Such a `.*` is SQL's, not a
    /// wildcard member accessor, which a chain that goes on after it, or
    /// stands elsewhere, is read as.
    fn all_fields_end(&self) -> bool {
        let after = self.peek_at(2);
        let word = matches!(after, Tok::KeyWord(k) if *k == kw::AS || *k == kw::FROM);
        word || matches!(after, Tok::End) || matches!(after, Tok::Delimiter(d) if *d == delim::COMMA)
    }

    /// Whether the brackets next hold `to` or `last` before they close.
    fn json_subscript(&self) -> bool {
        let mut n: usize = 1;
        loop {
            let tok = self.peek_at(n);
            if matches!(tok, Tok::End) || matches!(tok, Tok::Delimiter(d) if *d == delim::RIGHT_BRACKET) {
                return false;
            }
            let word = if let Tok::KeyWord(w) = tok {
                Some(*w)
            } else if let Tok::Identifier(w) = tok {
                Some(w.as_str())
            } else {
                None
            };
            if word.is_some_and(|w| JSON_SUBSCRIPT_WORDS.contains(&w)) {
                return true;
            }
            n = n.saturating_add(1);
        }
    }

    /// [§6.36] The accessor of `primary` and the ops from here on, read
    /// as written, an op SQL has no reading of next.
    ///
    /// # Errors
    ///
    /// A syntax error if brackets or parentheses do not close.
    pub(in crate::parser) fn json_raw_chain(&mut self, primary: ValueExpression) -> Result<ValueExpression> {
        let (at, start) = (self.here(), self.here());
        loop {
            if self.accept_delim(delim::PERIOD) {
                // `*`, a quoted key, or a key and its method's arguments.
                self.advance();
                if self.is_delim(delim::LEFT_PAREN) {
                    self.balanced(delim::LEFT_PAREN, delim::RIGHT_PAREN)?;
                }
            } else if self.is_delim(delim::LEFT_BRACKET) {
                self.balanced(delim::LEFT_BRACKET, delim::RIGHT_BRACKET)?;
            } else {
                break;
            }
        }
        let end = self.previous_end();
        let chain: String = self.src.get(start..end).unwrap_or_default().iter().collect();
        let typed = JsonTyped::Accessor { primary, chain, sql: None };
        Ok(ValueExpression::Json(Box::new(JsonExpression::Typed(typed)), at))
    }

    /// Takes `open`, then every token to the `close` that matches it.
    fn balanced(&mut self, open: &str, close: &str) -> Result<()> {
        self.expect_delim(open)?;
        let mut depth: usize = 1;
        while depth > 0 {
            if matches!(self.peek(), Tok::End) {
                return Err(self.syntax(format!("expected {close}")));
            }
            if self.is_delim(open) {
                depth = depth.saturating_add(1);
            } else if self.is_delim(close) {
                depth = depth.saturating_sub(1);
            }
            self.advance();
        }
        Ok(())
    }

    /// The character offset just past the token before this one.
    fn previous_end(&self) -> usize {
        self.toks.get(self.pos.saturating_sub(1)).map_or(0, |t| t.end)
    }
}
