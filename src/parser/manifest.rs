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
//!                    | LICENSE [ FILE ] <character string literal>
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
//!   | RANKING <identifier> | QUANTIZER <identifier>
//!   | COLLATION <identifier> VERSION <character string literal>
//!   | SYSTEM TABLE <identifier> ( <column name> <data type> [ , ... ] )
//!   | XQUERY FUNCTION <character string literal> <parameters> RETURNS <data type>
//!   | TYPE <identifier> [ ORDERED ]
//!   | STORAGE ENGINE <identifier> [ BATCH <unsigned integer> ]
//!   | INDEX METHOD <identifier> SUPPORTS ( <identifier> [ , ... ] ) [ BATCH <unsigned integer> ]
//!   | CREDENTIALS <identifier> | DIAGNOSTICS <identifier> [ BATCH <unsigned integer> ]
//!   | HOST <host kind> <identifier> [ ( <identifier> = <unsigned integer> | <character string literal> [ , ... ] ) ]
//!   | <setting declaration> | <reads settings>
//! <parameters>     ::= ( [ <data type> [ , <data type> ]... ] )
//! <determinism>    ::= DETERMINISTIC | NOT DETERMINISTIC
//! ```
//!
//! A data type is a `<data type>` or the name of a type beyond the
//! standard, as a column's is (`TypeName`). An XQuery function is named by
//! its expanded QName as one literal, its namespace's URI in braces before
//! its local name, `'{uri}local'` \[DESIGN\], as its local name is
//! case-sensitive and an `<identifier>` is not. A setting's declaration
//! and READS SETTINGS are `manifest/settings.rs`'s.

mod settings;

use super::Parser;
use crate::ast::TypeName;
use crate::error::Result;
use crate::lexer::{Tok, delim};
use crate::plugin::manifest::{Declared, HostDeclared, HostKind, HostOption, Item, ObjectKind};

