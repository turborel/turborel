//! Recursive descent parser. One method per BNF production, named after it.
//!
//! The standard's left-recursive rules ("`<term>` ::= `<factor>` | `<term>` * `<factor>`") become loops, which gives
//! the left-to-right evaluation order of [X3.135-1986 §5.9 GR7] and [X3.135-1986 §5.18 GR3]. The one real ambiguity in
//! the grammar, that both `<predicate>` and (`<search condition>`) can start with "(", is resolved by trying
//! `<predicate>` first and backtracking (\[DESIGN\] PEG-style ordered choice).
//!
//! Syntax that is valid SQL but not implemented reports `Code::Unsupported` rather than a misleading syntax error.

pub use ddl::{GENERATION, RESEAL, REVERT_COLLATION, method_of_type, option_of};
pub use domains::VALUE_COLUMN;
pub(crate) use dynamic::descriptor_type;
pub use graph::{Declared, Degree, Exposed, Exposure, exposure, in_path_patterns, normalize, normalize_within, parenthesized_exposure};
pub use graph::{LAST_NODE_PREFIX, MATCH_TABLE, checked_graph_table, element_key};
pub(crate) use periods::period_constraint;
use std::collections::{HashMap, HashSet};
pub use xml::{MappedSource, mapped_sources};

use crate::ast::{ColumnName, ParameterSpecification, QuerySpecification, SqlStatement, TableName, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, Token, delim, kw, lexed};

mod backup;
mod charsets;
mod composite;
mod connections;
mod constraints;
mod contents;
mod cursor;
mod data_statements;
mod data_type;
mod datetime;
mod ddl;
mod delta;
mod depth;
mod diagnostics;
mod distribution;
mod domains;
mod dynamic;
mod embedding;
mod expression;
mod extension;
mod filter;
mod generated;
mod gql;
mod graph;
pub(crate) mod graph_algo;
mod graph_ddl;
mod grouping;
mod hypothetical;
mod insert;
mod interval;
mod json;
mod lexed;
mod listagg;
mod literal;
mod locators;
mod manifest;
mod merge;
mod modules;
mod multisets;
mod names;
mod native;
mod numeric;
mod partitions;
mod pattern;
mod periods;
mod primary;
mod privileges;
mod psm;
mod ptf;
mod query;
mod regex;
mod roles;
mod routines;
mod savepoints;
mod schema;
mod search_cycle;
mod sequences;
mod served;
mod structured;
mod user_types;
mod window;
mod xml;
pub(crate) use data_statements::{TRUNCATE, begins_change, begins_word_statement, explained};
pub(crate) use {savepoints::SAVEPOINT, served::CANCEL};
mod session;
mod stored;
mod target;
mod triggers;
#[cfg(test)]
mod truncated;
mod users;
mod vectors;

