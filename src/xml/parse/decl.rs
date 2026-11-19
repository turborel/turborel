//! The XML declaration of a document [XML §2.8 \[23\] to \[26\], §2.9
//! \[32\]] and the text declaration of an external parsed entity [XML
//! §4.3.1 \[77\], §4.3.3 \[80\], \[81\]]: one grammar read once at the
//! start of the text, then
//! judged as either, since content may be a document or an entity
//! [ISO/IEC 9075-14:2006 draft §10.16 GR4, GR5].
//!
//! The encoding a declaration names is checked as an `EncName` and not
//! otherwise used: a character string is characters already, and a
//! binary string is decoded by its encoding before it is parsed
//! (`encoding.rs`).

use std::rc::Rc;

use super::{Frame, Parser};
use crate::error::Result;
use crate::xml::chars::{VERSION_10, VERSION_11, Version, is_space};

/// The target that begins a declaration.
const OPEN: &str = "<?xml";
/// What ends a declaration.
const CLOSE: &str = "?>";
/// [XML §2.9 \[32\]] The standalone document declaration's values.
const YES: &str = "yes";
/// The other.
const NO: &str = "no";

/// What a text's declaration says, if it has one.
#[derive(Debug, Default)]
pub(super) struct Declaration {
    /// Whether the text has one.
    present: bool,
    /// Its `VersionInfo`, if it has one.
    version: Option<Version>,
    /// Its `EncodingDecl`, if it has one.
    encoding: Option<String>,
    /// Its `SDDecl`, if it has one: whether it says yes.
    standalone: Option<bool>,
}

impl Declaration {
    /// Why it is no XML declaration, if it is not: one has a version
    /// [XML §2.8 \[23\]].
    pub(super) fn as_xml_declaration(&self) -> std::result::Result<(), &'static str> {
        if self.present && self.version.is_none() { Err("an XML declaration has a version [XML §2.8 [23]]") } else { Ok(()) }
    }

    /// Why it is no text declaration, if it is not: one has an encoding
    /// and no standalone document declaration [XML §4.3.1 \[77\]].
    pub(super) fn as_text_declaration(&self) -> std::result::Result<(), &'static str> {
        if !self.present {
            Ok(())
        } else if self.encoding.is_none() {
            Err("a text declaration has an encoding [XML §4.3.1 [77]]")
        } else if self.standalone.is_some() {
            Err("a text declaration has no standalone document declaration [XML §4.3.1 [77]]")
        } else {
            Ok(())
        }
    }

    /// Whether it says the document is standalone [XML §2.9 \[32\]].
    pub(super) fn standalone(&self) -> bool {
        self.standalone == Some(true)
    }
}

impl Parser {
    /// The declaration `chars` begins with, if any, and how many of its
    /// characters it takes; the parser set to its version, 1.0 if it
    /// names none [XML §2.8].
    pub(super) fn declaration(&mut self, chars: &[char]) -> Result<(Declaration, usize)> {
        let opens = chars.iter().take(OPEN.len()).copied().eq(OPEN.chars());
        let begins = opens && chars.get(OPEN.len()).is_some_and(|&c| is_space(c));
        if !begins {
            return Ok((Declaration::default(), 0));
        }
        self.frames.push(Frame { chars: Rc::from(chars), at: 0, entity: None, parameter: false });
        let read = self.declaration_body();
        let after = self.frames.pop().map_or(0, |f| f.at);
        let declaration = read?;
        self.version = declaration.version.unwrap_or(Version::V10);
        Ok((declaration, after))
    }

    /// The declaration read, from its `<?xml` to its `?>`.
    fn declaration_body(&mut self) -> Result<Declaration> {
        self.expect(OPEN, "[XML §2.8 [23]]")?;
        let mut declaration = Declaration { present: true, ..Declaration::default() };
        let mut spaced = self.space();
        if spaced && self.eat("version") {
            declaration.version = Some(self.version_number()?);
            spaced = self.space();
        }
        if spaced && self.eat("encoding") {
            declaration.encoding = Some(self.encoding_name()?);
            spaced = self.space();
        }
        if spaced && self.eat("standalone") {
            let value = self.pseudo_attribute("[XML §2.9 [32]]")?;
            declaration.standalone = match value.as_str() {
                YES => Some(true),
                NO => Some(false),
                _ => {
                    return Err(
                        self.malformed(&format!("the standalone document declaration is 'yes' or 'no', not '{value}' [XML §2.9 [32]]"))
                    );
                }
            };
            self.space();
        }
        self.expect(CLOSE, "[XML §2.8 [23]] [XML §4.3.1 [77]]")?;
        Ok(declaration)
    }

    /// The value of a pseudo-attribute, after its name: `Eq` and a quoted
    /// value [XML §2.8 \[25\]].
    fn pseudo_attribute(&mut self, rule: &str) -> Result<String> {
        self.space();
        self.expect("=", rule)?;
        self.space();
        let Some(quote) = self.bump().filter(|&q| q == '"' || q == '\'') else {
            return Err(self.malformed(&format!("{rule} requires a quoted value here")));
        };
        let mut value = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.malformed(&format!("{rule} requires the value to end with {quote}")));
            };
            if c == quote {
                return Ok(value);
            }
            value.push(c);
        }
    }

    /// The `VersionNum` [XML §2.8 \[24\], \[26\]]: 1.0 or 1.1, the versions
    /// turborel reads.
    fn version_number(&mut self) -> Result<Version> {
        let value = self.pseudo_attribute("[XML §2.8 [24]]")?;
        match value.as_str() {
            VERSION_10 => Ok(Version::V10),
            VERSION_11 => Ok(Version::V11),
            _ => Err(self.malformed(&format!("the version is '1.0' or '1.1', not '{value}' [XML §2.8 [26]]"))),
        }
    }

    /// The `EncName` [XML §4.3.3 \[80\], \[81\]]: a Latin letter, then Latin
    /// letters, digits, '.', '_' and '-'.
    fn encoding_name(&mut self) -> Result<String> {
        let value = self.pseudo_attribute("[XML §4.3.3 [80]]")?;
        let mut chars = value.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if valid { Ok(value) } else { Err(self.malformed(&format!("'{value}' is no encoding name [XML §4.3.3 [81]]"))) }
    }
}