/// The words of the manifest, in the order of its grammar.
const WORDS: [&str; 9] = ["NAME", "VERSION", "PROTOCOL", "EXECUTABLE", "FOR", "RETURNS", "BATCH", "DETERMINISTIC", "NOT"];
/// The word a host plug-in's declaration begins with (RULES.md D229).
const HOST: &str = "HOST";
/// The words of a package's items.
const PACKAGE_WORDS: [&str; 5] = ["REQUIRES", "TURBOREL", "UPGRADE", "FROM", "SCRIPT"];
/// The words of a package's license, LICENSE and LICENSE FILE.
const LICENSE_WORDS: [&str; 2] = ["LICENSE", "FILE"];
/// The word of RETURNS TABLE.
const TABLE: &str = "TABLE";
/// The words a declaration of each kind begins with, the second if it
/// has two.
const DECLARATIONS: [(&str, Option<&str>, ObjectKind); 19] = [
    ("FUNCTION", None, ObjectKind::Scalar),
    ("AGGREGATE", None, ObjectKind::Aggregate),
    ("TABLE", Some("FUNCTION"), ObjectKind::Table),
    ("PROCEDURE", None, ObjectKind::Procedure),
    ("METRIC", None, ObjectKind::Metric),
    ("EMBEDDING", Some("MODEL"), ObjectKind::Embedding),
    ("AUTHENTICATOR", None, ObjectKind::Authenticator),
    ("TARGET", None, ObjectKind::Target),
    ("PARTITION", Some("STRATEGY"), ObjectKind::Partition),
    ("RANKING", None, ObjectKind::Ranking),
    ("QUANTIZER", None, ObjectKind::Quantizer),
    ("COLLATION", None, ObjectKind::Collation),
    ("SYSTEM", Some("TABLE"), ObjectKind::SystemTable),
    ("XQUERY", Some("FUNCTION"), ObjectKind::XQuery),
    ("TYPE", None, ObjectKind::Type),
    ("STORAGE", Some("ENGINE"), ObjectKind::Engine),
    ("INDEX", Some("METHOD"), ObjectKind::IndexMethod),
    ("CREDENTIALS", None, ObjectKind::Credentials),
    ("DIAGNOSTICS", None, ObjectKind::Diagnostics),
];
/// The words of the clauses of a declaration.
const CLAUSES: [&str; 8] = ["READS", "MODIFIES", "SQL", "DATA", "FORMATS", "PROPERTIES", "DIMENSIONS", "ELEMENT"];
/// The words of the clauses of the kinds of RULES.md D220.
const NEW_CLAUSES: [&str; 2] = ["ORDERED", "SUPPORTS"];

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
        if self.accept_manifest_word(HOST) {
            return self.host_declaration().map(|d| Item::Host(Box::new(d)));
        }
        if let Some(item) = self.setting_item() {
            return item;
        }
        match self.package_item() {
            Some(item) => item,
            None => self.object_declaration().map(|d| Item::Object(Box::new(d))),
        }
    }

    /// REQUIRES, TURBOREL, LICENSE or UPGRADE, the items of a package
    /// (design document §13.2, RULES.md D265), if one is next.
    fn package_item(&mut self) -> Option<Result<Item>> {
        let [requires, turborel, upgrade, from, script] = PACKAGE_WORDS;
        if self.accept_manifest_word(requires) {
            return Some(self.identifier().and_then(|n| Ok(Item::Requires(n, self.character_string_literal("<version range>")?))));
        }
        if self.accept_manifest_word(turborel) {
            return Some(self.character_string_literal("<version range>").map(Item::Turborel));
        }
        let [license, file] = LICENSE_WORDS;
        if self.accept_manifest_word(license) {
            if self.accept_manifest_word(file) {
                return Some(self.character_string_literal("<license file>").map(Item::LicenseFile));
            }
            return Some(self.character_string_literal("<license expression>").map(Item::License));
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

    /// `<host declaration>`, after HOST (RULES.md D229).
    fn host_declaration(&mut self) -> Result<HostDeclared> {
        let at = self.here();
        let kind = self.host_kind()?;
        let name = self.identifier()?;
        let mut options = Vec::new();
        if self.accept_delim(delim::LEFT_PAREN) {
            loop {
                let option = self.identifier()?;
                self.expect_delim(delim::EQUALS)?;
                let value = if let Tok::CharacterString(_) = self.peek() {
                    HostOption::Text(self.character_string_literal("<option value>")?)
                } else {
                    HostOption::Number(u64::from(self.unsigned_integer()?))
                };
                options.push((option, value));
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(HostDeclared { kind, name, options, text: self.text_from(at) })
    }

    /// The words of a host declaration's kind.
    fn host_kind(&mut self) -> Result<HostKind> {
        for kind in HostKind::ALL {
            let [first, rest @ ..] = kind.words() else { continue };
            if self.accept_manifest_word(first) {
                for word in rest {
                    self.expect_manifest_word(word)?;
                }
                return Ok(kind);
            }
        }
        Err(self.syntax("expected the kind of a host object"))
    }

    /// `<object declaration>`.
    fn object_declaration(&mut self) -> Result<Declared> {
        let at = self.here();
        let kind = self.object_kind()?;
        let named = matches!(kind, ObjectKind::Authenticator | ObjectKind::Target | ObjectKind::XQuery);
        let name = if named { self.character_string_literal("<name>")? } else { self.identifier()? };
        let mut d = Declared::new(kind, name, at);
        self.declaration_body(&mut d)?;
        d.text = self.text_from(at);
        Ok(d)
    }

    /// The words a declaration begins with: its kind.
    fn object_kind(&mut self) -> Result<ObjectKind> {
        for (first, second, kind) in DECLARATIONS {
            if self.accept_manifest_word(first) {
                if let Some(second) = second {
                    self.expect_manifest_word(second)?;
                }
                return Ok(kind);
            }
        }
        Err(self.syntax("expected a <manifest item>"))
    }

    /// What follows a declaration's name, as its kind takes.
    fn declaration_body(&mut self, d: &mut Declared) -> Result<()> {
        match d.kind {
            ObjectKind::Scalar | ObjectKind::Aggregate | ObjectKind::Table | ObjectKind::Procedure | ObjectKind::XQuery => {
                d.parameters = self.manifest_parameters()?;
            }
            ObjectKind::Metric
            | ObjectKind::Embedding
            | ObjectKind::Authenticator
            | ObjectKind::Target
            | ObjectKind::Partition
            | ObjectKind::Ranking
            | ObjectKind::Quantizer
            | ObjectKind::Collation
            | ObjectKind::SystemTable
            | ObjectKind::Type
            | ObjectKind::Engine
            | ObjectKind::IndexMethod
            | ObjectKind::Credentials
            | ObjectKind::Diagnostics => {}
        }
        self.kind_clauses(d)?;
        d.deterministic = self.determinism()?;
        if self.accept_manifest_word(WORDS[6]) {
            d.batch = Some(self.unsigned_integer()?);
        }
        Ok(())
    }

    /// The clauses of `d`'s kind, after its parameters.
    fn kind_clauses(&mut self, d: &mut Declared) -> Result<()> {
        match d.kind {
            ObjectKind::Scalar | ObjectKind::Aggregate | ObjectKind::XQuery => {
                self.expect_manifest_word(WORDS[5])?;
                d.returns = Some(self.type_name()?);
            }
            ObjectKind::Table => {
                self.expect_manifest_word(WORDS[5])?;
                self.expect_manifest_word(TABLE)?;
                self.manifest_columns(d)?;
                d.reads = self.sql_data(CLAUSES[0])?;
            }
            ObjectKind::Procedure => d.modifies = self.sql_data(CLAUSES[1])?,
            ObjectKind::Metric => self.metric_clauses(d)?,
            ObjectKind::Embedding => {
                self.expect_manifest_word(CLAUSES[6])?;
                d.dimensions = Some(self.unsigned_integer()?);
                if self.accept_manifest_word(CLAUSES[7]) {
                    d.element = Some(self.identifier()?);
                }
            }
            ObjectKind::Collation => {
                self.expect_manifest_word(WORDS[1])?;
                d.version = Some(self.character_string_literal("<version>")?);
            }
            ObjectKind::SystemTable => self.manifest_columns(d)?,
            ObjectKind::Type => {
                if self.accept_manifest_word(NEW_CLAUSES[0]) {
                    d.properties.push(NEW_CLAUSES[0].to_owned());
                }
            }
            ObjectKind::IndexMethod => {
                self.expect_manifest_word(NEW_CLAUSES[1])?;
                d.supports = self.manifest_words()?;
            }
            ObjectKind::Authenticator
            | ObjectKind::Target
            | ObjectKind::Partition
            | ObjectKind::Ranking
            | ObjectKind::Quantizer
            | ObjectKind::Engine
            | ObjectKind::Credentials
            | ObjectKind::Diagnostics => {}
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

    /// ( `<column name>` `<data type>` \[ , ... \] ), a table's columns.
    fn manifest_columns(&mut self, d: &mut Declared) -> Result<()> {
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            let column = self.identifier()?;
            d.columns.push((column, self.type_name()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)
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