/// \[DESIGN\] The deepest syntax tree the parser builds. Each nested `<search
/// condition>`, `<boolean term>`, `<value expression>`, `<term>`, `<query expression>`
/// or `<subquery>` counts one level, and so does each further operator of a chain
/// (`OR`, `AND`, `+`, `-`, `||`, `*`, `/`, `UNION`, `EXCEPT`, `INTERSECT`, a join),
/// since a chain builds one tree level per operator, and each `<table reference>`
/// of a `<from clause>` after the first, since the plan joins them one level each.
/// Rewrites gather conjuncts with `logical::and_all`, which builds a balanced tree,
/// so no plan gets much deeper than the statement.
///
/// X3.135-1986 specifies no such limit ([X3.135-1986 §3.4] requires conforming
/// language to be processed according to the General Rules), so a deeper statement
/// is valid SQL-86 that this implementation does not process: SQLCODE -900. The
/// limit keeps every later recursive walk of the tree (binding, optimization,
/// execution, drop) within the stack (RULES.md R-CF-6).
///
/// `nesting_beyond_the_limit_is_rejected_not_a_crash` checks that the deepest
/// accepted statement of each shape runs on a 1 MiB thread, half of Rust's
/// default 2 MiB thread stack.
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
    /// The position last counted to and the `?`s before it, so each is
    /// counted once as the parser goes on (`Parser::questions_before`).
    questions: (usize, usize),
    /// The source, for keeping a `<cursor specification>` as text.
    src: Vec<char>,
    /// In a domain constraint, where VALUE may be written [ISO/IEC
    /// 9075:1992 §6.2 SR5].
    in_domain: bool,
    /// The first refusal of valid SQL not implemented (SQLCODE -900), given
    /// once the whole statement is read, so SQL that breaks a Format is a
    /// syntax error still (`Parser::refuse_later`).
    refused: Option<SqlError>,
    /// How a `<catalog name>` written in a name is taken.
    catalogs: Catalogs,
    /// [ISO/IEC 9075:1992 §17.19 SR1, §17.20 SR1] Of a statement being
    /// prepared that changes a row through a cursor and names no table, the
    /// table of the cursor's specification.
    implicit: Option<TableName>,
    /// Each `<value expression>` read, by the token it begins at: what was
    /// read, the token after it, and the dynamic parameters read by then.
    read_values: HashMap<usize, (Result<ValueExpression>, usize, Option<usize>)>,
    /// [ISO/IEC 9075-2:2023 §11.49] Of a `<triggered action>` being read, what it knows.
    trigger: Option<triggers::TriggerScope>,
    /// [ISO/IEC 9075-2:2023 §11.60] Of an `<SQL routine body>` being read, what it knows.
    routine: Option<routines::RoutineScope>,
    /// [ISO/IEC 9075-4:2006 draft §14.1] The SQL control statements being read, the outermost first.
    blocks: Vec<psm::Block>,
    /// [ISO/IEC 9075-4:2006 draft §12.8] In an SQL-server module, its own schema, which MODULE names.
    module: Option<String>,
    /// \[EXTENSION\] The GRAPH_TABLEs being read around the token at hand, where a graph
    /// function's or predicate's name before "(" is that function's, and elsewhere a routine's.
    graph: usize,
    /// \[EXTENSION\] [GS §10] The query specifications being read, the innermost last.
    compat: Vec<graph::Frame>,
    /// \[EXTENSION\] The number of each `<dynamic parameter specification>` by its place among them,
    /// of a text that copies some of a statement's (`Parser::hybrid_expansion`); of none, its place.
    numbers: Vec<usize>,
}

/// How a parser takes a `<catalog name>` written in a qualified name or a
/// `<column reference>` [ISO/IEC 9075:1992 §5.4]: dropped once taken,
/// there being one catalog.
#[derive(Debug, Clone)]
enum Catalogs {
    /// Refused (SQLCODE -900): a parser told of no catalog.
    Refused,
    /// Taken if it is this one, the one catalog's name [§5.4 SR20]; else
    /// SQLCODE -103, SQLSTATE 3D000.
    Of(String),
    /// Taken, whatever it is: the text of a definition, checked when it
    /// was defined.
    Kept,
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::KeyWord(k) => k.to_string(),
        Tok::Identifier(s) | Tok::ExactNumeric(s) | Tok::ApproximateNumeric(s) | Tok::LargeObjectLength(s) => s.clone(),
        Tok::CharacterString(s) => format!("'{s}'"),
        Tok::BinaryString(b) => format!("X'{b}'"),
        Tok::Introducer(name) => format!("_{name}"),
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
    /// A parser for standard SQL, without the \[EXTENSION\] function call syntax.
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
        Ok(Self::of_lexed(lexed(src)?, extensions, params))
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

    /// This parser, taking a `<catalog name>` written in a name if it is
    /// `catalog`, the one catalog's [ISO/IEC 9075:1992 §5.4 SR20]: as a
    /// statement is read as it is given.
    #[must_use]
    pub fn in_catalog(mut self, catalog: &str) -> Parser {
        self.catalogs = Catalogs::Of(catalog.trim_end().to_owned());
        self
    }

