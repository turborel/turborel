//! Recursive descent parser. One method per BNF production, named after it.
//!
//! The standard's left-recursive rules ("`<term>` ::= `<factor>` | `<term>` * `<factor>`")
//! become loops, which gives the left-to-right evaluation order of [X3.135-1986 §5.9 GR7]
//! and [X3.135-1986 §5.18 GR3]. The one real ambiguity in the grammar, that both
//! `<predicate>` and (`<search condition>`) can start with "(", is resolved by
//! trying `<predicate>` first and backtracking (\[DESIGN\] PEG-style ordered choice).
//!
//! Syntax that is valid SQL but not implemented reports `Code::Unsupported`
//! rather than a misleading syntax error.

pub use domains::VALUE_COLUMN;
use std::collections::HashSet;

use crate::ast::{
    ColumnName, ColumnSpecification, InsertSource, InsertStatement, ParameterSpecification, QuerySpecification, SetClause, SqlStatement,
    TableName, UpdateSource,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, Token, delim, kw, lex};

mod constraints;
mod cursor;
mod data_type;
mod datetime;
mod ddl;
mod diagnostics;
mod domains;
mod expression;
mod interval;
mod literal;
mod primary;
mod query;
mod schema;
mod session;
mod users;

/// \[DESIGN\] The deepest syntax tree the parser builds. Each nested `<search
/// condition>`, `<boolean term>`, `<value expression>`, `<term>` or `<query expression>`
/// counts one level, and so does each further operator of a chain (`OR`, `AND`,
/// `+`, `-`, `*`, `/`, `UNION`), since a chain builds one tree level per operator,
/// and each `<table reference>` after the first, since the plan joins them one
/// level each. Rewrites gather conjuncts with `logical::and_all`, which builds
/// a balanced tree, so no plan gets much deeper than the statement.
///
/// X3.135-1986 specifies no such limit ([X3.135-1986 §3.4] requires conforming
/// language to be processed according to the General Rules), so a deeper statement
/// is valid SQL-86 that this implementation does not process: SQLCODE -900. The
/// limit keeps every later recursive walk of the tree (binding, optimization,
/// execution, drop) within the stack (RULES.md R-CF-6).
///
/// Measured on 2026-10-23 (debug build, the worst case): at this limit the
/// deepest accepted statement of each shape needs under 512 KiB of stack, and a
/// chain of `UNION`s under 1 MiB, half of Rust's default 2 MiB thread stack.
/// `nesting_beyond_the_limit_is_rejected_not_a_crash` checks this on a 1 MiB thread.
pub const MAX_NESTING_DEPTH: usize = 64;

/// A parser over one piece of SQL text, which it lexes first.
pub struct Parser {
    toks: Vec<Token>,
    /// The `Tok::End` that `lex` puts last, also returned for any position
    /// past the last token, so no lookahead can go out of bounds.
    end: Token,
    pos: usize,
    /// Levels of the syntax tree above the construct being parsed (`MAX_NESTING_DEPTH`).
    depth: usize,
    /// \[EXTENSION\] Accept the function call syntax `<identifier>`(`<args>`).
    extensions: bool,
    /// The `<parameter name>`s declared for this statement [X3.135-1986 §7.3].
    params: HashSet<String>,
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §6.2 SR2] For a statement being
    /// prepared, the `<dynamic parameter specification>`s read so far; none
    /// for any other, where one is refused.
    dynamic: Option<usize>,
    /// The source, for keeping a `<cursor specification>` as text.
    src: Vec<char>,
    /// In a domain constraint, where VALUE may be written [ISO/IEC
    /// 9075:1992 §6.2 SR5].
    in_domain: bool,
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::KeyWord(k) => k.to_string(),
        Tok::Identifier(s) | Tok::ExactNumeric(s) | Tok::ApproximateNumeric(s) => s.clone(),
        Tok::CharacterString(s) => format!("'{s}'"),
        Tok::BitString(b) => format!("B'{b}'"),
        Tok::Delimiter(d) => format!("\"{d}\""),
        Tok::End => "end of statement".into(),
    }
}

/// When two alternatives both fail, report the one that got further.
fn furthest(a: SqlError, b: SqlError) -> SqlError {
    let (pa, pb) = (a.pos.unwrap_or(0), b.pos.unwrap_or(0));
    if pb > pa || (pb == pa && b.code == Code::Unsupported) { b } else { a }
}

