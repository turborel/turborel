//! \[EXTENSION\] CHUNK_TEXT as a table in FROM (the vector design's §12.3),
//! in the standard's notation:
//!
//! `<chunk table>` ::= { CHUNK_TEXT | VECTOR_CHUNKS | AI_GENERATE_CHUNKS }
//! ( `<value expression>` \[, `<chunk specification>`\] ) \[\[AS\]
//! `<correlation name>` \[( `<derived column list>` )\]\]
//!
//! `<chunk specification>` ::= `<character string literal>` | BY `<chunk
//! unit>` \[,\] MAX `<unsigned integer>` \[\[,\] OVERLAP `<unsigned
//! integer>`\] \[\[,\] SPLIT_ON `<character string literal>`\] \[\[,\]
//! NORMALIZE WHITESPACE\]
//!
//! It is read as the collection derived table it stands for, its tokens
//! put in place of its own:
//!
//! UNNEST ( CHUNK_TEXT ( `<value expression>` , 'spec' ) ) AS c (
//! CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT )
//!
//! its chunking CHUNK_TEXT's default if none is written, its correlation
//! name the function's if none is, its columns those, unless a derived
//! column list names them.

use crate::ast::TableReference;
use crate::error::Result;
use crate::lexer::{Tok, Token, delim, lex};
use crate::parser::Parser;
use crate::vector::chunk::{Spec, quote};
use crate::vector::embed::CHUNK_NAMES;

/// The columns of a chunk's row.
const COLUMNS: &str = "CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT";

impl Parser {
    /// Whether a `<chunk table>` begins here.
    pub(super) fn at_chunk_table(&self) -> bool {
        let named = CHUNK_NAMES.iter().any(|n| self.is_word(n));
        named && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN)
    }

    /// The text of the tokens from here to the comma or right parenthesis
    /// that ends them, outside any parentheses.
    fn chunk_argument(&mut self) -> Result<String> {
        let start = self.here();
        let mut depth = 0_usize;
        while depth > 0 || !(self.is_delim(delim::COMMA) || self.is_delim(delim::RIGHT_PAREN)) {
            if matches!(self.peek(), Tok::End) {
                return Err(self.syntax("expected \")\""));
            }
            if self.is_delim(delim::LEFT_PAREN) {
                depth = depth.saturating_add(1);
            } else if self.is_delim(delim::RIGHT_PAREN) {
                depth = depth.saturating_sub(1);
            }
            self.advance();
        }
        let text = self.text_from(start);
        if text.trim().is_empty() { Err(self.syntax("expected a value")) } else { Ok(text) }
    }

    /// A `<chunk specification>`, as CHUNK_TEXT reads it, up to the right
    /// parenthesis that ends it.
    fn chunk_specification(&mut self) -> Result<String> {
        if let (Tok::CharacterString(spec), Tok::Delimiter(d)) = (self.peek().clone(), self.peek_at(1))
            && *d == delim::RIGHT_PAREN
        {
            self.advance();
            return Ok(Spec::parse(&spec)?.written());
        }
        let start = self.here();
        while !self.is_delim(delim::RIGHT_PAREN) {
            if matches!(self.peek(), Tok::End) {
                return Err(self.syntax("expected \")\""));
            }
            self.advance();
        }
        let words = self.text_from(start).replace(',', " ");
        Ok(Spec::parse(&words)?.written())
    }

    /// A `<chunk table>`, read as the collection derived table it stands
    /// for.
    pub(super) fn chunk_table(&mut self) -> Result<TableReference> {
        let (first, start) = (self.pos, self.here());
        let name = self.identifier()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let text = self.chunk_argument()?;
        let spec = if self.accept_delim(delim::COMMA) { self.chunk_specification()? } else { Spec::default().written() };
        self.expect_delim(delim::RIGHT_PAREN)?;
        let named = self.correlation()?.unwrap_or_else(|| name.clone());
        let correlation = format!("\"{}\"", named.replace('"', "\"\""));
        let columns = if self.is_delim(delim::LEFT_PAREN) { String::new() } else { format!(" ({COLUMNS})") };
        let made = format!("UNNEST({name}({text}, {})) AS {correlation}{columns}", quote(&spec));
        let end = self.token_at(0).start;
        let tokens: Vec<Token> =
            lex(&made)?.into_iter().filter(|t| !matches!(t.tok, Tok::End)).map(|t| Token { tok: t.tok, start, end }).collect();
        self.toks.splice(first..self.pos, tokens).for_each(drop);
        self.pos = first;
        self.table_primary()
    }
}
