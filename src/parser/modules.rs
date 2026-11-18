//! [ISO/IEC 9075-4:2006 draft §9.21, §9.24, §12.8] `<SQL-server module
//! definition>` (Feature P001).
//!
//! Its routines and temporary table declarations are read in the module:
//! MODULE.T names one of its declared local temporary tables, in the
//! module's own schema [§12.8 GR3b] (`catalog::module_schema`). A
//! routine's text is kept as the `<schema routine>` it would be, CREATE
//! then the routine, so its body is read again as any routine's is.

use super::Parser;
use crate::ast::{ModuleDefinition, RoutineDefinition, TableDefinition};
use crate::catalog::{QualifiedName, module_schema};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` of an `<SQL-server module path
/// specification>`.
pub(super) const PATH: &str = "PATH";

/// The schemas of a module's PATH, if written, each its catalog, if
/// written, and name; and the PATH as written.
type ModulePath = (Option<Vec<(Option<String>, String)>>, Option<String>);

impl Parser {
    /// This parser, of the text of a routine of the module `module`, where
    /// MODULE.T names a declared table in the schema `module` [§12.8
    /// GR3b].
    #[must_use]
    pub fn in_module(mut self, module: Option<String>) -> Parser {
        self.module = module;
        self
    }

    /// The schema MODULE . names here: the module's own, in an
    /// `<SQL-server module definition>`; else MODULE, of the session's
    /// declared tables.
    pub(super) fn module_qualifier(&self) -> String {
        self.module.clone().unwrap_or_else(|| kw::MODULE.to_owned())
    }

    /// Whether an `<SQL-server module definition>` begins here: CREATE
    /// MODULE, a name next.
    pub(super) fn next_is_module(&self) -> bool {
        self.is_kw(kw::CREATE) && self.next_is_kw(kw::MODULE) && !matches!(self.peek_at(2), Tok::Delimiter(delim::PERIOD))
    }

    /// [ISO/IEC 9075-4:2006 draft §9.21] `<SQL-server module definition>`,
    /// from CREATE, its `<schema name>`, if its name has none, `schema`.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn module_definition(&mut self, schema: Option<&str>) -> Result<ModuleDefinition> {
        let start = self.here();
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::MODULE)?;
        let name = self.table_name()?;
        self.module_character_set()?;
        let module_schema_clause = if self.accept_kw(kw::SCHEMA) { Some(self.schema_name()?.1) } else { None };
        let (path, path_text) = self.module_path()?;
        // Of a module whose schema is the session's, read again as it is
        // defined, its own schema is not known yet.
        let auth = name.auth.clone().or_else(|| schema.map(str::to_owned)).unwrap_or_default();
        let outer = self.module.replace(module_schema(&QualifiedName { auth, ident: name.ident.clone() }));
        let read = self.module_contents();
        self.module = outer;
        let (temporaries, routines) = read?;
        Ok(ModuleDefinition { name, schema: module_schema_clause, path, path_text, temporaries, routines, text: self.text_from(start) })
    }

    /// [§9.21 SR8, SR9] `<SQL-server module character set specification>`
    /// ::= NAMES ARE `<character set specification>`, if written: SQL_TEXT,
    /// the one there is.
    fn module_character_set(&mut self) -> Result<()> {
        if !self.accept_kw(kw::NAMES) {
            return Ok(());
        }
        self.expect_kw(kw::ARE)?;
        let at = self.here();
        let name = self.character_set_name()?;
        self.only_sql_text(at, &name);
        Ok(())
    }

    /// [§9.21 SR4, SR5] `<SQL-server module path specification>` ::= PATH
    /// `<schema name>` \[{, `<schema name>`}...\], if written: each
    /// schema's catalog, if written, and name; and the text.
    fn module_path(&mut self) -> Result<ModulePath> {
        if !self.is_word(PATH) {
            return Ok((None, None));
        }
        let start = self.here();
        self.advance();
        let mut schemas = Vec::new();
        loop {
            let (catalog, name, _) = self.schema_name()?;
            schemas.push((catalog, name));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok((Some(schemas), Some(self.text_from(start))))
    }

    /// `<temporary table declaration>`... `<SQL-server module contents>`...
    /// END MODULE: the declarations and the routines, at least one.
    fn module_contents(&mut self) -> Result<(Vec<TableDefinition>, Vec<RoutineDefinition>)> {
        let mut temporaries = Vec::new();
        while self.is_kw(kw::DECLARE) && self.next_is_kw(kw::LOCAL) {
            temporaries.push(self.temporary_table_declaration()?);
        }
        let mut routines = Vec::new();
        loop {
            routines.push(self.module_routine()?);
            self.expect_delim(delim::SEMICOLON)?;
            if self.accept_kw(kw::END) {
                self.expect_kw(kw::MODULE)?;
                return Ok((temporaries, routines));
            }
        }
    }

    /// [ISO/IEC 9075-4:2006 draft §9.24] `<module routine>` ::= \[DECLARE\]
    /// `<SQL-invoked procedure>` | \[DECLARE\] `<SQL-invoked function>`: its
    /// text kept as CREATE then the routine.
    fn module_routine(&mut self) -> Result<RoutineDefinition> {
        self.accept_kw(kw::DECLARE);
        let start = self.here();
        let mut r = self.invoked_routine(start)?;
        r.text = format!("{} {}", kw::CREATE, r.text);
        Ok(r)
    }
}

// Unit tests: the syntax of a module definition (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
