//! \[EXTENSION\] [GS §10.1] What a query with a MATCH in its WHERE
//! clause writes of its paths, in SQL Server's syntax:
//!
//! ```text
//! <path table> ::= <table name> FOR PATH [ [ AS ] <correlation name> ]
//! <graph path aggregate> ::= <graph path function> ( <value expression> [ , <separator> ] )
//!     WITHIN GROUP ( GRAPH PATH )
//! <graph path function> ::= STRING_AGG | LAST_VALUE | COUNT | SUM | AVG | MIN | MAX
//! ```
//!
//! A FOR PATH table is a group variable of the pattern, so the FROM
//! clause's tables it is joined with are not crossed with it. Each
//! aggregate is a set function over the group's elements in path order
//! [P1 §4.4], kept as a column of the GRAPH_TABLE the MATCH becomes:
//! STRING_AGG is LISTAGG, and LAST_VALUE the last element of ARRAY_AGG.

use super::MATCH_TABLE;
use crate::ast::{
    ColumnSpecification, Composite, GraphMeasure, Listagg, Overflow, PathTable, SetArg, SetFunctionSpecification, TableName,
    ValueExpression,
};
use crate::error::Result;
use crate::functions::{ARRAY_AGG, CARDINALITY, LISTAGG};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// PATH, of FOR PATH and GRAPH PATH.
const PATH: &str = "PATH";
/// GRAPH, of GRAPH PATH.
const GRAPH: &str = "GRAPH";
/// WITHIN, of WITHIN GROUP.
const WITHIN: &str = "WITHIN";
/// STRING_AGG, LISTAGG in SQL Server's spelling.
const STRING_AGG: &str = "STRING_AGG";
/// LAST_VALUE, of the path's last element.
const LAST_VALUE: &str = "LAST_VALUE";
/// The set functions SQL Server names by key words of SQL.
const KEY_WORD_FUNCTIONS: [&str; 5] = [kw::COUNT, kw::SUM, kw::AVG, kw::MIN, kw::MAX];
/// How far past its function's right parenthesis WITHIN GROUP ( GRAPH
/// PATH ) ends.
const CLAUSE_TOKENS: usize = 6;

impl Parser {
    /// FOR PATH, after a table's name, if written.
    pub(in crate::parser) fn for_path(&mut self) -> bool {
        let written = self.is_kw(kw::FOR) && matches!(self.peek_at(1), Tok::Identifier(w) if w == PATH);
        if written {
            self.advance();
            self.advance();
        }
        written
    }

    /// Notes `table`, exposed as `correlation` if written, as a FOR PATH
    /// table of the query specification being read.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 outside a query specification.
    pub(in crate::parser) fn path_table(&mut self, table: &TableName, correlation: Option<&String>) -> Result<()> {
        let name = correlation.cloned().unwrap_or_else(|| table.ident.clone());
        let frame = self.compat_frame("FOR PATH", table.at)?;
        frame.paths.push(PathTable { table: table.clone(), name });
        Ok(())
    }