impl Parser {
    /// A parser for standard SQL-86 only.
    ///
    /// # Errors
    ///
    /// `src` is not made of `<token>`s and `<separator>`s [X3.135-1986 §5.3].
    pub fn new(src: &str) -> Result<Parser> {
        Self::with_extensions(src, false)
    }

    /// A parser that also accepts the \[EXTENSION\] function call syntax if
    /// `extensions` is set.
    ///
    /// # Errors
    ///
    /// As for `new`.
    pub fn with_extensions(src: &str, extensions: bool) -> Result<Parser> {
        Self::for_procedure(src, extensions, HashSet::new())
    }

    /// A parser for a statement whose procedure declares `params`.
    ///
    /// # Errors
    ///
    /// As for `new`.
    pub fn for_procedure(src: &str, extensions: bool, params: HashSet<String>) -> Result<Parser> {
        let toks = lex(src)?;
        let chars: Vec<char> = src.chars().collect();
        let end = Token { tok: Tok::End, start: chars.len(), end: chars.len() };
        Ok(Parser { toks, end, pos: 0, depth: 0, extensions, params, dynamic: None, src: chars, in_domain: false })
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6] A parser for a `<preparable
    /// statement>`, in which `<dynamic parameter specification>`s may be
    /// written [§6.2 SR2].
    ///
    /// # Errors
    ///
    /// As for `new`.
    pub fn for_prepare(src: &str, extensions: bool) -> Result<Parser> {
        let mut parser = Self::with_extensions(src, extensions)?;
        parser.dynamic = Some(0);
        Ok(parser)
    }

    /// The `<dynamic parameter specification>`s read so far, of a
    /// statement being prepared.
    pub fn dynamic_parameters(&self) -> usize {
        self.dynamic.unwrap_or(0)
    }

