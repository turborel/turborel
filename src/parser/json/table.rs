//! [ISO/IEC 9075-2:2023 §7.11] `<JSON table>`, a `<table primary>` [§7.6]
//! with its `<correlation or recognition>`.
//!
//! JSON_TABLE is no key word of SQL-92, so it is the table only before
//! "(", as the query operators are; and every word of its Format that is
//! none either, as COLUMNS, NESTED, PATH and PLAN, is read as that word
//! only where the Format has it. The nesting of columns and of plans
//! counts toward the statement's depth (`Parser::nested`).

use super::super::Parser;
use super::super::composite::ORDINALITY;
use super::super::modules::PATH;
use super::{EMPTY, ERROR, FORMAT, PASSING};
use crate::ast::{
    JsonCommon, JsonTable, JsonTableColumn, JsonTableColumnKind, JsonTableFormatted, JsonTableNamed, JsonTableNested, JsonTablePath,
    JsonTablePlan, JsonTablePlanClause, JsonValueBehavior, TableReference,
};
use crate::error::Result;
use crate::json::{QueryBehavior, TableErrorBehavior};
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 9075-2:2023 §5.2, §7.11] The words of the Format that are no
// key words of SQL-92.
/// JSON_TABLE, the table's name.
const JSON_TABLE: &str = "JSON_TABLE";
/// COLUMNS, of a `<JSON table columns clause>`.
const COLUMNS: &str = "COLUMNS";
/// NESTED, of `<JSON table nested columns>`.
const NESTED: &str = "NESTED";
/// PLAN, of a `<JSON table plan clause>`.
const PLAN: &str = "PLAN";

/// A behavior of a column ON EMPTY or ON ERROR, of either kind of column,
/// as read.
enum ColumnBehavior {
    /// One a regular column may have.
    Regular(JsonValueBehavior),
    /// EMPTY ARRAY or EMPTY OBJECT, which only a formatted column has.
    Formatted(QueryBehavior),
}

impl Parser {
    /// Whether a `<JSON table>` begins here: JSON_TABLE before "(".
    pub(in crate::parser) fn at_json_table(&self) -> bool {
        self.at_json(JSON_TABLE)
    }

