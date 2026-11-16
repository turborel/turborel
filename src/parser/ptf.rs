//! [ISO/IEC 9075-2:2023 §11.60, §7.6, §10.4, §20.16] The syntax of
//! polymorphic table functions (Feature B200): a `<generic table parameter
//! type>` or `<descriptor parameter type>`, a `<returns table type>` and a
//! `<polymorphic table function body>` of their definitions; and their
//! invocation in a `<PTF derived table>`, its `<table argument>`s,
//! `<descriptor argument>`s and `<co-partition clause>`.
//!
//! PASS, THROUGH, SEMANTICS, PRUNE, KEEP, EMPTY, FINISH, PRIVATE, START,
//! FULFILL and COPARTITION are no key words of SQL-92, so each stays an
//! identifier, read as a word here.
//!
//! In an argument list, a `<routine invocation>` whose own argument list
//! holds TABLE, DESCRIPTOR or COPARTITION is read as a `<table function
//! invocation>`; any other as a `<value expression>`, which the binder
//! reads as a table function invocation where its parameter is a generic
//! table parameter [§9.18 SR12b ii].

mod arguments;
mod descriptors;

use super::Parser;
use super::routines::{FULFILL, INOUT, OUT, PRIVATE, RESULT, START, STATIC, not_supported};
use crate::ast::{
    GenericTable, ParameterDeclaration, ParameterType, PrivateParameter, PtfDefinition, RoutineDesignator, TableSemantics, TypeName,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{delim, kw};

// [ISO/IEC 9075-2:2023 §11.60] The words of the Formats of a polymorphic
// table function's definition that are no key words of SQL-92.

/// Of `<pass through option>`.
const PASS: &str = "PASS";
/// Of `<pass through option>`.
const THROUGH: &str = "THROUGH";
/// Of `<generic table semantics>`.
const SEMANTICS: &str = "SEMANTICS";
/// Of `<generic table pruning>`.
pub(super) const PRUNE: &str = "PRUNE";
/// Of `<generic table pruning>`.
pub(super) const KEEP: &str = "KEEP";
/// Of `<polymorphic table function body>`.
const FINISH: &str = "FINISH";
/// Of `<generic table semantics>`.
const ROW: &str = "ROW";
/// Of `<PTF private parameters>`.
const DATA: &str = "DATA";
/// Of `<generic table pruning>`.
const EMPTY: &str = "EMPTY";

/// A `<returns table type>`: its `<table function column list>`, if
/// written, and whether ONLY PASS THROUGH.
pub(super) type ReturnsTable = (Option<Vec<(String, TypeName, usize)>>, bool);

/// Whether one of `parameters` is a generic table parameter.
pub(super) fn has_table_parameter(parameters: &[ParameterDeclaration]) -> bool {
    parameters.iter().any(|p| matches!(p.data_type, ParameterType::Table(_)))
}

/// [§11.60 SR10e i] A private parameter declares none of these.
fn private_refused(at: usize, what: &str) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR10e i] a private parameter of a polymorphic table function has no {what}");
    SqlError::at(Code::SyntaxRule, at, msg)
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §11.60 SR4, SR7a] The `<returns table type>`
    /// `table`, if written, of a routine of `parameters`: that of a
    /// polymorphic table function, none of a monomorphic table function
    /// [SR4b], which Feature T326 makes; and no routine but a polymorphic
    /// table function has a generic table or descriptor parameter.
    pub(super) fn polymorphic(&self, parameters: &[ParameterDeclaration], table: Option<ReturnsTable>) -> Result<Option<ReturnsTable>> {
        let generic = parameters.iter().find(|p| !matches!(p.data_type, ParameterType::Data(_)));
        match (table, generic) {
            (Some((Some(_), false)), None) => {
                let msg = "[ISO/IEC 9075-2:2023 §11.60 SR4b, CR25] a monomorphic table function needs Feature T326, “Table functions”, \
                           which turborel does not support";
                Err(not_supported(self.here(), msg))
            }
            (Some(table), _) => Ok(Some(table)),
            (None, Some(p)) => Err(SqlError::at(
                Code::SyntaxRule,
                p.at,
                "[ISO/IEC 9075-2:2023 §11.60 SR7a] only a polymorphic table function, which returns a table, has a <generic table \
                 parameter type> or <descriptor parameter type>",
            )),
            (None, None) => Ok(None),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR10a, SR10d] The `<polymorphic table
    /// function body>` next, of a function of the `<returns table type>`
    /// `table`: no `<dispatch clause>` before it, and not omitted.
    pub(super) fn polymorphic_body(&mut self, table: ReturnsTable) -> Result<PtfDefinition> {
        if self.is_word(STATIC) {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §11.60 SR10a] a polymorphic table function has no <dispatch clause>"));
        }
        if !self.at_ptf_body() {
            return Err(
                self.rule("[ISO/IEC 9075-2:2023 §11.60 SR10d] a polymorphic table function has a <polymorphic table function body>")
            );
        }
        self.ptf_body(table)
    }

    /// [§11.60] The `<generic table parameter type>` or `<descriptor
    /// parameter type>` next, if one is.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn generic_parameter_type(&mut self) -> Result<Option<ParameterType>> {
        if self.accept_kw(kw::DESCRIPTOR) {
            return Ok(Some(ParameterType::Descriptor));
        }
        if !self.accept_kw(kw::TABLE) {
            return Ok(None);
        }
        let pass_through = if self.accept_kw(kw::NO) {
            self.expect_word(PASS)?;
            self.expect_word(THROUGH)?;
            false
        } else if self.accept_word(PASS) {
            self.expect_word(THROUGH)?;
            true
        } else {
            false
        };
        Ok(Some(ParameterType::Table(GenericTable { pass_through, semantics: self.generic_table_semantics()? })))
    }

    /// [§11.60] `<generic table semantics>` ::= WITH ROW SEMANTICS | WITH
    /// SET SEMANTICS \[`<generic table pruning>`\], WITH SET SEMANTICS KEEP
    /// ON EMPTY if none is written [SR10g ii 3, 4].
    fn generic_table_semantics(&mut self) -> Result<TableSemantics> {
        if !self.accept_kw(kw::WITH) {
            return Ok(TableSemantics::Set { prune: false });
        }
        if self.accept_word(ROW) {
            self.expect_word(SEMANTICS)?;
            return Ok(TableSemantics::Row);
        }
        self.expect_kw(kw::SET)?;
        self.expect_word(SEMANTICS)?;
        let prune = if self.accept_word(PRUNE) {
            true
        } else if self.accept_word(KEEP) {
            false
        } else {
            return Ok(TableSemantics::Set { prune: false });
        };
        self.expect_kw(kw::ON)?;
        self.expect_word(EMPTY)?;
        Ok(TableSemantics::Set { prune })
    }

    /// Whether a `<returns table type>` is next.
    pub(super) fn at_returns_table(&self) -> bool {
        self.is_kw(kw::TABLE) || self.is_kw(kw::ONLY)
    }

    /// [§11.60] `<returns table type>` ::= TABLE \[`<table function column
    /// list>`\] | ONLY PASS THROUGH: the columns of the list, if written,
    /// and whether ONLY PASS THROUGH, no two columns of one name [SR6a].
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn returns_table_type(&mut self) -> Result<ReturnsTable> {
        if self.accept_kw(kw::ONLY) {
            self.expect_word(PASS)?;
            self.expect_word(THROUGH)?;
            return Ok((None, true));
        }
        self.expect_kw(kw::TABLE)?;
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok((None, false));
        }
        let mut columns: Vec<(String, TypeName, usize)> = Vec::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            if columns.iter().any(|(n, _, _)| *n == name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR6a] column {name} is named twice in the <table function column list>");
                return Err(SqlError::at(Code::SyntaxRule, at, msg));
            }
            columns.push((name, self.type_name()?, at));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok((Some(columns), false))
    }

    /// Whether a `<polymorphic table function body>` begins here: PRIVATE,
    /// or DESCRIBE, START or FULFILL before WITH.
    pub(super) fn at_ptf_body(&self) -> bool {
        let component = self.is_kw(kw::DESCRIBE) || self.is_word(START) || self.is_word(FULFILL);
        self.is_word(PRIVATE) || (component && self.next_is_kw(kw::WITH))
    }

    /// [§11.60] `<polymorphic table function body>` ::= \[`<PTF private
    /// parameters>`\] \[DESCRIBE WITH ...\] \[START WITH ...\] FULFILL WITH
    /// ... \[FINISH WITH ...\], each procedure a `<specific routine
    /// designator>`; of a function of the `<returns table type>` given.
    ///
    /// # Errors
    ///
    /// The text is not one, or breaks SR10e i.
    pub(super) fn ptf_body(&mut self, (columns, only_pass_through): ReturnsTable) -> Result<PtfDefinition> {
        let private = if self.accept_word(PRIVATE) {
            self.accept_word(DATA);
            self.private_parameters()?
        } else {
            Vec::new()
        };
        let describe = self.component(kw::DESCRIBE)?;
        let start = self.component(START)?;
        let Some(fulfill) = self.component(FULFILL)? else { return Err(self.syntax("expected FULFILL WITH")) };
        let finish = self.component(FINISH)?;
        Ok(PtfDefinition { columns, only_pass_through, private, describe, start, fulfill, finish })
    }

    /// `word` WITH `<specific routine designator>`, if `word` is next.
    fn component(&mut self, word: &str) -> Result<Option<RoutineDesignator>> {
        let next = if word == kw::DESCRIBE { self.is_kw(word) } else { self.is_word(word) };
        if !next || !self.next_is_kw(kw::WITH) {
            return Ok(None);
        }
        self.advance();
        self.advance();
        self.routine_designator().map(Some)
    }

    /// [§11.60] `<private parameter declaration list>` ::= ( \[`<SQL
    /// parameter declaration>` \[{, ...}...\]\] ), each a name, a `<data
    /// type>` and perhaps a `<parameter default>` [SR10e i].
    fn private_parameters(&mut self) -> Result<Vec<PrivateParameter>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut list: Vec<PrivateParameter> = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(list);
        }
        loop {
            list.push(self.private_parameter()?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(list)
    }

    /// One private `<SQL parameter declaration>`: no `<parameter mode>`,
    /// `<locator indication>`, RESULT, ROW type, generic table or
    /// descriptor type [SR10e i].
    fn private_parameter(&mut self) -> Result<PrivateParameter> {
        let at = self.here();
        if self.is_kw(kw::IN) || self.is_word(OUT) || self.is_word(INOUT) {
            return Err(private_refused(at, "<parameter mode>"));
        }
        let name = self.identifier()?;
        let type_at = self.here();
        if self.at_row_type() || self.is_kw(kw::TABLE) || self.is_kw(kw::DESCRIPTOR) {
            return Err(private_refused(type_at, "ROW, <generic table parameter type> or <descriptor parameter type>"));
        }
        let data_type = self.type_name()?;
        if self.is_kw(kw::AS) || self.is_word(RESULT) {
            return Err(private_refused(self.here(), "<locator indication> or RESULT"));
        }
        let default = if self.accept_kw(kw::DEFAULT) {
            let start = self.here();
            // <parameter default> ::= <value expression> | <contextually
            // typed value specification>, of which NULL is not a value
            // expression.
            let value = if self.accept_kw(kw::NULL) { None } else { Some(self.value_expression()?) };
            Some((value, self.text_from(start)))
        } else {
            None
        };
        Ok(PrivateParameter { name, data_type, default, at })
    }
}
