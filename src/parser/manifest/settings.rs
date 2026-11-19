//! \[EXTENSION\] The grammar of a manifest's settings (RULES.md D280,
//! D281), `plugin/manifest/settings.rs` giving its BNF: `SETTING`, a
//! setting declared, and `READS SETTINGS`, the built-in settings its
//! program reads.

use super::super::Parser;
use crate::ast::Literal;
use crate::error::Result;
use crate::lexer::{Tok, delim};
use crate::plugin::manifest::{Item, Scope, SettingDeclared, SettingType};
use crate::value::Value;

/// The words of a setting's declaration, in the order of its grammar.
const WORDS: [&str; 9] = ["SETTING", "TYPE", "MIN", "MAX", "CHOICES", "DEFAULT", "SCOPE", "DESCRIPTION", "READS"];
/// The word after READS.
const SETTINGS: &str = "SETTINGS";

impl Parser {
    /// `<setting declaration>` or `<reads settings>`, if one is next.
    pub(super) fn setting_item(&mut self) -> Option<Result<Item>> {
        let [setting, .., reads] = WORDS;
        if self.accept_manifest_word(setting) {
            return Some(self.setting_declaration().map(|d| Item::Setting(Box::new(d))));
        }
        let reads_settings = self.peek_word_is(reads) && self.peek_word_at_is(1, SETTINGS);
        if !reads_settings {
            return None;
        }
        Some((|| {
            self.expect_manifest_word(reads)?;
            self.expect_manifest_word(SETTINGS)?;
            self.expect_delim(delim::LEFT_PAREN)?;
            let mut names = vec![self.character_string_literal("<setting name>")?];
            while self.accept_delim(delim::COMMA) {
                names.push(self.character_string_literal("<setting name>")?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            Ok(Item::ReadsSettings(names))
        })())
    }

    /// Whether the token next but `ahead` is the word `word`, a key word
    /// or not.
    fn peek_word_at_is(&self, ahead: usize, word: &str) -> bool {
        let tok = self.peek_at(ahead);
        if let Tok::KeyWord(k) = tok {
            return k.eq_ignore_ascii_case(word);
        }
        if let Tok::Identifier(i) = tok {
            return i.eq_ignore_ascii_case(word);
        }
        false
    }

    /// Whether the next token is the word `word`.
    fn peek_word_is(&self, word: &str) -> bool {
        self.peek_word_at_is(0, word)
    }

    /// `<setting declaration>`, after SETTING.
    fn setting_declaration(&mut self) -> Result<SettingDeclared> {
        let [setting, kind, min, max, choices, default, scope, description, _] = WORDS;
        let at = self.here();
        let name = self.identifier()?;
        self.expect_manifest_word(kind)?;
        let mut d = SettingDeclared::new(name, self.setting_type()?);
        if self.accept_manifest_word(min) {
            d.min = Some(self.setting_literal()?);
        }
        if self.accept_manifest_word(max) {
            d.max = Some(self.setting_literal()?);
        }
        if self.accept_manifest_word(choices) {
            d.choices = self.manifest_words()?.into_iter().map(|w| w.to_ascii_uppercase()).collect();
        }
        if self.accept_manifest_word(default) {
            d.default = Some(self.setting_literal()?);
        }
        if self.accept_manifest_word(scope) {
            d.scope = Some(self.setting_scope()?);
        }
        if self.accept_manifest_word(description) {
            d.description = Some(self.character_string_literal("<description>")?);
        }
        d.text = format!("{setting} {}", self.text_from(at));
        Ok(d)
    }

    /// `<setting type>`.
    fn setting_type(&mut self) -> Result<SettingType> {
        let found = SettingType::ALL.into_iter().find(|(word, _)| self.accept_manifest_word(word));
        found.map(|(_, t)| t).ok_or_else(|| self.syntax("expected INTEGER, NUMBER, CHOICE, SWITCH, TEXT or IDENTIFIER"))
    }

    /// SESSION, DATABASE or NODE.
    fn setting_scope(&mut self) -> Result<Scope> {
        let found = Scope::ALL.into_iter().find(|(word, _)| self.accept_manifest_word(word));
        found.map(|(_, s)| s).ok_or_else(|| self.syntax("expected SESSION, DATABASE or NODE"))
    }

    /// `<setting literal>`: a signed number or a character string.
    fn setting_literal(&mut self) -> Result<Value> {
        match self.literal()? {
            Literal::Character(s) => Ok(Value::Char(s)),
            Literal::Exact(d) => Ok(Value::Exact(d)),
            Literal::Approximate(x) => Ok(Value::Approx(x)),
            Literal::Binary(_) | Literal::Datetime(_) | Literal::Interval(_) | Literal::Boolean(_) => {
                Err(self.syntax("a setting's value is a number or a character string"))
            }
        }
    }
}
