//! Character and entity references [XML §4.1 \[66\] to \[68\]], and what an
//! entity reference expands to: a predefined entity's character [XML
//! §4.6], or an internal entity's replacement text [XML §4.4.2], each
//! checked by the constraints on where entities may be referenced.

use std::rc::Rc;

use super::Parser;
use super::dtd::Value;
use crate::error::Result;
use crate::xml::chars::is_char;

/// [XML §4.1 \[66\]] The base of a hexadecimal character reference.
const HEX: u32 = 16;
/// The base of a decimal one.
const DECIMAL: u32 = 10;

/// What an entity reference expands to.
#[derive(Debug)]
pub(super) enum Expansion {
    /// A predefined entity's character, data and never markup [XML §4.6].
    Char(char),
    /// An internal entity's replacement text, read as the reference's
    /// context reads it.
    Text(Rc<[char]>),
}

/// The character the predefined entity `name` stands for, if it is one
/// [XML §4.6].
pub(crate) fn predefined(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => None,
    }
}

impl Parser {
    /// The character a character reference stands for, read after its
    /// `&#` to its `;` [XML §4.1 \[66\]].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if it is not one, or stands for no
    /// character of the text's version [XML §4.1 WFC: Legal Character].
    pub(super) fn char_reference(&mut self) -> Result<char> {
        let base = if self.eat("x") { HEX } else { DECIMAL };
        let mut code: Option<u32> = Some(0);
        let mut digits = 0_usize;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(base)) {
            self.bump();
            digits = digits.saturating_add(1);
            code = code.and_then(|c| c.checked_mul(base)).and_then(|c| c.checked_add(digit));
        }
        if digits == 0 || !self.eat(";") {
            return Err(self.malformed("a character reference is '&#' digits ';' or '&#x' hexadecimal digits ';' [XML §4.1 [66]]"));
        }
        let version = self.version;
        code.and_then(char::from_u32).filter(|&c| is_char(version, c)).ok_or_else(|| {
            let shown = code.map_or_else(|| "too large".to_owned(), |c| format!("#x{c:X}"));
            self.malformed(&format!(
                "the character reference to {shown} is to no character of XML {} [XML §4.1 WFC: Legal Character]",
                version.number()
            ))
        })
    }

    /// The name of an entity reference, read after its `&` or `%` to its
    /// `;` [XML §4.1 \[68\], \[69\]], which has no colon [Namespaces 1.0 §6]
    /// [Namespaces 1.1 §7].
    pub(super) fn reference_name(&mut self) -> Result<String> {
        let name = self.name("an entity reference [XML §4.1 [68]]")?;
        self.expect(";", "an entity reference [XML §4.1 [68]]")?;
        self.colon_free(&name)?;
        Ok(name)
    }

    /// What a reference to the general entity `name` expands to, in an
    /// attribute value if `in_attribute`, else in content.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if the reference breaks a constraint [XML
    /// §4.1 WFC: Entity Declared, WFC: Parsed Entity] [XML §3.1 WFC: No
    /// External Entity References, WFC: No < in Attribute Values], or
    /// names an entity, or needs a declaration, turborel does not read,
    /// which the data model cannot leave unexpanded [XQueryDM §3.2].
    pub(super) fn general_entity(&self, name: &str, in_attribute: bool) -> Result<Expansion> {
        if let Some(c) = predefined(name) {
            return Ok(Expansion::Char(c));
        }
        let declared = self.dtd.must_declare();
        let Some(entity) = self.dtd.general(name) else {
            return Err(if declared {
                self.malformed(&format!("the entity '{name}' is not declared [XML §4.1 WFC: Entity Declared]"))
            } else {
                self.malformed(&format!("the entity '{name}' is not declared in what turborel reads, and the data model has every entity expanded [XML §5.1] [XQueryDM §3.2]"))
            });
        };
        if declared && entity.in_parameter {
            return Err(self.malformed(&format!(
                "the entity '{name}' of a standalone document is declared in a parameter entity [XML §4.1 WFC: Entity Declared]"
            )));
        }
        match &entity.value {
            Value::Internal(text) => {
                if in_attribute && text.contains(&'<') {
                    Err(self.malformed(&format!(
                        "the replacement text of '{name}', in an attribute value, has a '<' [XML §3.1 WFC: No < in Attribute Values]"
                    )))
                } else {
                    Ok(Expansion::Text(Rc::clone(text)))
                }
            }
            Value::External => {
                Err(if in_attribute {
                    self.malformed(&format!(
                        "an attribute value references the external entity '{name}' [XML §3.1 WFC: No External Entity References]"
                    ))
                } else {
                    self.malformed(&format!("the external entity '{name}' is not read [XML §4.4.3], and the data model has every entity expanded [XQueryDM §3.2]"))
                })
            }
            Value::Unparsed => Err(self.malformed(&format!("'{name}' is an unparsed entity [XML §4.1 WFC: Parsed Entity]"))),
        }
    }
}
