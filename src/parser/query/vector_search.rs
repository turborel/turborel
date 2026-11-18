//! \[EXTENSION\] The table function VECTOR_SEARCH (the vector design
//! document §9.3), in the standard's notation:
//!
//! `<vector search>` ::= VECTOR_SEARCH ( `<search argument>` \[{,
//! `<search argument>`}...\] ) \[AS\] `<correlation name>`
//!
//! `<search argument>` ::= TABLE = `<table name>` \[AS `<correlation
//! name>`\] | COLUMN = `<column name>` | SIMILAR_TO = `<value expression>`
//! | METRIC = `<character string literal>` | TOP_N = `<fetch first
//! quantity>`
//!
//! It is read as the derived table it stands for, its tokens put in place
//! of its own:
//!
//! ( SELECT a.*, VECTOR_DISTANCE(a.c, (q), m) AS DISTANCE FROM t AS a
//! ORDER BY DISTANCE FETCH APPROX FIRST n ROWS ONLY ) AS s
//!
//! so each row is the table's columns and its distance, under the
//! correlation name `s`, or `a` if none is written, as `s.ID` and
//! `s.DISTANCE`. TABLE, COLUMN, SIMILAR_TO and TOP_N are each written
//! once, METRIC at most once, in any order.

use crate::ast::TableReference;
use crate::error::Result;
use crate::lexer::{Tok, Token, delim, kw, lex};
use crate::parser::Parser;

/// The function's name.
const VECTOR_SEARCH: &str = "VECTOR_SEARCH";
/// The argument of the query vector.
const SIMILAR_TO: &str = "SIMILAR_TO";
/// The argument of the metric.
const METRIC: &str = "METRIC";
/// The argument of the count of rows.
const TOP_N: &str = "TOP_N";

/// The source text of each argument, as written.
#[derive(Default)]
struct Arguments {
    table: Option<(String, Option<String>)>,
    column: Option<String>,
    similar_to: Option<String>,
    metric: Option<String>,
    top_n: Option<String>,
}

impl Parser {
    /// Whether a `<vector search>` begins here.
    pub(super) fn at_vector_search(&self) -> bool {
        self.is_word(VECTOR_SEARCH) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN)
    }

    /// A `<vector search>`, read as the derived table it stands for.
    pub(super) fn vector_search(&mut self) -> Result<TableReference> {
        let (first, start) = (self.pos, self.here());
        self.advance();
        self.advance();
        let mut given = Arguments::default();
        loop {
            self.search_argument(&mut given)?;
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let named = self.correlation()?;
        let text = self.searched(given, named)?;
        let end = self.token_at(0).start;
        let tokens: Vec<Token> =
            lex(&text)?.into_iter().filter(|t| !matches!(t.tok, Tok::End)).map(|t| Token { tok: t.tok, start, end }).collect();
        self.toks.splice(first..self.pos, tokens).for_each(drop);
        self.pos = first;
        self.table_primary()
    }

    /// A `<search argument>`, its text into `given`.
    fn search_argument(&mut self, given: &mut Arguments) -> Result<()> {
        if self.accept_kw(kw::TABLE) {
            let name = self.argument_value(given.table.is_some())?;
            let alias = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { None };
            given.table = Some((name, alias));
            return Ok(());
        }
        let slot = if self.accept_kw(kw::COLUMN) {
            &mut given.column
        } else if self.accept_word(SIMILAR_TO) {
            &mut given.similar_to
        } else if self.accept_word(METRIC) {
            &mut given.metric
        } else if self.accept_word(TOP_N) {
            &mut given.top_n
        } else {
            return Err(self.syntax("expected TABLE, COLUMN, SIMILAR_TO, METRIC or TOP_N"));
        };
        let written = slot.is_some();
        *slot = Some(self.argument_value(written)?);
        Ok(())
    }

    /// = and the text of an argument's value, refused if `written` before:
    /// its tokens up to the comma, right parenthesis or AS that ends it,
    /// outside any parentheses.
    fn argument_value(&mut self, written: bool) -> Result<String> {
        if written {
            return Err(self.rule("[EXTENSION] an argument of VECTOR_SEARCH is written once"));
        }
        self.expect_delim(delim::EQUALS)?;
        let start = self.here();
        let mut depth = 0_usize;
        while !(depth == 0 && self.ends_argument()) {
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
        if text.is_empty() { Err(self.syntax("expected a value")) } else { Ok(text) }
    }

    /// Whether the token at hand ends an argument's value.
    fn ends_argument(&self) -> bool {
        self.is_delim(delim::COMMA) || self.is_delim(delim::RIGHT_PAREN) || self.is_kw(kw::AS)
    }

    /// The text of the derived table `given` stands for, of the
    /// correlation name `named`, if written.
    fn searched(&self, given: Arguments, named: Option<String>) -> Result<String> {
        let missing = |what: &str| self.rule(format!("[EXTENSION] VECTOR_SEARCH names its {what}"));
        let table = given.table.ok_or_else(|| missing("TABLE"))?;
        let column = given.column.ok_or_else(|| missing("COLUMN"))?;
        let query = given.similar_to.ok_or_else(|| missing("SIMILAR_TO"))?;
        let count = given.top_n.ok_or_else(|| missing("TOP_N"))?;
        let (name, alias) = table;
        let alias = alias.unwrap_or_else(|| name.rsplit('.').next().unwrap_or(&name).trim().to_owned());
        let metric = given.metric.map(|m| format!(", {m}")).unwrap_or_default();
        let distance = format!("VECTOR_DISTANCE({alias}.{column}, ({query}){metric})");
        let correlation = named.unwrap_or_else(|| alias.clone());
        Ok(format!(
            "( SELECT {alias}.*, {distance} AS DISTANCE FROM {name} AS {alias} \
             ORDER BY DISTANCE FETCH APPROX FIRST {count} ROWS ONLY ) AS {correlation}"
        ))
    }
}
