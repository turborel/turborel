//! The parser of XQuery modules [XQuery Appendix A.1]: recursive descent,
//! one method for each production, looking a few tokens ahead where a
//! keyword is told from a name by what follows it [Appendix A.1.3,
//! grammar-note: parens].
//!
//! The module and its prolog are read in `parser/prolog.rs`, FLWOR,
//! quantified, typeswitch and conditional expressions in
//! `parser/flwor.rs`, the operators in `parser/operators.rs`, paths and
//! steps in `parser/path.rs`, primary expressions and the computed
//! constructors in `parser/primary.rs`, the direct constructors in
//! `parser/direct.rs`, and sequence types and kind tests in
//! `parser/types.rs`.
//!
//! \[DESIGN\] RULES.md R-CF-6: every recursion passes through an
//! `ExprSingle` or a direct element constructor, each of which counts one
//! level deeper, so the parser refuses a query nested past `MAX_NESTING`
//! with SQLCODE -900 before its stack can overflow.

mod direct;
mod flwor;
mod operators;
mod path;
mod primary;
mod prolog;
mod types;

use std::collections::VecDeque;

use super::MAX_NESTING;
use super::ast::{Module, QName};
use super::error::too_deep;
use super::lexer::{Lexer, Sym, Tok, Token};
use crate::error::{Result, SqlError};
use crate::xml::Version;

/// A query being parsed.
#[derive(Debug)]
struct Parser {
    /// The lexer, read on demand.
    lexer: Lexer,
    /// The tokens read and not yet taken, the next first.
    ahead: VecDeque<Token>,
    /// How many `ExprSingle`s and direct element constructors the one
    /// being parsed is within (`MAX_NESTING`).
    depth: usize,
}

/// The module `text` is, by XML `version`'s lexical rules.
pub(super) fn module(text: &str, version: Version) -> Result<Module> {
    let mut parser = Parser { lexer: Lexer::new(text, version)?, ahead: VecDeque::new(), depth: 0 };
    let module = parser.module()?;
    if !parser.at_end() {
        return Err(parser.unexpected("a module ends after its query body [XQuery Appendix A.1 [1]]"));
    }
    Ok(module)
}

impl Parser {
    /// Reads tokens until the one `n` after the next is read, or the end
    /// or an error is.
    fn fill(&mut self, n: usize) {
        while self.ahead.len() <= n {
            let stop = self.ahead.back().is_some_and(|t| matches!(t.tok, Ok(Tok::End) | Err(_)));
            if stop {
                return;
            }
            let token = self.lexer.token();
            self.ahead.push_back(token);
        }
    }

    /// The token `n` after the next, or the end or error read before it.
    fn peek_token(&mut self, n: usize) -> Option<&Token> {
        self.fill(n);
        let last = self.ahead.len().saturating_sub(1);
        self.ahead.get(n.min(last))
    }

    /// The token `n` after the next, if it was read without error.
    fn peek(&mut self, n: usize) -> Option<&Tok> {
        self.peek_token(n).and_then(|t| t.tok.as_ref().ok())
    }

    /// Whether the token `n` after the next is `sym`.
    fn is_sym(&mut self, n: usize, sym: Sym) -> bool {
        self.peek(n) == Some(&Tok::Sym(sym))
    }

    /// Whether the token `n` after the next is the unprefixed name
    /// `keyword`.
    fn is_keyword(&mut self, n: usize, keyword: &str) -> bool {
        self.peek(n).is_some_and(|tok| if let Tok::Name(name) = tok { name.is(keyword) } else { false })
    }

    /// Whether the token `n` after the next is an `NCName`, a name with no
    /// prefix.
    fn is_ncname(&mut self, n: usize) -> bool {
        self.peek(n).is_some_and(|tok| if let Tok::Name(name) = tok { name.prefix.is_none() } else { false })
    }

    /// Whether the token `n` after the next is a name.
    fn is_name(&mut self, n: usize) -> bool {
        matches!(self.peek(n), Some(Tok::Name(_)))
    }

    /// Whether the text has ended.
    fn at_end(&mut self) -> bool {
        self.peek(0) == Some(&Tok::End)
    }

    /// Where the next token starts.
    fn here(&mut self) -> usize {
        let position = self.lexer.position();
        self.peek_token(0).map_or(position, |t| t.at)
    }

    /// Takes the next token.
    ///
    /// # Errors
    ///
    /// The error reading it raised.
    fn advance(&mut self) -> Result<Token> {
        self.fill(0);
        let token = self.ahead.pop_front().ok_or_else(|| self.lexer.error(self.lexer.position(), "the text ends"))?;
        match &token.tok {
            Err(error) => Err((**error).clone()),
            Ok(_) => Ok(token),
        }
    }