    /// [§7.11] `<JSON table>` ::= JSON_TABLE ( `<JSON API common syntax>`
    /// `<JSON table columns clause>` \[`<JSON table plan clause>`\]
    /// \[`<JSON table error behavior>` ON ERROR\] ), then [§7.6] its
    /// `<correlation or recognition>`, \[AS\] `<correlation name>`
    /// \[(`<derived column list>`)\].
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if it does not conform to the Format; -900 of a
    /// `<row pattern recognition clause and name>`, which a JSON table
    /// does not take here.
    pub(in crate::parser) fn json_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let (common, name) = self.json_common_named()?;
        let columns = self.json_table_columns()?;
        let plan = if self.accept_word(PLAN) { Some(self.json_table_plan_clause()?) } else { None };
        let on_error = self.table_error_behavior()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let Some(correlation) = self.correlation()? else {
            if self.at_match_recognize() {
                return Err(self.unsupported("a <row pattern recognition clause and name> of a <JSON table>"));
            }
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §7.6 F] a <JSON table> has a correlation name"));
        };
        let derived = self.derived_columns()?;
        let table = JsonTable { common, name, columns, plan, on_error, correlation, derived, at };
        Ok(TableReference::Json(Box::new(table)))
    }

    /// [§10.14] `<JSON API common syntax>` ::= `<JSON context item>` ,
    /// `<JSON path specification>` \[AS `<JSON table path name>`\]
    /// \[`<JSON passing clause>`\], of a JSON table, which alone has the
    /// path name \[SR2\].
    fn json_common_named(&mut self) -> Result<(JsonCommon, Option<(String, usize)>)> {
        let context = self.json_input()?;
        self.expect_delim(delim::COMMA)?;
        let path = self.json_table_path()?;
        let name = self.path_name()?;
        let mut passing = Vec::new();
        if self.accept_word(PASSING) {
            passing.push(self.json_argument()?);
            while self.accept_delim(delim::COMMA) {
                passing.push(self.json_argument()?);
            }
        }
        Ok((JsonCommon { context, path: path.text, path_at: path.at, passing }, name))
    }

    /// \[AS `<JSON table path name>`\], and where the name is written.
    fn path_name(&mut self) -> Result<Option<(String, usize)>> {
        if !self.accept_kw(kw::AS) {
            return Ok(None);
        }
        let at = self.here();
        Ok(Some((self.identifier()?, at)))
    }

    /// [§7.11] \[`<JSON table error behavior>` ON ERROR\]: ERROR or EMPTY.
    fn table_error_behavior(&mut self) -> Result<Option<TableErrorBehavior>> {
        let behavior = if self.accept_word(ERROR) {
            TableErrorBehavior::Error
        } else if self.accept_word(EMPTY) {
            TableErrorBehavior::Empty
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::ON)?;
        self.expect_word(ERROR)?;
        Ok(Some(behavior))
    }

    /// [§7.11] `<JSON table columns clause>` ::= COLUMNS ( `<JSON table
    /// column definition>` \[, ...\] ).
    fn json_table_columns(&mut self) -> Result<Vec<JsonTableColumn>> {
        self.expect_word(COLUMNS)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![self.json_table_column()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.json_table_column()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// [§7.11] `<JSON table column definition>`: nested columns, NESTED
    /// before PATH or a path specification; else a column of a name.
    fn json_table_column(&mut self) -> Result<JsonTableColumn> {
        let nested = self.is_word(NESTED) && matches!(self.peek_at(1), Tok::Identifier(w) if w == PATH);
        if nested || (self.is_word(NESTED) && matches!(self.peek_at(1), Tok::CharacterString(_))) {
            return self.nested(Self::json_table_nested).map(|n| JsonTableColumn::Nested(Box::new(n)));
        }
        let at = self.here();
        let name = self.identifier()?;
        if self.accept_kw(kw::FOR) {
            self.expect_word(ORDINALITY)?;
            return Ok(JsonTableColumn::Column(JsonTableNamed { name, at, kind: JsonTableColumnKind::Ordinality }));
        }
        let kind = self.json_table_typed()?;
        Ok(JsonTableColumn::Column(JsonTableNamed { name, at, kind }))
    }

    /// [§7.11] `<JSON table nested columns>` ::= NESTED \[PATH\] `<JSON
    /// table nested path specification>` \[AS `<JSON table nested path
    /// name>`\] `<JSON table columns clause>`, NESTED next.
    fn json_table_nested(&mut self) -> Result<JsonTableNested> {
        self.advance();
        self.accept_word(PATH);
        let path = self.json_table_path()?;
        let name = self.path_name()?;
        let columns = self.json_table_columns()?;
        Ok(JsonTableNested { path, name, columns })
    }

    /// A `<JSON path specification>`, a `<character string literal>`.
    fn json_table_path(&mut self) -> Result<JsonTablePath> {
        let at = self.here();
        let Tok::CharacterString(text) = self.peek().clone() else {
            return Err(self.syntax("expected a <JSON path specification>, a <character string literal>"));
        };
        self.advance();
        Ok(JsonTablePath { text, at })
    }

    /// [§7.11] The rest of a `<JSON table regular column definition>` or
    /// `<JSON table formatted column definition>`, from its `<data type>`:
    /// formatted if it has a `<JSON representation>`, a wrapper or quotes
    /// behavior, or EMPTY ARRAY or EMPTY OBJECT.
    fn json_table_typed(&mut self) -> Result<JsonTableColumnKind> {
        let data_type = self.type_name()?;
        let format = if self.accept_word(FORMAT) { Some(self.json_representation()?) } else { None };
        let path = if self.accept_word(PATH) { Some(self.json_table_path()?) } else { None };
        let wrapper = self.wrapper_behavior()?;
        let quotes_at = self.here();
        let quotes = self.quotes_behavior()?.map(|q| (q, quotes_at));
        let (on_empty, on_error) = self.behaviors(Self::column_behavior)?;
        let formatted = format.is_some() || wrapper.is_some() || quotes.is_some();
        let empties = [&on_empty, &on_error].into_iter().flatten().any(|b| matches!(b, ColumnBehavior::Formatted(_)));
        if !formatted && !empties {
            return Ok(JsonTableColumnKind::Regular {
                data_type,
                path,
                on_empty: on_empty.and_then(regular),
                on_error: on_error.and_then(regular),
            });
        }
        let on_empty = self.formatted_behavior(on_empty.as_ref())?;
        let on_error = self.formatted_behavior(on_error.as_ref())?;
        let f = JsonTableFormatted { data_type, format, path, wrapper, quotes, on_empty, on_error };
        Ok(JsonTableColumnKind::Formatted(Box::new(f)))
    }

    /// A behavior of a formatted column, none if none is written.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 of DEFAULT, which only a regular column has.
    fn formatted_behavior(&self, behavior: Option<&ColumnBehavior>) -> Result<Option<QueryBehavior>> {
        match behavior {
            None => Ok(None),
            Some(ColumnBehavior::Formatted(b)) => Ok(Some(*b)),
            Some(ColumnBehavior::Regular(JsonValueBehavior::Error)) => Ok(Some(QueryBehavior::Error)),
            Some(ColumnBehavior::Regular(JsonValueBehavior::Null)) => Ok(Some(QueryBehavior::Null)),
            Some(ColumnBehavior::Regular(JsonValueBehavior::Default(_))) => Err(self.syntax(
                "[ISO/IEC 9075-2:2023 §7.11 F] a <JSON table formatted column definition> has no DEFAULT behavior, only a regular column has one",
            )),
        }
    }

    /// [§7.11] A behavior of a column ON EMPTY or ON ERROR, if one is next:
    /// ERROR, NULL, DEFAULT `<value expression>`, EMPTY ARRAY or EMPTY
    /// OBJECT.
    fn column_behavior(&mut self) -> Result<Option<ColumnBehavior>> {
        if self.is_word(EMPTY) && matches!(self.peek_at(1), Tok::KeyWord(k) if *k == kw::ON) {
            return Ok(None);
        }
        if self.is_word(EMPTY) {
            return Ok(self.query_behavior()?.map(ColumnBehavior::Formatted));
        }
        Ok(self.value_behavior()?.map(ColumnBehavior::Regular))
    }

    /// [§7.11] `<JSON table plan clause>`, PLAN read: `( <JSON table plan>
    /// )` or DEFAULT ( `<JSON table default plan choices>` ).
    fn json_table_plan_clause(&mut self) -> Result<JsonTablePlanClause> {
        if !self.accept_kw(kw::DEFAULT) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let plan = self.json_table_plan()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(JsonTablePlanClause::Specific(plan));
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let (mut inner, mut cross) = (None, None);
        loop {
            if inner.is_none() && (self.is_kw(kw::INNER) || self.is_kw(kw::OUTER)) {
                inner = Some(self.accept_kw(kw::INNER) || !self.accept_kw(kw::OUTER));
            } else if cross.is_none() && (self.is_kw(kw::UNION) || self.is_kw(kw::CROSS)) {
                cross = Some(self.accept_kw(kw::CROSS) || !self.accept_kw(kw::UNION));
            } else {
                return Err(self.syntax("expected INNER, OUTER, UNION or CROSS"));
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(JsonTablePlanClause::Default { inner: inner.unwrap_or(false), cross: cross.unwrap_or(false) })
    }

    /// [§7.11] `<JSON table plan>`: a path name, alone or the parent of
    /// OUTER or INNER, or primaries of UNION or of CROSS.
    fn json_table_plan(&mut self) -> Result<JsonTablePlan> {
        let first = self.json_table_plan_primary()?;
        if let JsonTablePlan::Name(name, at) = &first {
            let inner = self.is_kw(kw::INNER);
            if inner || self.is_kw(kw::OUTER) {
                self.advance();
                let child = Box::new(self.json_table_plan_primary()?);
                return Ok(JsonTablePlan::Parent { name: name.clone(), at: *at, inner, child });
            }
        }
        let cross = self.is_kw(kw::CROSS);
        if !cross && !self.is_kw(kw::UNION) {
            return Ok(first);
        }
        let word = if cross { kw::CROSS } else { kw::UNION };
        let mut operands = vec![first];
        while self.accept_kw(word) {
            operands.push(self.json_table_plan_primary()?);
        }
        Ok(JsonTablePlan::Siblings { cross, operands })
    }

    /// [§7.11] `<JSON table plan primary>` ::= `<JSON table path name>` |
    /// ( `<JSON table plan>` ).
    fn json_table_plan_primary(&mut self) -> Result<JsonTablePlan> {
        if self.accept_delim(delim::LEFT_PAREN) {
            let plan = self.nested(Self::json_table_plan)?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(plan);
        }
        let at = self.here();
        Ok(JsonTablePlan::Name(self.identifier()?, at))
    }
}

/// A behavior read of a column that is regular.
fn regular(behavior: ColumnBehavior) -> Option<JsonValueBehavior> {
    match behavior {
        ColumnBehavior::Regular(b) => Some(b),
        ColumnBehavior::Formatted(_) => None,
    }
}