    /// [X3.135-1986 §7.3 SR4] a column whose name is a parameter's must be
    /// qualified, so an unqualified declared name is the parameter.
    fn at_parameter(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(n) if self.params.contains(n)) && !matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD))
    }

    // ----- token helpers -----

    /// The token `n` places after the next one, or the end.
    fn token_at(&self, n: usize) -> &Token {
        self.toks.get(self.pos.saturating_add(n)).unwrap_or(&self.end)
    }
    fn peek(&self) -> &Tok {
        &self.token_at(0).tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.token_at(n).tok
    }
    fn here(&self) -> usize {
        self.token_at(0).start
    }
    /// Moves to the next token; at the end, stays there.
    fn advance(&mut self) {
        if self.pos.saturating_add(1) < self.toks.len() {
            self.pos = self.pos.saturating_add(1);
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::KeyWord(k) if *k == kw)
    }
    /// Whether the token after the one at hand is the key word `kw`.
    fn next_is_kw(&self, kw: &str) -> bool {
        matches!(self.peek_at(1), Tok::KeyWord(k) if *k == kw)
    }
    fn accept_kw(&mut self, kw: &str) -> bool {
        let yes = self.is_kw(kw);
        if yes {
            self.advance();
        }
        yes
    }
    fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.accept_kw(kw) { Ok(()) } else { Err(self.syntax(format!("expected {kw}"))) }
    }
    fn is_delim(&self, d: &str) -> bool {
        matches!(self.peek(), Tok::Delimiter(x) if *x == d)
    }
    fn accept_delim(&mut self, d: &str) -> bool {
        let yes = self.is_delim(d);
        if yes {
            self.advance();
        }
        yes
    }
    fn expect_delim(&mut self, d: &str) -> Result<()> {
        if self.accept_delim(d) { Ok(()) } else { Err(self.syntax(format!("expected \"{d}\""))) }
    }
    fn syntax(&self, msg: impl Into<String>) -> SqlError {
        SqlError::at(Code::Syntax, self.here(), format!("{}, found {}", msg.into(), describe(self.peek())))
    }
    fn rule(&self, msg: impl Into<String>) -> SqlError {
        SqlError::at(Code::SyntaxRule, self.here(), msg)
    }
    fn unsupported(&self, what: &str) -> SqlError {
        SqlError::at(Code::Unsupported, self.here(), format!("[DESIGN] {what} is valid SQL but is not implemented (RULES.md D18)"))
    }
    /// The source from character `start` to the token now at hand, without
    /// the separators before it.
    fn text_from(&self, start: usize) -> String {
        let text: String = self.src.get(start..self.here()).unwrap_or_default().iter().collect();
        text.trim_end().to_owned()
    }

    fn end(&self) -> Result<()> {
        if *self.peek() == Tok::End { Ok(()) } else { Err(self.syntax("expected end of statement")) }
    }

    /// [X3.135-1986 §5.3] `<identifier>`
    fn identifier(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Identifier(s) => {
                self.advance();
                Ok(s)
            }
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Delimiter(_)
            | Tok::End => Err(self.syntax("expected an <identifier>")),
        }
    }

    /// [X3.135-1986 §5.2] `<unsigned integer>` ::= `<digit>`...
    fn unsigned_integer(&mut self) -> Result<u32> {
        match self.peek().clone() {
            Tok::ExactNumeric(t) if t.chars().all(|c| c.is_ascii_digit()) => {
                // [X3.135-1986 §5.2] sets no maximum; this implementation's is u32::MAX.
                let n = t.parse().map_err(|_| {
                    SqlError::at(
                        Code::Unsupported,
                        self.here(),
                        format!("[DESIGN] {t} is larger than this implementation's maximum of {}", u32::MAX),
                    )
                })?;
                self.advance();
                Ok(n)
            }
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Delimiter(_)
            | Tok::End => Err(self.syntax("expected an <unsigned integer>")),
        }
    }

    /// [X3.135-1986 §5.5 SR2] a `<length>` or `<precision>` shall be greater than 0.
    fn positive(&mut self, what: &str) -> Result<u32> {
        let at = self.here();
        let n = self.unsigned_integer()?;
        if n == 0 {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[X3.135-1986 §5.5 SR2] a <{what}> shall be greater than 0")));
        }
        Ok(n)
    }

    // =====================================================================
    // [X3.135-1986 §5.4] Names
    // =====================================================================

    /// [X3.135-1986 §5.4] `<table name>` ::= \[`<authorization identifier>`.\] `<table identifier>`
    fn table_name(&mut self) -> Result<TableName> {
        let at = self.here();
        let first = self.qualifier()?;
        if self.accept_delim(delim::PERIOD) {
            let ident = self.identifier()?;
            Ok(TableName { auth: Some(first), ident, at })
        } else {
            Ok(TableName { auth: None, ident: first, at })
        }
    }

    /// A `<table name>`'s, or a `<column reference>`'s, first part: an
    /// `<identifier>`, or MODULE, of a `<qualified local table name>` ::=
    /// MODULE . `<local table name>` [ISO/IEC 9075:1992 §5.4].
    fn qualifier(&mut self) -> Result<String> {
        if self.is_kw(kw::MODULE) && matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD)) {
            self.advance();
            return Ok(kw::MODULE.to_owned());
        }
        self.identifier()
    }

    // =====================================================================
    // [X3.135-1986 §8.7] <insert statement>
    // =====================================================================

    /// INSERT INTO `<table name>` {\[(`<insert column list>`)\] `<query
    /// expression>` | DEFAULT VALUES} [ISO/IEC 9075:1992 §13.8]
    ///
    /// # Errors
    ///
    /// The text is not an `<insert statement>`.
    pub fn insert_statement(&mut self) -> Result<InsertStatement> {
        self.expect_kw(kw::INSERT)?;
        self.expect_kw(kw::INTO)?;
        let table = self.table_name()?;

        // <insert column list> ::= <column name> [{,<column name>}...]
        let mut columns = None;
        if self.accept_delim(delim::LEFT_PAREN) {
            let mut list = vec![self.column_name()?];
            while self.accept_delim(delim::COMMA) {
                list.push(self.column_name()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            columns = Some(list);
        }

        // [ISO/IEC 9075:1992 §13.8] DEFAULT VALUES, with no column list.
        if columns.is_none() && self.accept_kw(kw::DEFAULT) {
            self.expect_kw(kw::VALUES)?;
            self.end()?;
            return Ok(InsertStatement { table, columns, source: InsertSource::DefaultValues });
        }
        let query = self.query_expression()?;
        self.end()?;
        Ok(InsertStatement { table, columns, source: InsertSource::Query(Box::new(query)) })
    }

    /// A `<column name>`, with where it is written.
    fn column_name(&mut self) -> Result<ColumnName> {
        let at = self.here();
        Ok(ColumnName { name: self.identifier()?, at })
    }

    /// Parses one level deeper in the syntax tree. The depth is restored
    /// afterwards, also on failure, because `boolean_primary` backtracks.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let saved = self.depth;
        let result = self.deepen().and_then(|()| parse(self));
        self.depth = saved;
        result
    }

    /// Counts one more level of the tree being built; fails past `MAX_NESTING_DEPTH`.
    fn deepen(&mut self) -> Result<()> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(SqlError::at(
                Code::Unsupported,
                self.here(),
                format!("[DESIGN] the statement nests deeper than this implementation's limit of {MAX_NESTING_DEPTH} levels"),
            ));
        }
        self.depth = self.depth.saturating_add(1);
        Ok(())
    }

    /// [X3.135-1986 §5.6] `<parameter specification>` ::= `<parameter name>` \[`<indicator parameter>`\]
    /// `<indicator parameter>` ::= \[INDICATOR\] `<parameter name>`
    pub(super) fn parameter_specification(&mut self) -> Result<ParameterSpecification> {
        let at = self.here();
        let name = self.identifier()?;
        if !self.params.contains(&name) {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[X3.135-1986 §7.3 SR3] parameter {name} is not declared")));
        }
        let has_keyword = self.accept_kw(kw::INDICATOR);
        let indicator = if has_keyword || self.at_parameter() {
            let indicator_at = self.here();
            let ind = self.identifier()?;
            if !self.params.contains(&ind) {
                return Err(SqlError::at(
                    Code::SyntaxRule,
                    indicator_at,
                    format!("[X3.135-1986 §7.3 SR3] parameter {ind} is not declared"),
                ));
            }
            Some(ind)
        } else {
            None
        };
        Ok(ParameterSpecification { name, indicator, at })
    }

    /// [X3.135-1986 §8.6, §8.10] `<fetch target list>` and `<select target list>`
    /// ::= `<target specification>` \[{, `<target specification>`}...\]
    pub(super) fn target_list(&mut self) -> Result<Vec<ParameterSpecification>> {
        let mut list = vec![self.target_specification()?];
        while self.accept_delim(delim::COMMA) {
            list.push(self.target_specification()?);
        }
        Ok(list)
    }

    /// [X3.135-1986 §5.6] `<target specification>` ::= `<parameter specification>`
    /// (not implemented: `<variable specification>`, which is embedded SQL)
    fn target_specification(&mut self) -> Result<ParameterSpecification> {
        if !matches!(self.peek(), Tok::Identifier(_)) {
            return Err(self.syntax("expected a <target specification>"));
        }
        self.parameter_specification()
    }

    // =====================================================================
    // [X3.135-1986 §7.3] <SQL statement>
    // =====================================================================

    /// `<SQL statement>`: one statement of a procedure.
    ///
    /// # Errors
    ///
    /// The text is not an `<SQL statement>`, or is a `<declare cursor>`, which
    /// belongs to the module.
    pub fn sql_statement(&mut self) -> Result<SqlStatement> {
        let stmt = match self.peek() {
            Tok::KeyWord(kw::CLOSE) => {
                // [X3.135-1986 §8.1] CLOSE <cursor name>
                self.advance();
                SqlStatement::Close(self.identifier()?)
            }
            Tok::KeyWord(kw::OPEN) => {
                // [X3.135-1986 §8.8] OPEN <cursor name>
                self.advance();
                SqlStatement::Open(self.identifier()?)
            }
            Tok::KeyWord(kw::COMMIT) => {
                // [X3.135-1986 §8.2] COMMIT WORK; [ISO/IEC 9075:1992 §14.3] WORK optional.
                self.advance();
                self.accept_kw(kw::WORK);
                SqlStatement::Commit
            }
            Tok::KeyWord(kw::ROLLBACK) => {
                // [X3.135-1986 §8.9] ROLLBACK WORK; [ISO/IEC 9075:1992 §14.4] WORK optional.
                self.advance();
                self.accept_kw(kw::WORK);
                SqlStatement::Rollback
            }
            Tok::KeyWord(kw::SET) => self.set_statement()?,
            Tok::KeyWord(kw::FETCH) => self.fetch_statement()?,
            Tok::KeyWord(kw::GET) => SqlStatement::GetDiagnostics(self.get_diagnostics()?),
            Tok::KeyWord(kw::INSERT) => return Ok(SqlStatement::Insert(self.insert_statement()?)),
            Tok::KeyWord(kw::DELETE) => self.delete_statement()?,
            Tok::KeyWord(kw::UPDATE) => self.update_statement()?,
            Tok::KeyWord(kw::SELECT) => self.select_statement()?,
            Tok::KeyWord(kw::GRANT | kw::REVOKE) => self.schema_statement()?,
            Tok::KeyWord(kw::DROP | kw::ALTER | kw::CREATE) => self.schema_manipulation()?,
            // [ISO/IEC 9075:1992 §20.1] a <temporary table declaration>
            // is a <direct SQL data statement>.
            Tok::KeyWord(kw::DECLARE) => return self.declare_statement(),
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected an <SQL statement> [X3.135-1986 §7.3]")),
        };
        self.end()?;
        Ok(stmt)
    }

    /// [X3.135-1986 §8.4, §8.5] DELETE FROM `<table name>`
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    fn delete_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DELETE)?;
        self.expect_kw(kw::FROM)?;
        let table = self.table_name()?;
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::DeleteSearched { table, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            return Ok(SqlStatement::DeletePositioned { table, cursor: self.identifier()? });
        }
        Ok(SqlStatement::DeleteSearched { table, where_clause: Some(self.search_condition()?) })
    }

    /// [X3.135-1986 §8.11, §8.12] UPDATE `<table name>` SET `<set clause>` \[{,`<set clause>`}...\]
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    fn update_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::UPDATE)?;
        let table = self.table_name()?;
        self.expect_kw(kw::SET)?;
        let mut set = vec![self.set_clause()?];
        while self.accept_delim(delim::COMMA) {
            set.push(self.set_clause()?);
        }
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::UpdateSearched { table, set, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            return Ok(SqlStatement::UpdatePositioned { table, set, cursor: self.identifier()? });
        }
        Ok(SqlStatement::UpdateSearched { table, set, where_clause: Some(self.search_condition()?) })
    }

    /// [X3.135-1986 §8.11, §8.12] `<set clause>` ::= `<object column>` =
    /// `<update source>`, which is `<value expression>` | NULL | DEFAULT
    /// [ISO/IEC 9075:1992 §13.9]; `<object column>` ::= `<column name>`
    fn set_clause(&mut self) -> Result<SetClause> {
        let at = self.here();
        let column = self.identifier()?;
        self.expect_delim(delim::EQUALS)?;
        let value = if self.accept_kw(kw::NULL) {
            UpdateSource::Null
        } else if self.accept_kw(kw::DEFAULT) {
            UpdateSource::Default
        } else {
            UpdateSource::Value(self.value_expression()?)
        };
        Ok(SetClause { column, value, at })
    }

    /// [X3.135-1986 §8.10] SELECT \[ALL | DISTINCT\] `<select list>`
    ///     INTO `<select target list>` `<table expression>`
    fn select_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::SELECT)?;
        let distinct = if self.accept_kw(kw::DISTINCT) {
            true
        } else {
            self.accept_kw(kw::ALL);
            false
        };
        let select_list = self.select_list()?;
        if !self.accept_kw(kw::INTO) {
            return Err(self
                .syntax("a <select statement> needs INTO <select target list> [X3.135-1986 §8.10]; to read many rows, declare a cursor"));
        }
        let targets = self.target_list()?;
        let table_expression = self.table_expression()?;
        Ok(SqlStatement::Select { query: Box::new(QuerySpecification { distinct, select_list, table_expression }), targets })
    }

    /// [X3.135-1986 §5.7] `<column specification>` ::= \[`<qualifier>`.\] `<column name>`
    /// `<qualifier>` ::= `<table name>` | `<correlation name>`
    fn column_specification(&mut self) -> Result<ColumnSpecification> {
        let at = self.here();
        let a = self.qualifier()?;
        if !self.accept_delim(delim::PERIOD) {
            return Ok(ColumnSpecification { qualifier: None, column: a, at });
        }
        let b = self.identifier()?;
        if !self.accept_delim(delim::PERIOD) {
            return Ok(ColumnSpecification { qualifier: Some(TableName { auth: None, ident: a, at }), column: b, at });
        }
        let c = self.identifier()?;
        Ok(ColumnSpecification { qualifier: Some(TableName { auth: Some(a), ident: b, at }), column: c, at })
    }
}