    /// The offset of the token just past the right parenthesis matching
    /// the one at offset `open`, if it has one.
    pub(in crate::parser) fn past_closing(&self, open: usize) -> Option<usize> {
        let mut depth = 0_usize;
        let mut n = open;
        loop {
            match self.peek_at(n) {
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(n.saturating_add(1));
                    }
                }
                Tok::End => return None,
                Tok::KeyWord(_)
                | Tok::Identifier(_)
                | Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BinaryString(_)
                | Tok::Introducer(_)
                | Tok::LargeObjectLength(_)
                | Tok::Delimiter(_) => {}
            }
            n = n.saturating_add(1);
        }
    }

    /// The name of the `<graph path function>` here, if a `<graph path
    /// aggregate>` begins here.
    fn graph_path_function(&self) -> Option<&'static str> {
        let name = match self.peek() {
            Tok::Identifier(w) => [STRING_AGG, LAST_VALUE].into_iter().find(|f| w == f),
            Tok::KeyWord(k) => KEY_WORD_FUNCTIONS.into_iter().find(|f| k == f),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        }?;
        if *self.peek_at(1) != Tok::Delimiter(delim::LEFT_PAREN) {
            return None;
        }
        let after = self.past_closing(1)?;
        let word = |n: usize, w: &str| matches!(self.peek_at(after.saturating_add(n)), Tok::Identifier(x) if x == w);
        let clause = word(0, WITHIN) && *self.peek_at(after.saturating_add(1)) == Tok::KeyWord(kw::GROUP);
        let graph = *self.peek_at(after.saturating_add(2)) == Tok::Delimiter(delim::LEFT_PAREN) && word(3, GRAPH) && word(4, PATH);
        (clause && graph).then_some(name)
    }

    /// A `<graph path aggregate>`, if one begins here: the column of the
    /// match it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format; -103 outside a
    /// query specification.
    pub(in crate::parser) fn graph_path_aggregate(&mut self) -> Result<Option<ValueExpression>> {
        let Some(name) = self.graph_path_function() else { return Ok(None) };
        let at = self.here();
        self.advance();
        self.advance();
        let arg = self.value_expression()?;
        let separator = if name == STRING_AGG {
            self.expect_delim(delim::COMMA)?;
            Some(self.character_string_literal("<separator>")?)
        } else {
            None
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        for _ in 0..CLAUSE_TOKENS {
            self.advance();
        }
        let value = measure(name, arg, separator, at);
        let frame = self.compat_frame("WITHIN GROUP (GRAPH PATH)", at)?;
        let column = format!("$PATH{}", frame.measures.len().saturating_add(1));
        frame.measures.push(GraphMeasure { name: column.clone(), value, at });
        let qualifier = Some(TableName { auth: None, ident: MATCH_TABLE.to_owned(), at });
        Ok(Some(ValueExpression::Column(ColumnSpecification { qualifier, column, catalog: None, at })))
    }
}

impl Parser {
    /// \[EXTENSION\] [GS §10.3] PGQL's LISTAGG ( x \[ , separator \] ), of
    /// no WITHIN GROUP, if one begins here: LISTAGG in no order but that
    /// of a path's elements, its separator none if not written.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format.
    pub(in crate::parser) fn pgql_listagg(&mut self) -> Result<Option<ValueExpression>> {
        let named = matches!(self.peek(), Tok::Identifier(w) if w == LISTAGG) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN);
        let Some(after) = named.then(|| self.past_closing(1)).flatten() else { return Ok(None) };
        if matches!(self.peek_at(after), Tok::Identifier(w) if w == WITHIN) {
            return Ok(None);
        }
        let at = self.here();
        self.advance();
        self.advance();
        let arg = self.value_expression()?;
        let separator = if self.accept_delim(delim::COMMA) { self.character_string_literal("<separator>")? } else { String::new() };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(set_function(LISTAGG, arg, Some(separator), at)))
    }
}

/// The set function `name` over `arg`, of LISTAGG's `separator`.
fn set_function(name: &'static str, arg: ValueExpression, separator: Option<String>, at: usize) -> ValueExpression {
    let listagg = separator.map(|separator| Box::new(Listagg { separator, overflow: Overflow::Error }));
    let arg = SetArg::All(Box::new(arg));
    let f = SetFunctionSpecification {
        name,
        arg,
        order_by: Vec::new(),
        listagg,
        json: None,
        filter: None,
        semantics: None,
        returning: None,
        at,
    };
    ValueExpression::SetFunction(f)
}

/// The GPML value of the `<graph path function>` `name` of `arg`.
fn measure(name: &'static str, arg: ValueExpression, separator: Option<String>, at: usize) -> ValueExpression {
    if name == STRING_AGG {
        return set_function(LISTAGG, arg, separator, at);
    }
    if name != LAST_VALUE {
        return set_function(name, arg, None, at);
    }
    let all = set_function(ARRAY_AGG, arg, None, at);
    let last = ValueExpression::Function { name: CARDINALITY, args: vec![all.clone()], at };
    ValueExpression::Composite(Box::new(Composite::Element(Box::new(all), Box::new(last))), at)
}
