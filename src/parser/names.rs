//! [X3.135-1986 §5.4, ISO/IEC 9075:1992 §5.4] Names: a `<table name>`, a
//! `<qualifier>` and a `<column reference>`, each qualified, if written
//! so, by a schema and the one catalog.

use super::{Catalogs, Parser};
use crate::ast::{ColumnSpecification, Composite, TableName, ValueExpression};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// A name of a table's form, and nothing after it: a `<collation name>`
    /// in a value [ISO/IEC 9075-2:2023 §19.10 GR1a].
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub fn only_table_name(&mut self) -> Result<TableName> {
        let name = self.table_name()?;
        if *self.peek() == Tok::End { Ok(name) } else { Err(self.syntax("expected one name")) }
    }

    /// [X3.135-1986 §5.4] `<table name>` ::= \[`<authorization identifier>`.\] `<table identifier>`
    pub(super) fn table_name(&mut self) -> Result<TableName> {
        let at = self.here();
        let first = self.qualifier()?;
        if self.accept_delim(delim::PERIOD) {
            let ident = self.identifier()?;
            // [ISO/IEC 9075:1992 §5.4] <schema name> ::= [<catalog name> .]
            // <unqualified schema name>: a name of three parts, its first
            // the <catalog name>, taken as `catalog_named` says.
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
            return Ok(ColumnSpecification { qualifier: None, column: a, catalog: None, at });
        }
        let b = self.identifier()?;
        if !self.accept_delim(delim::PERIOD) {
            return Ok(ColumnSpecification { qualifier: Some(TableName { auth: None, ident: a, at }), column: b, catalog: None, at });
        }
        let c = self.identifier()?;
        // [ISO/IEC 9075:1992 §6.4] a <column reference> of a <qualifier>
        // with its <catalog name>.
        if self.accept_delim(delim::PERIOD) {
            self.catalog_named(&a, at)?;
            let d = self.identifier()?;
            return Ok(ColumnSpecification { qualifier: Some(TableName { auth: Some(b), ident: c, at }), column: d, catalog: Some(a), at });
        }
        Ok(ColumnSpecification { qualifier: Some(TableName { auth: Some(a), ident: b, at }), column: c, catalog: None, at })
    }

    /// [ISO/IEC 9075-2:2023 §6.6] A `<column reference>` as a value: its
    /// `<identifier chain>`. Of three `<identifier>`s at most, or of four
    /// whose first may be the catalog's name, a `<column specification>`
    /// (`column_specification`). The first of four may be the catalog's
    /// name if it is this one's, or if the text is a definition's, checked
    /// when it was defined; a parser told of no catalog refuses it there.
    /// Otherwise, of four or more, the chain of a column of a row type and
    /// its fields [SR9b iv 2], which the binder resolves.
    pub(super) fn identifier_chain(&mut self) -> Result<ValueExpression> {
        let mut n: usize = 1;
        while matches!(self.peek_at(n.saturating_mul(2).saturating_sub(1)), Tok::Delimiter(delim::PERIOD))
            && matches!(self.peek_at(n.saturating_mul(2)), Tok::Identifier(_))
        {
            n = n.saturating_add(1);
        }
        let catalog = match (&self.catalogs, self.peek()) {
            (Catalogs::Refused | Catalogs::Kept, _) => true,
            (Catalogs::Of(ours), Tok::Identifier(first)) => first == ours,
            (Catalogs::Of(_), _) => false,
        };
        if n <= 3 || (n == 4 && catalog) {
            return Ok(ValueExpression::Column(self.column_specification()?));
        }
        let at = self.here();
        let mut parts = vec![self.qualifier()?];
        while parts.len() < n && self.accept_delim(delim::PERIOD) {
            parts.push(self.identifier()?);
        }
        Ok(ValueExpression::Composite(Box::new(Composite::Chain(parts)), at))
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
