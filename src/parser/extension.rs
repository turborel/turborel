//! \[EXTENSION\] The statements of extensions from other parties (RULES.md
//! D168):
//!
//! ```text
//! CREATE EXTENSION <identifier> [ VERSION <character string literal> ]
//!     [ FROM <character string literal> ] [ <extension as read> ]
//! ALTER EXTENSION <identifier> UPDATE [ TO <character string literal> ]
//!     [ FROM <character string literal> ] [ <extension as read> ]
//! DROP EXTENSION <identifier> [ RESTRICT | CASCADE ]
//! <extension as read> ::= MANIFEST <binary string literal>
//!     DIRECTORY <character string literal> [ SIGNER <character string literal> ]
//!     [ DIGEST <character string literal> ] [ REGISTRY <character string literal> ]
//!     [ AS DEPENDENCY ]
//! ```
//!
//! `<extension as read>` is what the database writes into the text it
//! logs, its manifest as the hexadecimal of its UTF-8 so its lines are
//! kept; a session that writes it is refused (`db/extensions/session.rs`).

use super::Parser;
use crate::ast::{
    DropBehavior, DropObject, ExtensionDefinition, ExtensionRead, ExtensionStatement, Literal, SchemaStatement, SqlStatement,
};
use crate::error::Result;
use crate::lexer::{Tok, kw};

/// The word after CREATE, ALTER or DROP.
const EXTENSION: &str = "EXTENSION";
/// The words of the statements, in the order of the grammar.
const WORDS: [&str; 6] = ["VERSION", "FROM", "UPDATE", "TO", "MANIFEST", "DIRECTORY"];
/// The words of what the database read.
const READ: [&str; 5] = ["SIGNER", "DIGEST", "REGISTRY", "AS", "DEPENDENCY"];

impl Parser {
    /// Whether `word`, a key word or not, is the token `n` ahead.
    fn extension_word_at(&self, n: usize, word: &str) -> bool {
        match self.peek_at(n) {
            Tok::Identifier(s) => s == word,
            Tok::KeyWord(k) => *k == word,
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

    /// `word`, a key word or not, if it is next: whether it was.
    fn accept_extension_word(&mut self, word: &str) -> bool {
        let next = self.extension_word_at(0, word);
        if next {
            self.advance();
        }
        next
    }

    /// `word`, which must be next.
    fn expect_extension_word(&mut self, word: &str) -> Result<()> {
        if self.accept_extension_word(word) { Ok(()) } else { Err(self.syntax(format!("expected {word}"))) }
    }

    /// Whether CREATE, ALTER or DROP EXTENSION begins here.
    pub(super) fn at_extension_statement(&self) -> bool {
        let verb = self.is_kw(kw::CREATE) || self.is_kw(kw::ALTER) || self.is_kw(kw::DROP);
        verb && self.extension_word_at(1, EXTENSION)
    }

    /// CREATE, ALTER or DROP EXTENSION (`at_extension_statement`).
    ///
    /// # Errors
    ///
    /// SQLCODE -102 where the text breaks the grammar.
    pub(super) fn extension_statement(&mut self) -> Result<SqlStatement> {
        let start = self.here();
        let (create, alter) = (self.is_kw(kw::CREATE), self.is_kw(kw::ALTER));
        self.advance();
        self.advance();
        let at = self.here();
        let name = self.identifier()?;
        if !create && !alter {
            let behavior = if self.is_kw(kw::CASCADE) || self.is_kw(kw::RESTRICT) { self.drop_behavior()? } else { DropBehavior::Restrict };
            return Ok(SqlStatement::Drop { object: DropObject::Extension(name, at), behavior });
        }
        let [version_word, from_word, update, to, ..] = WORDS;
        if alter {
            self.expect_extension_word(update)?;
        }
        let wanted = if alter { to } else { version_word };
        let version = if self.accept_extension_word(wanted) { Some(self.character_string_literal("<version>")?) } else { None };
        let from = self.optional_literal(from_word)?;
        let read = self.extension_read()?;
        let d = ExtensionDefinition { name, at, version, from, read };
        let statement = if alter { ExtensionStatement::Update(d) } else { ExtensionStatement::Create(d) };
        Ok(SqlStatement::Schema { statement: SchemaStatement::Extension(Box::new(statement)), text: self.text_from(start) })
    }

    /// `word` and a `<character string literal>`, if `word` is next.
    fn optional_literal(&mut self, word: &'static str) -> Result<Option<String>> {
        if self.accept_extension_word(word) { self.character_string_literal(word).map(Some) } else { Ok(None) }
    }

    /// `<extension as read>`, if written.
    fn extension_read(&mut self) -> Result<Option<ExtensionRead>> {
        let [.., manifest_word, directory_word] = WORDS;
        if !self.accept_extension_word(manifest_word) {
            return Ok(None);
        }
        let manifest = self.manifest_text()?;
        self.expect_extension_word(directory_word)?;
        let directory = self.character_string_literal("<directory>")?;
        let [signer_word, digest_word, registry_word, as_word, dependency_word] = READ;
        let signer = self.optional_literal(signer_word)?;
        let digest = self.optional_literal(digest_word)?;
        let registry = self.optional_literal(registry_word)?;
        let dependency = self.accept_extension_word(as_word);
        if dependency {
            self.expect_extension_word(dependency_word)?;
        }
        Ok(Some(ExtensionRead { manifest, directory, signer, digest, registry, dependency }))
    }

    /// The manifest of `<extension as read>`: a `<binary string literal>`
    /// of its UTF-8, as the database writes it, so its lines are kept, or
    /// a `<character string literal>`.
    fn manifest_text(&mut self) -> Result<String> {
        if !matches!(self.peek(), Tok::BinaryString(_)) {
            return self.character_string_literal("<manifest>");
        }
        let Literal::Binary(b) = self.literal()? else { return Err(self.syntax("expected the manifest")) };
        String::from_utf8(b.bytes().to_vec()).map_err(|_| self.syntax("the manifest is not UTF-8"))
    }
}
