//! The `<XML lexically scoped options>` [ISO/IEC 9075-14:2006 draft
//! §11.3]: XMLNAMESPACES and XMLBINARY, of a `<with clause>` [§7.2], of
//! WITH after the first word of DELETE, INSERT, MERGE and UPDATE [§14.3
//! to §14.7], after the parenthesis of a generation expression, a CHECK
//! and an assertion [§12.1, §12.2, §12.4], and of DECLARE in a compound
//! statement [§15.1].
//!
//! XMLNAMESPACES and XMLBINARY are reserved words of SQL/XML [§5.1], no
//! key words of SQL-92, so each is read as one where an option may begin
//! and what follows it is what only an option has: of XMLNAMESPACES, (
//! then a `<character string literal>`, DEFAULT or NO; of XMLBINARY,
//! USING, BASE64 or HEX. So a `<query name>` XMLBINARY stays one.

use super::super::Parser;
use super::publish::XMLNAMESPACES;
use crate::ast::LexicallyScopedOptions;
use crate::error::{At, Code, Result, SqlError};
use crate::functions::BinaryEncoding;
use crate::lexer::{Tok, delim, kw};

/// [§11.3] `<XML binary encoding>`'s word.
const XMLBINARY: &str = "XMLBINARY";
/// BASE64.
const BASE64: &str = "BASE64";
/// HEX.
const HEX: &str = "HEX";

impl Parser {
    /// Whether an `<XML lexically scoped option>` begins here.
    pub(in crate::parser) fn at_scoped_option(&self) -> bool {
        self.scoped_option_at(0)
    }

    /// Whether an `<XML lexically scoped option>` begins `n` tokens on.
    fn scoped_option_at(&self, n: usize) -> bool {
        let Tok::Identifier(word) = self.peek_at(n) else { return false };
        let after = self.peek_at(n.saturating_add(1));
        if word == XMLBINARY {
            let encoding = if let Tok::Identifier(w) = after { w == BASE64 || w == HEX } else { false };
            return encoding || matches!(after, Tok::KeyWord(kw::USING));
        }
        let item = matches!(
            self.peek_at(n.saturating_add(2)),
            Tok::CharacterString(_) | Tok::Introducer(_) | Tok::KeyWord(kw::DEFAULT) | Tok::KeyWord(kw::NO)
        );
        word == XMLNAMESPACES && matches!(after, Tok::Delimiter(delim::LEFT_PAREN)) && item
    }

    /// [§11.3] `<XML lexically scoped options>` ::= `<XML lexically scoped
    /// option>` \[ , `<XML lexically scoped option>` \], each an `<XML
    /// namespace declaration>` or an `<XML binary encoding>`.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if none is here; -103 if two of one kind are \[SR1\],
    /// or the declaration breaks SR2 to SR7.
    pub(in crate::parser) fn xml_lexically_scoped_options(&mut self) -> Result<LexicallyScopedOptions> {
        let mut options = LexicallyScopedOptions::default();
        loop {
            let at = self.here();
            if !self.at_scoped_option() {
                return Err(self.syntax("expected XMLNAMESPACES or XMLBINARY [ISO/IEC 9075-14:2006 draft §11.3]"));
            }
            let twice = if self.accept_word(XMLBINARY) {
                options.binary.replace(self.xml_binary_encoding()?).is_some()
            } else {
                let items = self.xml_namespace_declaration()?;
                crate::binder::checked_namespaces(&items).or_at(at)?;
                options.namespaces.replace(items).is_some()
            };
            if twice {
                let msg = "[ISO/IEC 9075-14:2006 draft §11.3 SR1] the options declare namespaces once and a binary encoding once";
                return Err(SqlError::at(Code::SyntaxRule, at, msg));
            }
            // [§7.2 SR4] a comma before a <with list> is the <with clause>'s.
            if !(self.is_delim(delim::COMMA) && self.scoped_option_at(1)) {
                return Ok(options);
            }
            self.advance();
        }
    }

    /// [§11.3] `<XML binary encoding>` ::= XMLBINARY \[ USING \] { BASE64
    /// | HEX }, after XMLBINARY.
    fn xml_binary_encoding(&mut self) -> Result<BinaryEncoding> {
        self.accept_kw(kw::USING);
        if self.accept_word(BASE64) {
            return Ok(BinaryEncoding::Base64);
        }
        self.expect_word(HEX)?;
        Ok(BinaryEncoding::Hex)
    }

    /// WITH `<XML lexically scoped options>`, if WITH is next: of a DELETE,
    /// INSERT, MERGE or UPDATE after its first word [§14.3 to §14.7], or
    /// of a generation expression, a CHECK or an assertion after its
    /// parenthesis [§12.1, §12.2, §12.4].
    ///
    /// # Errors
    ///
    /// As `xml_lexically_scoped_options`.
    pub(in crate::parser) fn with_scoped_options(&mut self) -> Result<Option<LexicallyScopedOptions>> {
        if !self.accept_kw(kw::WITH) {
            return Ok(None);
        }
        self.xml_lexically_scoped_options().map(Some)
    }

    /// [§7.2] What a `<with clause>` holds after WITH: its `<XML lexically
    /// scoped options>`, if written, and whether a `<with list>` follows,
    /// the comma between read.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it holds neither \[SR1\], or it has a comma unless
    /// it holds both, or none if it does \[SR4\]; as
    /// `xml_lexically_scoped_options`.
    pub(in crate::parser) fn with_clause_options(&mut self) -> Result<(Option<LexicallyScopedOptions>, bool)> {
        let options = if self.at_scoped_option() { Some(self.xml_lexically_scoped_options()?) } else { None };
        let comma = self.accept_delim(delim::COMMA);
        // A <with list> begins with a <query name>, or RECURSIVE; a <query
        // expression body> with a key word or a parenthesis.
        let listed = matches!(self.peek(), Tok::Identifier(_));
        if options.is_none() && !listed {
            return Err(self.rule("[ISO/IEC 9075-14:2006 draft §7.2 SR1] a <with clause> holds XML options, a <with list> or both"));
        }
        if comma != (options.is_some() && listed) {
            return Err(self.rule(
                "[ISO/IEC 9075-14:2006 draft §7.2 SR4] a <with clause> has a comma between its XML options and its <with list>, and none else",
            ));
        }
        Ok((options, listed))
    }

    /// [§15.1] DECLARE `<XML lexically scoped options>` ;, if next in a
    /// compound statement, after BEGIN \[\[NOT\] ATOMIC\].
    ///
    /// # Errors
    ///
    /// As `xml_lexically_scoped_options`; SQLCODE -104 if no semicolon
    /// follows.
    pub(in crate::parser) fn declared_scoped_options(&mut self) -> Result<Option<LexicallyScopedOptions>> {
        if !(self.is_kw(kw::DECLARE) && self.scoped_option_at(1)) {
            return Ok(None);
        }
        self.advance();
        let options = self.xml_lexically_scoped_options()?;
        self.expect_delim(delim::SEMICOLON)?;
        Ok(Some(options))
    }
}
