//! \[EXTENSION\] The grammar of a plug-in's manifest, format 1 (RULES.md
//! D168), read by SQL's own tokens, so its names and data types are
//! written as in SQL:
//!
//! ```text
//! <manifest>       ::= <manifest item> ; [ <manifest item> ; ]...
//! <manifest item>  ::= NAME <identifier> | VERSION <character string literal>
//!                    | PROTOCOL <unsigned integer>
//!                    | EXECUTABLE [ FOR <character string literal> ] <character string literal>
//!                    | REQUIRES <identifier> <character string literal>
//!                    | TURBOREL <character string literal>
//!                    | UPGRADE FROM <character string literal> SCRIPT <character string literal>
//!                    | <object declaration>
//! <object declaration> ::=
//!     FUNCTION <identifier> <parameters> RETURNS <data type> [ <determinism> ] [ BATCH <unsigned integer> ]
//!   | AGGREGATE <identifier> ( <data type> ) RETURNS <data type> [ BATCH <unsigned integer> ]
//!   | TABLE FUNCTION <identifier> <parameters> RETURNS TABLE ( <column name> <data type> [ , ... ] ) [ READS SQL DATA ]
//!   | PROCEDURE <identifier> <parameters> [ MODIFIES SQL DATA ]
//!   | METRIC <identifier> [ FORMATS ( <identifier> [ , ... ] ) ] [ PROPERTIES ( <identifier> [ , ... ] ) ] [ BATCH <unsigned integer> ]
//!   | EMBEDDING MODEL <identifier> DIMENSIONS <unsigned integer> [ ELEMENT <identifier> ] [ <determinism> ] [ BATCH <unsigned integer> ]
//!   | AUTHENTICATOR <character string literal>
//!   | TARGET <character string literal>
//!   | PARTITION STRATEGY <identifier>
//! <parameters>     ::= ( [ <data type> [ , <data type> ]... ] )
//! <determinism>    ::= DETERMINISTIC | NOT DETERMINISTIC
//! ```
//!
//! A data type is a `<data type>` or the name of a type beyond the
//! standard, as a column's is (`TypeName`).

use super::Parser;
use crate::ast::TypeName;
use crate::error::Result;
use crate::lexer::{Tok, delim};
use crate::plugin::manifest::{Declared, Item, ObjectKind};

/// The words of the manifest, in the order of its grammar.
const WORDS: [&str; 9] = ["NAME", "VERSION", "PROTOCOL", "EXECUTABLE", "FOR", "RETURNS", "BATCH", "DETERMINISTIC", "NOT"];
/// The words of a package's items.
const PACKAGE_WORDS: [&str; 5] = ["REQUIRES", "TURBOREL", "UPGRADE", "FROM", "SCRIPT"];
/// The words of the object declarations.
const KINDS: [&str; 10] =
    ["FUNCTION", "AGGREGATE", "TABLE", "PROCEDURE", "METRIC", "EMBEDDING", "AUTHENTICATOR", "TARGET", "PARTITION", "MODEL"];
/// The words of the clauses of a declaration.
const CLAUSES: [&str; 9] = ["READS", "MODIFIES", "SQL", "DATA", "FORMATS", "PROPERTIES", "DIMENSIONS", "ELEMENT", "STRATEGY"];

impl Parser {
    /// `word`, a key word or not, if it is next: whether it was.
    fn accept_manifest_word(&mut self, word: &str) -> bool {
        if self.accept_kw(word) {
            return true;
        }
        self.accept_word(word)
    }

    /// `word`, a key word or not, which must be next.
    fn expect_manifest_word(&mut self, word: &str) -> Result<()> {
        if self.accept_manifest_word(word) { Ok(()) } else { Err(self.syntax(format!("expected {word}"))) }
    }

