//! [ISO/IEC 9075-2:2023 §7.6, §7.7, §7.8, §7.9] Row pattern recognition:
//! a `<row pattern recognition clause and name>` after a table or derived
//! table, its `<row pattern measures>` and the `<row pattern common
//! syntax>` it shares with a window [§7.15]. The `<row pattern>` itself is
//! in `row_pattern.rs`, and the scalar expressions only row pattern
//! matching reads in `navigation.rs`.
//!
//! SQL-92 reserves none of MATCH_RECOGNIZE, MEASURES, ONE, PER, SHOW, OMIT,
//! EMPTY, MATCHES, UNMATCHED, AFTER, SKIP, PAST, INITIAL, SEEK, PATTERN,
//! SUBSET, DEFINE and PERMUTE, so each is an identifier here, read as a key
//! word only where this syntax has it. ALL, ROWS, MATCH, WITH, TO, NEXT,
//! FIRST, LAST and AS are SQL-92's reserved words.

mod navigation;
mod row_pattern;

use super::Parser;
use super::window::ROW;
use crate::ast::{
    Definition, DerivedTable, EmptyMatches, GroupingColumn, Measure, PatternCommon, RecognizedTable, RowPatternRecognition, RowsPerMatch,
    SkipTo, Subset, TableReference,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 9075-2:2023 §5.2, §7.7, §7.8, §7.9] Words of row pattern
// recognition that SQL-92 does not reserve.
/// MATCH_RECOGNIZE, before "(" after a table primary.
const MATCH_RECOGNIZE: &str = "MATCH_RECOGNIZE";
/// MEASURES, before a `<row pattern measure list>`.
const MEASURES: &str = "MEASURES";
/// ONE, of ONE ROW PER MATCH.
const ONE: &str = "ONE";
/// PER, of ONE ROW PER MATCH and ALL ROWS PER MATCH.
const PER: &str = "PER";
/// SHOW, of SHOW EMPTY MATCHES.
const SHOW: &str = "SHOW";
/// OMIT, of OMIT EMPTY MATCHES.
const OMIT: &str = "OMIT";
/// EMPTY, of SHOW and OMIT EMPTY MATCHES.
const EMPTY: &str = "EMPTY";
/// MATCHES, of SHOW and OMIT EMPTY MATCHES.
const MATCHES: &str = "MATCHES";
/// UNMATCHED, of WITH UNMATCHED ROWS.
const UNMATCHED: &str = "UNMATCHED";
/// AFTER, of AFTER MATCH.
const AFTER: &str = "AFTER";
/// SKIP, of a `<row pattern skip to>`.
const SKIP: &str = "SKIP";
/// PAST, of SKIP PAST LAST ROW.
const PAST: &str = "PAST";
/// INITIAL, a `<row pattern initial or seek>`.
const INITIAL: &str = "INITIAL";
/// SEEK, a `<row pattern initial or seek>`.
const SEEK: &str = "SEEK";
/// PATTERN, before "(" and a `<row pattern>`.
const PATTERN: &str = "PATTERN";
/// SUBSET, before a `<row pattern subset list>`.
const SUBSET: &str = "SUBSET";
/// DEFINE, before a `<row pattern definition list>`.
const DEFINE: &str = "DEFINE";

