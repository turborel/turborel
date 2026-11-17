//! [ISO/IEC 9075-2:2023 §7.6, §10.4, §20.16] A `<PTF derived table>` and
//! the `<SQL argument list>` of its `<routine invocation>`: `<table
//! argument>`s, `<descriptor argument>`s, `<value expression>`s and its
//! `<co-partition clause>`.

use super::{KEEP, PRUNE};
use crate::ast::{
    ColumnSpecification, DescriptorColumn, NamedTable, PtfArgument, PtfInvocation, PtfTable, TableArgument, TableArgumentProper, TableName,
    TableReference, ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;
use crate::parser::constraints::NULLS;
use crate::parser::query::PARTITION;
use crate::parser::routines::COPARTITION;

/// Of `<table argument pruning>`.
const EMPTY: &str = "EMPTY";

/// The tokens of CAST ( NULL AS DESCRIPTOR ) after CAST: (, NULL, AS and
/// DESCRIPTOR.
const NULL_DESCRIPTOR_TOKENS: usize = 4;

/// The tokens of a schema-qualified `<routine name>`: the schema, a period
/// and the name.
const QUALIFIED_TOKENS: usize = 3;

/// Whether `tok` begins nothing but a `<table argument>` or a `<descriptor
/// argument>`, or is COPARTITION: what shows a `<routine invocation>` is
/// a `<table function invocation>`.
fn polymorphic_token(tok: &Tok) -> bool {
    matches!(tok, Tok::KeyWord(kw::TABLE | kw::DESCRIPTOR)) || matches!(tok, Tok::Identifier(w) if w == COPARTITION)
}

impl Parser {
    /// [§7.6] `<PTF derived table>` ::= TABLE ( `<routine invocation>` ),
    /// then \[\[AS\] `<correlation name>` \[( `<derived column list>` )\]\].
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn ptf_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.expect_kw(kw::TABLE)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let invocation = self.nested(Self::ptf_invocation)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        Ok(TableReference::Polymorphic(Box::new(PtfTable { invocation, correlation, columns, at })))
    }

    /// Whether `TABLE (` begins a `<PTF derived table>` here.
    pub(in crate::parser) fn at_ptf_table(&self) -> bool {
        self.is_kw(kw::TABLE)
            && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
            && matches!(self.peek_at(2), Tok::Identifier(_))
    }

    /// [§10.4] `<routine invocation>` ::= `<routine name>` ( \[`<SQL
    /// argument>` \[{, ...}...\] \[`<co-partition clause>`\]\] ).
    fn ptf_invocation(&mut self) -> Result<PtfInvocation> {
        let at = self.here();
        let mut name = self.identifier()?;
        let mut schema = None;
        if self.accept_delim(delim::PERIOD) {
            schema = Some(std::mem::replace(&mut name, self.identifier()?));
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut arguments = Vec::new();
        let mut copartition = Vec::new();
        if !self.accept_delim(delim::RIGHT_PAREN) {
            loop {
                arguments.push(self.ptf_argument()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            if self.accept_word(COPARTITION) {
                copartition = self.copartition_list()?;
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(PtfInvocation { schema, name, arguments, copartition, at, extension: self.extensions })
    }

    /// Whether a `<table function invocation>` is next: a `<routine name>`
    /// and an argument list holding, at any depth, what only a
    /// polymorphic table function's may (`polymorphic_token`).
    fn at_table_function_invocation(&self) -> bool {
        let Tok::Identifier(_) = self.peek() else { return false };
        let open = if matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD)) { QUALIFIED_TOKENS } else { 1 };
        if !matches!(self.peek_at(open), Tok::Delimiter(delim::LEFT_PAREN)) {
            return false;
        }
        let mut depth = 0_usize;
        for n in open.. {
            let tok = self.peek_at(n);
            if *tok == Tok::End {
                return false;
            }
            if polymorphic_token(tok) {
                return true;
            }
            if *tok == Tok::Delimiter(delim::LEFT_PAREN) {
                depth = depth.saturating_add(1);
            } else if *tok == Tok::Delimiter(delim::RIGHT_PAREN) {
                depth = depth.saturating_sub(1);
            }
            if depth == 0 {
                return false;
            }
        }
        false
    }

    /// [§10.4] An `<SQL argument>` of a polymorphic table function: a
    /// `<table argument>`, a `<descriptor argument>`, NULL or DEFAULT, or a
    /// `<value expression>`; a named one needs Feature T524.
    fn ptf_argument(&mut self) -> Result<PtfArgument> {
        let at = self.here();
        if matches!(self.peek(), Tok::Identifier(_))
            && *self.peek_at(1) == Tok::Delimiter(delim::EQUALS)
            && *self.peek_at(2) == Tok::Delimiter(delim::GREATER)
        {
            let msg = "[ISO/IEC 9075-2:2023 §10.4 CR4] a <named argument specification> needs Feature T524, “Named arguments in routine \
                       invocations other than a CALL statement”, which turborel does not support";
            return Err(SqlError::at(Code::Unsupported, at, msg));
        }
        if self.is_kw(kw::DESCRIPTOR) || self.at_null_descriptor() {
            return self.descriptor_argument();
        }
        // [§9.18 SR12e i 2] DEFAULT of a parameter of no default, which no
        // parameter of a function has (Feature T525), is NULL.
        if (self.is_kw(kw::NULL) || self.is_kw(kw::DEFAULT)) && matches!(self.peek_at(1), Tok::Delimiter(delim::COMMA | delim::RIGHT_PAREN))
        {
            self.advance();
            return Ok(PtfArgument::Null(at));
        }
        if self.is_kw(kw::TABLE) {
            self.advance();
            let proper = if self.at_subquery() {
                TableArgumentProper::Query(Box::new(self.subquery()?))
            } else {
                self.expect_delim(delim::LEFT_PAREN)?;
                let name = self.table_name()?;
                self.expect_delim(delim::RIGHT_PAREN)?;
                TableArgumentProper::Named(NamedTable { table: name, only: false, correlation: None, columns: None, system_time: None })
            };
            return self.table_argument(proper, at);
        }
        if self.at_table_function_invocation() {
            let invocation = self.nested(Self::ptf_invocation)?;
            return self.table_argument(TableArgumentProper::Invocation(Box::new(invocation)), at);
        }
        let value = self.value_expression()?;
        if self.at_table_argument_tail() {
            if let ValueExpression::Call { schema, name, args, at: call_at, extension } = value {
                let arguments = args.into_iter().map(PtfArgument::Value).collect();
                let invocation = PtfInvocation { schema, name, arguments, copartition: Vec::new(), at: call_at, extension };
                return self.table_argument(TableArgumentProper::Invocation(Box::new(invocation)), at);
            }
            return Err(self.syntax("expected \",\" or \")\""));
        }
        Ok(PtfArgument::Value(value))
    }

    /// Whether what follows a `<table argument proper>` is next: AS, a
    /// correlation name, PARTITION BY, PRUNE or KEEP WHEN EMPTY, or ORDER
    /// BY.
    fn at_table_argument_tail(&self) -> bool {
        match self.peek() {
            Tok::KeyWord(k) => *k == kw::AS || *k == kw::ORDER,
            Tok::Identifier(w) => *w != COPARTITION,
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        }
    }

    /// Whether CAST ( NULL AS DESCRIPTOR ) is next.
    fn at_null_descriptor(&self) -> bool {
        self.is_kw(kw::CAST) && self.next_is_null_descriptor()
    }

    /// Whether ( NULL AS DESCRIPTOR ) follows the token at hand.
    pub(in crate::parser) fn next_is_null_descriptor(&self) -> bool {
        matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
            && matches!(self.peek_at(2), Tok::KeyWord(kw::NULL))
            && matches!(self.peek_at(3), Tok::KeyWord(kw::AS))
            && matches!(self.peek_at(4), Tok::KeyWord(kw::DESCRIPTOR))
    }

    /// [§10.4] `<descriptor argument>` ::= `<descriptor value
    /// constructor>` | CAST ( NULL AS DESCRIPTOR ).
    fn descriptor_argument(&mut self) -> Result<PtfArgument> {
        let at = self.here();
        if self.accept_kw(kw::CAST) {
            (0..NULL_DESCRIPTOR_TOKENS).for_each(|_| self.advance());
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(PtfArgument::Descriptor(None, at));
        }
        Ok(PtfArgument::Descriptor(Some(self.descriptor_value_constructor()?), at))
    }

    /// [§20.16] `<descriptor value constructor>` ::= DESCRIPTOR (
    /// `<descriptor column list>` ): its columns, each a `<column name>`
    /// and perhaps a `<data type>`, no two of equivalent names \[SR1\].
    ///
    /// # Errors
    ///
    /// The text is not one, or names a column twice.
    pub(in crate::parser) fn descriptor_value_constructor(&mut self) -> Result<Vec<DescriptorColumn>> {
        self.expect_kw(kw::DESCRIPTOR)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns: Vec<DescriptorColumn> = Vec::new();
        loop {
            let column_at = self.here();
            let name = self.identifier()?;
            if columns.iter().any(|c| c.name == name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §20.16 SR1] column {name} is named twice in the <descriptor column list>");
                return Err(SqlError::at(Code::SyntaxRule, column_at, msg));
            }
            let named_only = self.is_delim(delim::COMMA) || self.is_delim(delim::RIGHT_PAREN);
            let data_type = if named_only { None } else { Some(self.type_name()?) };
            columns.push(DescriptorColumn { name, data_type, at: column_at });
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// The rest of a `<table argument>` after its `<table argument proper>`
    /// `proper`, which begins at `at`.
    fn table_argument(&mut self, proper: TableArgumentProper, at: usize) -> Result<PtfArgument> {
        let correlation = self.table_argument_correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        let partition = if self.is_word(PARTITION) && self.next_is_kw(kw::BY) {
            self.advance();
            self.advance();
            Some(self.partitioning_list()?)
        } else {
            None
        };
        let prune = self.table_argument_pruning()?;
        let order = if self.is_kw(kw::ORDER) {
            self.advance();
            self.expect_kw(kw::BY)?;
            self.table_argument_ordering()?
        } else {
            Vec::new()
        };
        Ok(PtfArgument::Table(Box::new(TableArgument { proper, correlation, columns, partition, prune, order, at })))
    }

    /// \[\[AS\] `<table argument correlation name>`\], if written: no word
    /// that begins what may follow it.
    fn table_argument_correlation(&mut self) -> Result<Option<String>> {
        if self.accept_kw(kw::AS) {
            return self.identifier().map(Some);
        }
        let Tok::Identifier(w) = self.peek() else { return Ok(None) };
        let pruning = (w == PRUNE || w == KEEP) && self.next_is_kw(kw::WHEN);
        let partitioning = w == PARTITION && self.next_is_kw(kw::BY);
        if pruning || partitioning || w == COPARTITION {
            return Ok(None);
        }
        self.identifier().map(Some)
    }

    /// `<table argument pruning>` ::= PRUNE WHEN EMPTY | KEEP WHEN EMPTY, if
    /// written: whether PRUNE.
    fn table_argument_pruning(&mut self) -> Result<Option<bool>> {
        let prune = self.is_word(PRUNE);
        if !(prune || self.is_word(KEEP)) || !self.next_is_kw(kw::WHEN) {
            return Ok(None);
        }
        self.advance();
        self.advance();
        self.expect_word(EMPTY)?;
        Ok(Some(prune))
    }

    /// `<table argument partitioning list>` ::= `<column reference>` | (
    /// \[`<column reference>` \[{, ...}...\]\] ).
    fn partitioning_list(&mut self) -> Result<Vec<ColumnSpecification>> {
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok(vec![self.column_specification()?]);
        }
        let mut list = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(list);
        }
        loop {
            list.push(self.column_specification()?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(list)
    }

    /// `<table argument ordering list>` ::= `<table argument ordering
    /// column>` | ( `<table argument ordering column>` \[{, ...}...\] ),
    /// each a `<column reference>` \[ASC | DESC\] \[`<null ordering>`\].
    fn table_argument_ordering(&mut self) -> Result<Vec<(ColumnSpecification, bool, Option<bool>)>> {
        let listed = self.accept_delim(delim::LEFT_PAREN);
        let mut list = Vec::new();
        loop {
            let column = self.column_specification()?;
            let descending = self.accept_kw(kw::DESC);
            if !descending {
                self.accept_kw(kw::ASC);
            }
            let nulls_first = if self.is_word(NULLS) { self.null_ordering()? } else { None };
            list.push((column, descending, nulls_first));
            if !listed || !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        if listed {
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(list)
    }

    /// `<co-partition list>` ::= `<co-partition specification>` \[{, ...}...\],
    /// each ( `<range variable>` , `<range variable>` \[{, ...}...\] ).
    fn copartition_list(&mut self) -> Result<Vec<Vec<(TableName, usize)>>> {
        let mut list = Vec::new();
        loop {
            self.expect_delim(delim::LEFT_PAREN)?;
            let mut names = Vec::new();
            loop {
                let at = self.here();
                names.push((self.table_name()?, at));
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            if names.len() < 2 {
                return Err(self.syntax("expected \",\" and a second <range variable>"));
            }
            list.push(names);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(list)
    }
}
