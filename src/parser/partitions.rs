//! \[EXTENSION\] Partitioning's syntax (RULES.md D153), in the standard's
//! notation, as no standard defines it:
//!
//! ```text
//! <partition by clause> ::= PARTITION BY <partition strategy name>
//!     ( <partition key part> [ { , <partition key part> }... ] )
//! <partition key part> ::= <column name> | ( <value expression> )
//! <partition of clause> ::= PARTITION OF <table name> <partition bound specification>
//! <partition bound specification> ::= FOR VALUES <partition bound> | DEFAULT
//! <partition bound> ::= FROM ( <range datum> [ { , <range datum> }... ] )
//!         TO ( <range datum> [ { , <range datum> }... ] )
//!     | IN ( <list datum> [ { , <list datum> }... ] )
//!     | WITH ( <bound option> [ { , <bound option> }... ] )
//! <range datum> ::= <literal> | MINVALUE | MAXVALUE
//! <list datum> ::= <literal> | NULL | ( <literal or null> [ { , <literal or null> }... ] )
//! <bound option> ::= <identifier> <signed numeric literal>
//! <alter table action> ::= ... | ATTACH PARTITION <table name> <partition bound specification>
//!     | DETACH PARTITION <table name>
//! ```
//!
//! Its words are `<non-reserved word>`s.

use super::Parser;
use crate::ast::{AlterAction, BoundDatum, BoundSpec, PartitionBy, PartitionKeyPart, PartitionOfClause, TablePartitioning};
use crate::error::Result;
use crate::lexer::{delim, kw};

/// The words of partitioning's syntax.
pub(super) const PARTITION: &str = "PARTITION";
/// ATTACH PARTITION.
const ATTACH: &str = "ATTACH";
/// DETACH PARTITION.
const DETACH: &str = "DETACH";
/// A range bound below every value.
const MINVALUE: &str = "MINVALUE";
/// A range bound above every value.
const MAXVALUE: &str = "MAXVALUE";

impl Parser {
    /// A table's partitioning: `of`, and `by`, else a `<partition by
    /// clause>` if it is next, written after the table's other clauses.
    pub(super) fn table_partitioning(
        &mut self,
        of: Option<PartitionOfClause>,
        by: Option<PartitionBy>,
    ) -> Result<Option<Box<TablePartitioning>>> {
        let by = match by {
            Some(given) => Some(given),
            None => self.partition_by()?,
        };
        Ok((by.is_some() || of.is_some()).then(|| Box::new(TablePartitioning { by, of })))
    }

    /// `<partition by clause>`, if it is next.
    pub(super) fn partition_by(&mut self) -> Result<Option<PartitionBy>> {
        if !self.accept_word(PARTITION) {
            return Ok(None);
        }
        self.expect_kw(kw::BY)?;
        let at = self.here();
        let strategy = self.identifier()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut parts = Vec::new();
        loop {
            let part_at = self.here();
            if self.accept_delim(delim::LEFT_PAREN) {
                let start = self.here();
                self.value_expression()?;
                parts.push(PartitionKeyPart::Expression(self.text_from(start), part_at));
                self.expect_delim(delim::RIGHT_PAREN)?;
            } else {
                parts.push(PartitionKeyPart::Column(self.identifier()?, at));
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(PartitionBy { strategy, at, parts }))
    }

    /// `<partition of clause>`, if it is next.
    pub(super) fn partition_of(&mut self) -> Result<Option<PartitionOfClause>> {
        if !self.accept_word(PARTITION) {
            return Ok(None);
        }
        self.expect_kw(kw::OF)?;
        let parent = self.table_name()?;
        let at = self.here();
        Ok(Some(PartitionOfClause { parent, bound: self.bound_specification()?, at }))
    }

    /// ATTACH PARTITION or DETACH PARTITION, if next.
    pub(super) fn partition_action(&mut self) -> Result<Option<AlterAction>> {
        if self.accept_word(ATTACH) {
            self.expect_word(PARTITION)?;
            let table = self.table_name()?;
            return Ok(Some(AlterAction::AttachPartition { table, bound: self.bound_specification()? }));
        }
        if self.accept_word(DETACH) {
            self.expect_word(PARTITION)?;
            return Ok(Some(AlterAction::DetachPartition(self.table_name()?)));
        }
        Ok(None)
    }

    /// `<partition bound specification>`.
    fn bound_specification(&mut self) -> Result<BoundSpec> {
        if self.accept_kw(kw::DEFAULT) {
            return Ok(BoundSpec::Default);
        }
        self.expect_kw(kw::FOR)?;
        self.expect_kw(kw::VALUES)?;
        if self.accept_kw(kw::FROM) {
            let from = self.datums()?;
            self.expect_kw(kw::TO)?;
            return Ok(BoundSpec::Range { from, to: self.datums()? });
        }
        if self.accept_kw(kw::IN) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let mut keys = vec![self.list_datum()?];
            while self.accept_delim(delim::COMMA) {
                keys.push(self.list_datum()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(BoundSpec::List(keys));
        }
        self.expect_kw(kw::WITH)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut options = Vec::new();
        loop {
            let name = self.identifier()?;
            options.push((name, self.datum()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(BoundSpec::With(options))
    }

    /// ( `<range datum>` \[{, `<range datum>`}...\] )
    fn datums(&mut self) -> Result<Vec<BoundDatum>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut datums = vec![self.datum()?];
        while self.accept_delim(delim::COMMA) {
            datums.push(self.datum()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(datums)
    }

    /// `<list datum>`: a key of one part, or of several in parentheses.
    fn list_datum(&mut self) -> Result<Vec<BoundDatum>> {
        if self.is_delim(delim::LEFT_PAREN) {
            return self.datums();
        }
        Ok(vec![self.datum()?])
    }

    /// A `<literal>`, NULL, MINVALUE or MAXVALUE.
    fn datum(&mut self) -> Result<BoundDatum> {
        if self.accept_kw(kw::NULL) {
            return Ok(BoundDatum::Null);
        }
        if self.accept_word(MINVALUE) {
            return Ok(BoundDatum::MinValue);
        }
        if self.accept_word(MAXVALUE) {
            return Ok(BoundDatum::MaxValue);
        }
        Ok(BoundDatum::Literal(self.literal()?))
    }
}