impl Parser {
    /// Whether MATCH_RECOGNIZE and "(" are next, so that MATCH_RECOGNIZE is
    /// not taken as a correlation name.
    pub(super) fn at_match_recognize(&self) -> bool {
        self.is_word(MATCH_RECOGNIZE) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [ISO/IEC 9075-2:2023 §7.6] `input`, a table or derived table with
    /// its `<row pattern input name>` and columns if written, then its
    /// `<row pattern recognition clause>` and `<row pattern output name>`
    /// and columns, if MATCH_RECOGNIZE is next; else `input` alone.
    pub(super) fn recognized(&mut self, input: TableReference) -> Result<TableReference> {
        if !self.at_match_recognize() {
            return Ok(input);
        }
        let at = self.here();
        self.advance();
        self.advance();
        let clause = self.row_pattern_recognition()?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        Ok(TableReference::Recognized(Box::new(RecognizedTable { input, clause, correlation, columns, at })))
    }

    /// [ISO/IEC 9075-2:2023 §7.6] A `<derived table>` written with no
    /// `<row pattern input name>` before MATCH_RECOGNIZE. Its range
    /// variable is implementation-dependent \[SR6b i 4 D\], here the empty
    /// name. No `<identifier>` is empty [ISO/IEC 9075:1992 §5.2], so the
    /// name is distinct from every other.
    pub(super) fn unnamed_input(&mut self, query: DerivedTable) -> Result<TableReference> {
        if !self.at_match_recognize() {
            return Err(self.syntax("[ISO/IEC 9075:1992 §6.3 F] a <derived table> has a correlation name"));
        }
        self.recognized(TableReference::Derived(query))
    }

    /// [ISO/IEC 9075-2:2023 §7.7] The rest of a `<row pattern recognition
    /// clause>`, MATCH_RECOGNIZE and "(" taken, up to and including its
    /// ")".
    fn row_pattern_recognition(&mut self) -> Result<RowPatternRecognition> {
        let partition = self.row_pattern_partition_by()?;
        let order = if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            self.sort_specification_list()?
        } else {
            Vec::new()
        };
        let measures = self.row_pattern_measures()?;
        let rows = self.row_pattern_rows_per_match()?;
        let common = self.row_pattern_common_syntax(false)?;
        if rows == RowsPerMatch::All(EmptyMatches::Unmatched) && common.pattern.excludes() {
            return Err(SqlError::at(
                Code::SyntaxRule,
                common.at,
                "[ISO/IEC 9075-2:2023 §7.9 SR22] the <row pattern> of ALL ROWS PER MATCH WITH UNMATCHED ROWS has no {- and -}",
            ));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(RowPatternRecognition { partition, order, measures, rows, common })
    }

    /// `<row pattern partition by>` ::= PARTITION BY `<row pattern
    /// partition column>` \[{, ...}\], each a `<column reference>` and its
    /// `<collate clause>` if written; none if not written.
    fn row_pattern_partition_by(&mut self) -> Result<Vec<GroupingColumn>> {
        let mut partition = Vec::new();
        if !self.at_partitioning() {
            return Ok(partition);
        }
        self.advance();
        self.advance();
        partition.push(self.grouping_column()?);
        while self.accept_delim(delim::COMMA) {
            partition.push(self.grouping_column()?);
        }
        Ok(partition)
    }

    /// Whether a window's `<row pattern measures>` begin here, at a
    /// MEASURES that is no `<existing window name>` \[DESIGN\]. MEASURES before
    /// ")", ORDER, ROWS, PARTITION, RANGE or GROUPS names a window, so a
    /// measure's expression does not begin with a column of those names.
    pub(super) fn at_window_measures(&self) -> bool {
        if !self.is_word(MEASURES) {
            return false;
        }
        match self.peek_at(1) {
            Tok::Delimiter(delim::RIGHT_PAREN) | Tok::KeyWord(kw::ORDER | kw::ROWS) => false,
            Tok::Identifier(s) => !super::window::NAMES_AFTER_WINDOW_NAME.contains(&s.as_str()),
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => true,
        }
    }

    /// [ISO/IEC 9075-2:2023 §7.8] `<row pattern measures>` ::= MEASURES
    /// `<row pattern measure definition>` \[{, ...}\], each a `<value
    /// expression>` AS `<measure name>`; none if not written.
    pub(super) fn row_pattern_measures(&mut self) -> Result<Vec<Measure>> {
        let mut measures = Vec::new();
        if !self.accept_word(MEASURES) {
            return Ok(measures);
        }
        loop {
            let at = self.here();
            let expr = self.value_expression()?;
            self.expect_kw(kw::AS)?;
            let name = self.identifier()?;
            measures.push(Measure { expr, name, at });
            if !self.accept_delim(delim::COMMA) {
                return Ok(measures);
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §7.7] `<row pattern rows per match>` ::= ONE
    /// ROW PER MATCH | ALL ROWS PER MATCH \[`<row pattern empty match
    /// handling>`\]. It is ONE ROW PER MATCH if not written \[SR5\], and
    /// SHOW EMPTY MATCHES if no handling is \[SR6\].
    fn row_pattern_rows_per_match(&mut self) -> Result<RowsPerMatch> {
        if self.accept_word(ONE) {
            self.expect_word(ROW)?;
            self.per_match()?;
            return Ok(RowsPerMatch::One);
        }
        if !self.accept_kw(kw::ALL) {
            return Ok(RowsPerMatch::One);
        }
        self.expect_kw(kw::ROWS)?;
        self.per_match()?;
        let handling = if self.accept_word(SHOW) {
            self.empty_matches().map(|()| EmptyMatches::Show)?
        } else if self.accept_word(OMIT) {
            self.empty_matches().map(|()| EmptyMatches::Omit)?
        } else if self.accept_kw(kw::WITH) {
            self.expect_word(UNMATCHED)?;
            self.expect_kw(kw::ROWS)?;
            EmptyMatches::Unmatched
        } else {
            EmptyMatches::Show
        };
        Ok(RowsPerMatch::All(handling))
    }

    /// PER MATCH.
    fn per_match(&mut self) -> Result<()> {
        self.expect_word(PER)?;
        self.expect_kw(kw::MATCH)
    }

    /// EMPTY MATCHES.
    fn empty_matches(&mut self) -> Result<()> {
        self.expect_word(EMPTY)?;
        self.expect_word(MATCHES)
    }

    /// Whether a `<row pattern common syntax>` begins here, at AFTER MATCH,
    /// at INITIAL or SEEK before PATTERN, or at PATTERN before "(".
    pub(super) fn at_row_pattern_common_syntax(&self) -> bool {
        let after = self.is_word(AFTER) && self.next_is_kw(kw::MATCH);
        let initial = (self.is_word(INITIAL) || self.is_word(SEEK)) && is_named(self.peek_at(1), PATTERN);
        let pattern = self.is_word(PATTERN) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        after || initial || pattern
    }

    /// [ISO/IEC 9075-2:2023 §7.9] `<row pattern common syntax>` ::=
    /// \[AFTER MATCH `<row pattern skip to>`\] \[`<row pattern initial or
    /// seek>`\] PATTERN ( `<row pattern>` ) \[`<row pattern subset
    /// clause>`\] DEFINE `<row pattern definition list>`, of a window if
    /// `window`, else of a `<row pattern recognition clause>`.
    pub(super) fn row_pattern_common_syntax(&mut self, window: bool) -> Result<PatternCommon> {
        let at = self.here();
        let skip = if self.accept_word(AFTER) {
            self.expect_kw(kw::MATCH)?;
            self.row_pattern_skip_to()?
        } else {
            // [SR23a] AFTER MATCH SKIP PAST LAST ROW, if not written.
            SkipTo::PastLastRow
        };
        let seek = self.row_pattern_initial_or_seek(window)?;
        self.expect_word(PATTERN)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let pattern = self.row_pattern()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let subsets = self.row_pattern_subset_clause()?;
        self.expect_word(DEFINE)?;
        let defines = self.row_pattern_definition_list()?;
        Ok(PatternCommon { skip, seek, pattern, subsets, defines, at })
    }

    /// `<row pattern skip to>` ::= SKIP TO NEXT ROW | SKIP PAST LAST ROW |
    /// SKIP TO FIRST `<row pattern skip to variable name>` | SKIP TO LAST
    /// it | SKIP TO it, read as SKIP TO LAST \[SR23b\].
    fn row_pattern_skip_to(&mut self) -> Result<SkipTo> {
        self.expect_word(SKIP)?;
        if self.accept_word(PAST) {
            self.expect_kw(kw::LAST)?;
            self.expect_word(ROW)?;
            return Ok(SkipTo::PastLastRow);
        }
        self.expect_kw(kw::TO)?;
        if self.accept_kw(kw::NEXT) {
            self.expect_word(ROW)?;
            return Ok(SkipTo::NextRow);
        }
        let first = self.accept_kw(kw::FIRST);
        if !first {
            self.accept_kw(kw::LAST);
        }
        let at = self.here();
        let name = self.identifier()?;
        Ok(if first { SkipTo::First(name, at) } else { SkipTo::Last(name, at) })
    }

    /// `<row pattern initial or seek>` ::= INITIAL | SEEK, if written, as
    /// whether SEEK. Of a `<row pattern recognition clause>`, not `window`,
    /// neither is written \[SR3a\].
    fn row_pattern_initial_or_seek(&mut self, window: bool) -> Result<Option<bool>> {
        let seek = if self.is_word(INITIAL) {
            false
        } else if self.is_word(SEEK) {
            true
        } else {
            return Ok(None);
        };
        if !window {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §7.9 SR3a] a <row pattern recognition clause> specifies neither INITIAL nor SEEK"));
        }
        self.advance();
        Ok(Some(seek))
    }

    /// `<row pattern subset clause>` ::= SUBSET `<row pattern subset
    /// item>` \[{, ...}\], each a name = ( names ); none if not written.
    fn row_pattern_subset_clause(&mut self) -> Result<Vec<Subset>> {
        let mut subsets = Vec::new();
        if !self.accept_word(SUBSET) {
            return Ok(subsets);
        }
        loop {
            let at = self.here();
            let name = self.identifier()?;
            self.expect_delim(delim::EQUALS)?;
            let variables = self.name_list()?;
            subsets.push(Subset { name, variables, at });
            if !self.accept_delim(delim::COMMA) {
                return Ok(subsets);
            }
        }
    }

    /// `<row pattern definition list>` ::= `<row pattern definition>`
    /// \[{, ...}\], each a name AS `<search condition>`, DEFINE taken.
    fn row_pattern_definition_list(&mut self) -> Result<Vec<Definition>> {
        let mut defines = Vec::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            self.expect_kw(kw::AS)?;
            let condition = self.search_condition()?;
            defines.push(Definition { name, condition, at });
            if !self.accept_delim(delim::COMMA) {
                return Ok(defines);
            }
        }
    }
}

/// Whether `t` is the `<non-reserved word>` `word`.
fn is_named(t: &Tok, word: &str) -> bool {
    if let Tok::Identifier(s) = t { s == word } else { false }
}

// Unit tests: each form of the clause, its pattern and its expressions
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