    /// `<manifest>`, read whole: its items, in order.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 where it breaks the grammar.
    pub fn manifest(&mut self) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        while *self.peek() != Tok::End {
            items.push(self.manifest_item()?);
            self.expect_delim(delim::SEMICOLON)?;
        }
        Ok(items)
    }

    /// `<manifest item>`.
    fn manifest_item(&mut self) -> Result<Item> {
        let [name, version, protocol, executable, platform, ..] = WORDS;
        if self.accept_manifest_word(name) {
            return Ok(Item::Name(self.identifier()?));
        }
        if self.accept_manifest_word(version) {
            return Ok(Item::Version(self.character_string_literal("<version>")?));
        }
        if self.accept_manifest_word(protocol) {
            return Ok(Item::Protocol(self.unsigned_integer()?));
        }
        if self.accept_manifest_word(executable) {
            let os = if self.accept_manifest_word(platform) { Some(self.character_string_literal("<platform>")?) } else { None };
            return Ok(Item::Executable(os, self.character_string_literal("<executable>")?));
        }
        match self.package_item() {
            Some(item) => item,
            None => self.object_declaration().map(|d| Item::Object(Box::new(d))),
        }
    }

    /// REQUIRES, TURBOREL or UPGRADE, the items of a package (design
    /// document §13.2), if one is next.
    fn package_item(&mut self) -> Option<Result<Item>> {
        let [requires, turborel, upgrade, from, script] = PACKAGE_WORDS;
        if self.accept_manifest_word(requires) {
            return Some(self.identifier().and_then(|n| Ok(Item::Requires(n, self.character_string_literal("<version range>")?))));
        }
        if self.accept_manifest_word(turborel) {
            return Some(self.character_string_literal("<version range>").map(Item::Turborel));
        }
        if !self.accept_manifest_word(upgrade) {
            return None;
        }
        Some((|| {
            self.expect_manifest_word(from)?;
            let range = self.character_string_literal("<version range>")?;
            self.expect_manifest_word(script)?;
            Ok(Item::Upgrade(range, self.character_string_literal("<script>")?))
        })())
    }

    /// `<object declaration>`.
    fn object_declaration(&mut self) -> Result<Declared> {
        let [function, aggregate, table, procedure, metric, embedding, authenticator, target, partition, model] = KINDS;
        let at = self.here();
        let kind = if self.accept_manifest_word(function) {
            ObjectKind::Scalar
        } else if self.accept_manifest_word(aggregate) {
            ObjectKind::Aggregate
        } else if self.accept_manifest_word(table) {
            self.expect_manifest_word(function)?;
            ObjectKind::Table
        } else if self.accept_manifest_word(procedure) {
            ObjectKind::Procedure
        } else if self.accept_manifest_word(metric) {
            ObjectKind::Metric
        } else if self.accept_manifest_word(embedding) {
            self.expect_manifest_word(model)?;
            ObjectKind::Embedding
        } else if self.accept_manifest_word(authenticator) {
            ObjectKind::Authenticator
        } else if self.accept_manifest_word(target) {
            ObjectKind::Target
        } else if self.accept_manifest_word(partition) {
            self.expect_manifest_word(CLAUSES[8])?;
            ObjectKind::Partition
        } else {
            return Err(self.syntax("expected a <manifest item>"));
        };
        let named = matches!(kind, ObjectKind::Authenticator | ObjectKind::Target);
        let name = if named { self.character_string_literal("<name>")? } else { self.identifier()? };
        let mut d = Declared::new(kind, name, at);
        self.declaration_body(&mut d)?;
        d.text = self.text_from(at);
        Ok(d)
    }

    /// What follows a declaration's name, as its kind takes.
    fn declaration_body(&mut self, d: &mut Declared) -> Result<()> {
        match d.kind {
            ObjectKind::Scalar | ObjectKind::Aggregate | ObjectKind::Table | ObjectKind::Procedure => {
                d.parameters = self.manifest_parameters()?;
            }
            ObjectKind::Metric | ObjectKind::Embedding | ObjectKind::Authenticator | ObjectKind::Target | ObjectKind::Partition => {}
        }
        match d.kind {
            ObjectKind::Scalar | ObjectKind::Aggregate => {
                self.expect_manifest_word(WORDS[5])?;
                d.returns = Some(self.type_name()?);
            }
            ObjectKind::Table => self.returned_table(d)?,
            ObjectKind::Procedure => d.modifies = self.sql_data(CLAUSES[1])?,
            ObjectKind::Metric => self.metric_clauses(d)?,
            ObjectKind::Embedding => {
                self.expect_manifest_word(CLAUSES[6])?;
                d.dimensions = Some(self.unsigned_integer()?);
                if self.accept_manifest_word(CLAUSES[7]) {
                    d.element = Some(self.identifier()?);
                }
            }
            ObjectKind::Authenticator | ObjectKind::Target | ObjectKind::Partition => {}
        }
        d.deterministic = self.determinism()?;
        if self.accept_manifest_word(WORDS[6]) {
            d.batch = Some(self.unsigned_integer()?);
        }
        Ok(())
    }

    /// `<parameters>`.
    fn manifest_parameters(&mut self) -> Result<Vec<TypeName>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut types = Vec::new();
        if !self.accept_delim(delim::RIGHT_PAREN) {
            loop {
                types.push(self.type_name()?);
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(types)
    }

    /// RETURNS TABLE ( columns ) \[ READS SQL DATA \].
    fn returned_table(&mut self, d: &mut Declared) -> Result<()> {
        self.expect_manifest_word(WORDS[5])?;
        self.expect_manifest_word(KINDS[2])?;
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            let column = self.identifier()?;
            d.columns.push((column, self.type_name()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        d.reads = self.sql_data(CLAUSES[0])?;
        Ok(())
    }

    /// `first` SQL DATA, if `first` is next: whether it was.
    fn sql_data(&mut self, first: &str) -> Result<bool> {
        if !self.accept_manifest_word(first) {
            return Ok(false);
        }
        self.expect_manifest_word(CLAUSES[2])?;
        self.expect_manifest_word(CLAUSES[3])?;
        Ok(true)
    }

    /// A metric's FORMATS and PROPERTIES.
    fn metric_clauses(&mut self, d: &mut Declared) -> Result<()> {
        if self.accept_manifest_word(CLAUSES[4]) {
            d.formats = self.manifest_words()?;
        }
        if self.accept_manifest_word(CLAUSES[5]) {
            d.properties = self.manifest_words()?;
        }
        Ok(())
    }

    /// ( `<identifier>` \[ , ... \] ).
    fn manifest_words(&mut self) -> Result<Vec<String>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut words = vec![self.identifier()?];
        while self.accept_delim(delim::COMMA) {
            words.push(self.identifier()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(words)
    }

    /// `<determinism>`, if written: whether it is DETERMINISTIC.
    fn determinism(&mut self) -> Result<bool> {
        let [.., deterministic, not] = WORDS;
        if self.accept_manifest_word(not) {
            self.expect_manifest_word(deterministic)?;
            return Ok(false);
        }
        Ok(self.accept_manifest_word(deterministic))
    }
}
