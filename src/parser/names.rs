//! [X3.135-1986 §5.4, ISO/IEC 9075:1992 §5.4] Names: a `<table name>`,
//! a `<qualifier>` and a `<column reference>`, each qualified, if written
//! so, by a schema and the one catalog. Split from parser.rs (RULES.md
//! R-SIZE-2).

use super::{Catalogs, Parser};
use crate::ast::{ColumnSpecification, TableName};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// [X3.135-1986 §5.4] `<table name>` ::= \[`<authorization identifier>`.\] `<table identifier>`
    pub(super) fn table_name(&mut self) -> Result<TableName> {
        let at = self.here();
        let first = self.qualifier()?;
        if self.accept_delim(delim::PERIOD) {
            let ident = self.identifier()?;
            // [ISO/IEC 9075:1992 §5.4] <schema name> ::= [<catalog name> .]
            // <unqualified schema name>: a name of three parts, refused.
            if self.is_delim(delim::PERIOD) && matches!(self.peek_at(1), Tok::Identifier(_)) {
                self.advance();
                self.catalog_named(&first, at)?;
                return Ok(TableName { auth: Some(ident), ident: self.identifier()?, at });
            }
            Ok(TableName { auth: Some(first), ident, at })
        } else {
            Ok(TableName { auth: None, ident: first, at })
        }
    }

    /// A `<table name>`'s, or a `<column reference>`'s, first part: an
    /// `<identifier>`, or MODULE, of a `<qualified local table name>` ::=
    /// MODULE . `<local table name>` [ISO/IEC 9075:1992 §5.4].
    pub(super) fn qualifier(&mut self) -> Result<String> {
        if self.is_kw(kw::MODULE) && matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD)) {
            self.advance();
            return Ok(kw::MODULE.to_owned());
        }
        self.identifier()
    }

    /// [X3.135-1986 §5.7] `<column specification>` ::= \[`<qualifier>`.\] `<column name>`
    /// `<qualifier>` ::= `<table name>` | `<correlation name>`
    pub(super) fn column_specification(&mut self) -> Result<ColumnSpecification> {
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
        // [ISO/IEC 9075:1992 §6.4] a <column reference> of a <qualifier>
        // with its <catalog name>.
        if self.accept_delim(delim::PERIOD) {
            self.catalog_named(&a, at)?;
            let d = self.identifier()?;
            return Ok(ColumnSpecification { qualifier: Some(TableName { auth: Some(b), ident: c, at }), column: d, at });
        }
        Ok(ColumnSpecification { qualifier: Some(TableName { auth: Some(a), ident: b, at }), column: c, at })
    }

    /// The `<catalog name>` `catalog`, written at `at` in a name [ISO/IEC
    /// 9075:1992 §5.4], taken as `catalogs` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, SQLSTATE 3D000, if it is not the one catalog's name
    /// \[SR20\]. A parser told of no catalog refuses it, SQLCODE -900, at
    /// the statement's end.
    pub(super) fn catalog_named(&mut self, catalog: &str, at: usize) -> Result<()> {
        match &self.catalogs {
            Catalogs::Kept => Ok(()),
            Catalogs::Of(ours) => {
                if catalog == ours {
                    return Ok(());
                }
                let msg = format!("[ISO/IEC 9075:1992 §5.4 SR20] the one catalog is {ours}, not {catalog}");
                Err(SqlError::at(Code::SyntaxRule, at, msg).with_state(SqlState::INVALID_CATALOG_NAME))
            }
            Catalogs::Refused => {
                let msg = "[DESIGN] a name qualified by its <catalog name> is read only where the catalog is known (RULES.md D18)";
                self.refuse_later(SqlError::at(Code::Unsupported, at, msg));
                Ok(())
            }
        }
    }
}
