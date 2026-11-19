//! [ISO/IEC 9075-2:2023 §7.6] `<data change delta table>` ::= `<result
//! option>` TABLE ( `<data change statement>` ), and \[EXTENSION\] a data
//! change statement with RETURNING `<select list>`, the query of the rows
//! it changed (RULES.md D183, D184).

use super::Parser;
use super::data_statements::MERGE;
use crate::ast::{DeltaResult, DeltaRows, DeltaTable, QueryExpression, QuerySpecification, SqlStatement, TableExpression, TableReference};
use crate::error::Result;
use crate::lexer::{Tok, Token, delim, kw};

/// \[EXTENSION\] The word after a data change statement that begins its
/// select list (RULES.md D184).
const RETURNING: &str = "RETURNING";

/// The `<result option>` `tok` is, if it is one.
fn result_option(tok: &Tok) -> Option<DeltaResult> {
    let Tok::Identifier(w) = tok else { return None };
    match w.as_str() {
        "OLD" => Some(DeltaResult::Old),
        "NEW" => Some(DeltaResult::New),
        "FINAL" => Some(DeltaResult::Final),
        _ => None,
    }
}

/// Whether `toks` begin a data change statement: INSERT, UPDATE, DELETE,
/// or MERGE INTO.
fn begins_change(toks: &[Token]) -> bool {
    match toks {
        [Token { tok: Tok::KeyWord(kw::INSERT | kw::UPDATE | kw::DELETE), .. }, ..] => true,
        [Token { tok: Tok::Identifier(w), .. }, Token { tok: Tok::KeyWord(kw::INTO), .. }, ..] => w == MERGE,
        _ => false,
    }
}

/// The place in `toks` of the RETURNING outside every parenthesis, if
/// one is.
fn returning_in(toks: &[Token]) -> Option<usize> {
    let mut depth = 0_usize;
    for (k, t) in toks.iter().enumerate() {
        depth = deepened(depth, &t.tok);
        if let Tok::Identifier(w) = &t.tok
            && depth == 0
            && w == RETURNING
        {
            return Some(k);
        }
    }
    None
}

/// \[EXTENSION\] Whether `toks`, a statement's, are a data change
/// statement with RETURNING: a query, its rows a cursor's (RULES.md D184).
pub(crate) fn begins_returning(toks: &[Token]) -> bool {
    begins_change(toks) && returning_in(toks).is_some()
}

/// The place in `toks`, from `from`, at a left parenthesis, of the right
/// parenthesis that closes it, if one does.
fn closing(toks: &[Token], from: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (k, t) in toks.iter().enumerate().skip(from) {
        depth = deepened(depth, &t.tok);
        if depth == 0 {
            return Some(k);
        }
    }
    None
}

/// The depth of parentheses after `tok`, at `depth` before it.
fn deepened(depth: usize, tok: &Tok) -> usize {
    if *tok == Tok::Delimiter(delim::LEFT_PAREN) {
        depth.saturating_add(1)
    } else if *tok == Tok::Delimiter(delim::RIGHT_PAREN) {
        depth.saturating_sub(1)
    } else {
        depth
    }
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §7.6] A `<data change delta table>` and its
    /// `<correlation name>` and `<derived column list>`, if one begins here.
    pub(super) fn delta_table(&mut self) -> Result<Option<TableReference>> {
        let Some(result) = result_option(self.peek()) else { return Ok(None) };
        if !self.next_is_kw(kw::TABLE) || *self.peek_at(2) != Tok::Delimiter(delim::LEFT_PAREN) {
            return Ok(None);
        }
        let at = self.here();
        self.advance();
        self.advance();
        let Some(close) = closing(&self.toks, self.pos) else {
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §7.6] expected ) after the <data change statement>"));
        };
        self.advance();
        let statement = Box::new(self.nested(|p| p.change_until(close))?);
        self.expect_delim(delim::RIGHT_PAREN)?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        let rows = DeltaRows::default();
        Ok(Some(TableReference::Delta(Box::new(DeltaTable { result, statement, correlation, columns, at, rows }))))
    }

    /// \[EXTENSION\] A data change statement with RETURNING `<select
    /// list>`, if one begins here: the query of the rows it changed, as
    /// SELECT `<select list>` FROM a delta table of them (RULES.md D184).
    pub(super) fn returning(&mut self) -> Result<Option<QueryExpression>> {
        let rest = self.toks.get(self.pos..).unwrap_or_default();
        let Some(at) = returning_in(rest).filter(|_| begins_change(rest)).map(|k| k.saturating_add(self.pos)) else {
            return Ok(None);
        };
        let start = self.here();
        let statement = Box::new(self.nested(|p| p.change_until(at))?);
        // RETURNING.
        self.advance();
        let select_list = self.select_list()?;
        let (correlation, columns, rows) = (None, None, DeltaRows::default());
        let delta = DeltaTable { result: DeltaResult::Changed, statement, correlation, columns, at: start, rows };
        let table_expression = TableExpression {
            from: vec![TableReference::Delta(Box::new(delta))],
            where_clause: None,
            group_by: None,
            having: None,
            window: Vec::new(),
        };
        Ok(Some(QueryExpression::Spec(Box::new(QuerySpecification { distinct: false, select_list, table_expression }))))
    }

    /// The `<data change statement>` from here to the token at `end`, read
    /// as a statement of its own: INSERT, UPDATE, DELETE or MERGE.
    fn change_until(&mut self, end: usize) -> Result<SqlStatement> {
        let inner: Vec<Token> = self.toks.get(self.pos..end).unwrap_or_default().iter().cloned().chain([self.end.clone()]).collect();
        let outer = std::mem::replace(&mut self.toks, inner);
        let pos = std::mem::replace(&mut self.pos, 0);
        // What was read is kept by the place of its token, and these tokens'
        // places are their own.
        let values = std::mem::take(&mut self.read_values);
        let read = self.data_change();
        self.read_values = values;
        self.toks = outer;
        self.pos = if read.is_ok() { end } else { pos };
        read
    }

    /// A `<data change statement>` read whole from here.
    fn data_change(&mut self) -> Result<SqlStatement> {
        let statement = if self.is_kw(kw::INSERT) {
            SqlStatement::Insert(self.insert_statement()?)
        } else if self.is_kw(kw::UPDATE) {
            self.update_statement()?
        } else if self.is_kw(kw::DELETE) {
            self.delete_statement()?
        } else if matches!(self.peek(), Tok::Identifier(w) if w == MERGE) {
            self.merge_statement()?
        } else {
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §7.6] expected a <data change statement>: INSERT, UPDATE, DELETE or MERGE"));
        };
        if *self.peek() != Tok::End {
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §7.6] expected the end of the <data change statement>"));
        }
        Ok(statement)
    }
}