    /// This parser, of a statement being prepared that changes a row
    /// through a cursor of the table `implicit` and names no table
    /// [ISO/IEC 9075:1992 §17.19 SR1, §17.20 SR1].
    #[must_use]
    pub fn implying(mut self, implicit: Option<TableName>) -> Parser {
        self.implicit = implicit;
        self
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

    /// The end of a statement: \[ISO/IEC 9075:1992 §20.1\] after one
    /// semicolon, if written; then the refusal of valid SQL not implemented
    /// read in it, if any (`refuse_later`).
    fn end(&mut self) -> Result<()> {
        // A statement of a trigger's action or a routine's body ends where its Format does.
        if self.trigger.is_some() || self.routine.is_some() {
            return Ok(());
        }
        self.accept_delim(delim::SEMICOLON);
        if *self.peek() != Tok::End {
            return Err(self.syntax("expected end of statement"));
        }
        self.refused.take().map_or(Ok(()), Err)
    }

    /// Refuses the statement as valid SQL not implemented with `e`, once it
    /// is read whole, unless it breaks a Format first: the first such.
    fn refuse_later(&mut self, e: SqlError) {
        self.refused.get_or_insert(e);
    }

    /// [X3.135-1986 §5.3] `<identifier>`
    fn identifier(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Identifier(s) => {
                self.advance();
                Ok(s)
            }
            // [ISO/IEC 9075:1992 §5.4] <identifier> ::= [<introducer>
            // <character set specification>] <actual identifier>.
            Tok::Introducer(_) | Tok::LargeObjectLength(_) => {
                if !matches!(self.peek_at(1), Tok::Identifier(_)) {
                    return Err(self.syntax("expected an <identifier> after the <introducer>"));
                }
                self.introducer();
                self.identifier()
            }
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
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
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
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
    // [X3.135-1986 §8.7] <insert statement>
    // =====================================================================

    /// (`<insert column list>`) ::= ( `<column name>` \[{, `<column
    /// name>`}...\] ), each column where it is written, of an INSERT
    /// \[X3.135-1986 §8.7\] or a MERGE's insert \[ISO/IEC 9075-2:2023
    /// §14.12\].
    pub(super) fn insert_column_list(&mut self) -> Result<Vec<ColumnName>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![self.column_name()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.column_name()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// A `<column name>`, with where it is written.
    fn column_name(&mut self) -> Result<ColumnName> {
        let at = self.here();
        Ok(ColumnName { name: self.identifier()?, at })
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
        Ok(ParameterSpecification { name, qualifier: None, indicator, at, kind: crate::ast::TargetKind::Parameter, element: None })
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
        // [ISO/IEC 9075-4:2006 draft §4.10.7] an SQL control statement, in a body or directly.
        if let Some(stmt) = self.control_statement()? {
            return Ok(stmt);
        }
        // \[EXTENSION\] CANCEL is no key word, nor TRUNCATE: words.
        if let Some(stmt) = self.word_statement()? {
            return Ok(stmt);
        }
        let stmt = match self.peek() {
            Tok::KeyWord(kw::CLOSE | kw::OPEN) => self.open_or_close()?,
            // [ISO/IEC 9075:1992 §15.1, §15.3]
            Tok::KeyWord(kw::CONNECT) => self.connect_statement()?,
            Tok::KeyWord(kw::DISCONNECT) => self.disconnect_statement()?,
            // \[EXTENSION\] RULES.md D42.
            Tok::KeyWord(kw::END) => self.served_statement()?,
            // [ISO/IEC 9075:1992 §17] dynamic SQL in SQL text.
            Tok::KeyWord(kw::ALLOCATE | kw::DEALLOCATE) => self.allocation()?,
            Tok::KeyWord(kw::PREPARE | kw::DESCRIBE | kw::EXECUTE) => self.dynamic_statement()?,

            Tok::KeyWord(kw::COMMIT | kw::ROLLBACK) => self.transaction_end()?,
            Tok::KeyWord(kw::SET) => self.set_statement()?,
            Tok::KeyWord(kw::FETCH) => self.fetch_statement()?,
            Tok::KeyWord(kw::GET) => {
                if self.next_is_kw(kw::DESCRIPTOR) {
                    self.advance();
                    self.get_descriptor()?
                } else {
                    SqlStatement::GetDiagnostics(self.get_diagnostics()?)
                }
            }
            Tok::KeyWord(kw::INSERT) => return Ok(SqlStatement::Insert(self.insert_statement()?)),
            Tok::KeyWord(kw::DELETE) => self.delete_statement()?,
            Tok::KeyWord(kw::UPDATE) => self.update_statement()?,
            Tok::KeyWord(kw::SELECT) => self.select_statement()?,
            Tok::KeyWord(kw::GRANT | kw::REVOKE) => self.schema_statement()?,
            Tok::KeyWord(kw::DROP | kw::ALTER | kw::CREATE) => self.manipulation()?,
            // [ISO/IEC 9075:1992 §20.1] a <temporary table declaration>
            // is a <direct SQL data statement>.
            Tok::KeyWord(kw::DECLARE) => return self.declare_statement(),
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected an <SQL statement> [X3.135-1986 §7.3]")),
        };
        self.end()?;
        Ok(stmt)
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
        // [ISO/IEC 9075-2:2023 §7.6] whose statement a SELECT INTO runs
        // (RULES.md D174).
        if let [crate::ast::TableReference::Delta(d)] = table_expression.from.as_slice() {
            d.rows.admit();
        }
        Ok(SqlStatement::Select { query: Box::new(QuerySpecification { distinct, select_list, table_expression }), targets })
    }
}