    /// Takes the next token if it is `sym`.
    fn accept_sym(&mut self, sym: Sym) -> bool {
        let matched = self.is_sym(0, sym);
        if matched {
            self.ahead.pop_front();
        }
        matched
    }

    /// Takes the next token if it is the keyword `keyword`.
    fn accept_keyword(&mut self, keyword: &str) -> bool {
        let matched = self.is_keyword(0, keyword);
        if matched {
            self.ahead.pop_front();
        }
        matched
    }

    /// Takes the next token, which must be `sym`; `rule` names the
    /// production that requires it.
    fn expect_sym(&mut self, sym: Sym, rule: &str) -> Result<Token> {
        if self.is_sym(0, sym) {
            return self.advance();
        }
        Err(self.unexpected(rule))
    }

    /// Takes the next token, which must be the keyword `keyword`.
    fn expect_keyword(&mut self, keyword: &str, rule: &str) -> Result<()> {
        if self.accept_keyword(keyword) {
            return Ok(());
        }
        Err(self.unexpected(rule))
    }

    /// Takes the next token, which must be a QName.
    fn expect_name(&mut self, rule: &str) -> Result<QName> {
        if let Some(Tok::Name(name)) = self.peek(0) {
            let name = name.clone();
            self.ahead.pop_front();
            return Ok(name);
        }
        Err(self.unexpected(rule))
    }

    /// Takes the next token, which must be an `NCName`: a name with no
    /// prefix.
    fn expect_ncname(&mut self, rule: &str) -> Result<String> {
        if let Some(Tok::Name(QName { prefix: None, local })) = self.peek(0) {
            let local = local.clone();
            self.ahead.pop_front();
            return Ok(local);
        }
        Err(self.unexpected(rule))
    }

    /// Takes the next token, which must be a string literal: a
    /// `StringLiteral` or a `URILiteral` [XQuery Appendix A.1 \[140\]].
    fn expect_string(&mut self, rule: &str) -> Result<String> {
        if let Some(Tok::String(value)) = self.peek(0) {
            let value = value.clone();
            self.ahead.pop_front();
            return Ok(value);
        }
        Err(self.unexpected(rule))
    }

    /// Takes `$` and the variable's QName after it [XQuery Appendix A.1
    /// \[87\], \[88\]].
    fn var_name(&mut self, rule: &str) -> Result<QName> {
        self.expect_sym(Sym::Dollar, rule)?;
        self.expect_name(rule)
    }

    /// The syntax error of the next token, which is not what `rule`, a
    /// production cited, requires; or the error reading it raised.
    fn unexpected(&mut self, rule: &str) -> SqlError {
        let at = self.here();
        let found = match self.peek_token(0).map(|t| &t.tok) {
            Some(Err(error)) => return (**error).clone(),
            Some(Ok(tok)) => described(tok),
            None => "the end".to_owned(),
        };
        self.lexer.error(at, &format!("{found} is not what the grammar allows here: {rule}"))
    }

    /// Goes on reading at `at` in another state: the tokens looked ahead
    /// to are dropped, as the lexer reads what follows `at` again.
    fn reread_from(&mut self, at: usize) {
        self.ahead.clear();
        self.lexer.seek(at);
    }

    /// Runs `parse` one level deeper, refusing to past `MAX_NESTING`
    /// (RULES.md R-CF-6). A failure deepens nothing, so the depth needs no
    /// restoring on one.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.depth >= MAX_NESTING {
            let at = self.here();
            return Err(too_deep(self.lexer.place(at)));
        }
        self.depth = self.depth.saturating_add(1);
        let result = parse(self);
        self.depth = self.depth.saturating_sub(1);
        result
    }
}

/// A token, described for a message.
fn described(tok: &Tok) -> String {
    match tok {
        Tok::Name(QName { prefix: Some(prefix), local }) => format!("the name '{prefix}:{local}'"),
        Tok::Name(QName { prefix: None, local }) => format!("the name '{local}'"),
        Tok::PrefixWildcard(prefix) => format!("'{prefix}:*'"),
        Tok::LocalWildcard(local) => format!("'*:{local}'"),
        Tok::Integer(text) | Tok::Decimal(text) | Tok::Double(text) => format!("the number {text}"),
        Tok::String(_) => "a string literal".to_owned(),
        Tok::Pragma(_) => "a pragma".to_owned(),
        Tok::Sym(sym) => format!("'{}'", super::lexer::symbol_text(*sym)),
        Tok::End => "the end of the query".to_owned(),
    }
}

#[cfg(test)]
mod tests;
