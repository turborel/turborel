//! Attribute-list declarations [XML §3.3 \[52\], \[53\], §3.3.1, §3.3.2] and the values of
//! attributes [XML §2.3 \[10\], §3.3.3]: a value normalized by its type,
//! which a declaration gives, and an attribute a declaration defaults
//! added to the elements that do not specify it [XQueryDM §6.2.3]
//! [ISO/IEC 9075-14:2006 draft §10.16 GR7d].
//!
//! Only what the data model keeps is kept of a declaration: whether the
//! type is CDATA, ID, IDREF or IDREFS, or another, which normalizes as a
//! list of tokens, and its default. A processor that does not validate
//! checks no validity constraint [XML §5.1].

use super::Parser;
use super::refs::Expansion;
use crate::error::Result;
use crate::xml::chars::is_name_char;

/// The type of an attribute, as far as the data model tells types apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// CDATA, a string, the type of an attribute no declaration names.
    Cdata,
    /// ID [XQueryDM §6.3.3].
    Id,
    /// IDREF or IDREFS [XQueryDM §6.3.3].
    References,
    /// ENTITY, ENTITIES, NMTOKEN, NMTOKENS, NOTATION or an enumeration.
    Tokens,
}

/// [XML §3.3.1 \[55\], \[56\]] The type keywords, a longer before a shorter
/// that begins it.
const TYPES: [(&str, Kind); 8] = [
    ("CDATA", Kind::Cdata),
    ("IDREFS", Kind::References),
    ("IDREF", Kind::References),
    ("ID", Kind::Id),
    ("ENTITIES", Kind::Tokens),
    ("ENTITY", Kind::Tokens),
    ("NMTOKENS", Kind::Tokens),
    ("NMTOKEN", Kind::Tokens),
];
/// [XML §3.3.1 \[58\]] The keyword of a notation type.
const NOTATION: &str = "NOTATION";

/// An attribute's declaration.
#[derive(Debug, Clone)]
pub(super) struct Declared {
    /// Its name as written.
    pub(super) name: String,
    pub(super) kind: Kind,
    /// Its default, normalized, if it has one: none if `#REQUIRED` or
    /// `#IMPLIED` [XML §3.3.2 \[60\]].
    pub(super) default: Option<String>,
}

/// `value` normalized as a value of a type other than CDATA: no leading
/// or trailing space, and each run of spaces one [XML §3.3.3].
pub(super) fn tokenized(value: &str) -> String {
    value.split(' ').filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

impl Parser {
    /// An attribute-list declaration read after its `<!ATTLIST` [XML §3.3
    /// \[52\]], its attributes declared unless the parser no longer
    /// processes declarations [XML §5.1]; the first declaration of an
    /// attribute binds [XML §3.3].
    pub(super) fn attlist_decl(&mut self) -> Result<()> {
        self.require_space("[XML §3.3 [52]]")?;
        let element = self.name("[XML §3.3 [52]]")?;
        self.qname(&element)?;
        let processes = self.dtd.processes();
        loop {
            let spaced = self.space();
            if self.eat(">") {
                return Ok(());
            }
            if !spaced {
                return Err(self.malformed("[XML §3.3 [53]] requires white space before each attribute definition"));
            }
            let name = self.name("[XML §3.3 [53]]")?;
            self.qname(&name)?;
            self.require_space("[XML §3.3 [53]]")?;
            let kind = self.attribute_type()?;
            self.require_space("[XML §3.3 [53]]")?;
            let default = self.default_decl(kind, processes)?;
            if processes {
                self.dtd.declare_attribute(&element, Declared { name, kind, default });
            }
        }
    }

    /// An `AttType` [XML §3.3.1 \[54\]].
    fn attribute_type(&mut self) -> Result<Kind> {
        if self.eat(NOTATION) {
            self.require_space("[XML §3.3.1 [58]]")?;
            self.enumeration(true)?;
            return Ok(Kind::Tokens);
        }
        if self.looking_at("(") {
            self.enumeration(false)?;
            return Ok(Kind::Tokens);
        }
        match TYPES.iter().find(|(word, _)| self.looking_at(word)) {
            Some(&(word, kind)) => {
                self.eat(word);
                Ok(kind)
            }
            None => Err(self.malformed("[XML §3.3.1 [54]] requires an attribute type here")),
        }
    }

    /// A parenthesized list of notation names, if `names`, else of name
    /// tokens [XML §3.3.1 \[58\], \[59\]].
    fn enumeration(&mut self, names: bool) -> Result<()> {
        let rule = if names { "[XML §3.3.1 [58]]" } else { "[XML §3.3.1 [59]]" };
        self.expect("(", rule)?;
        loop {
            self.space();
            if names {
                let name = self.name(rule)?;
                self.colon_free(&name)?;
            } else {
                self.nmtoken(rule)?;
            }
            self.space();
            if self.eat(")") {
                return Ok(());
            }
            self.expect("|", rule)?;
        }
    }

    /// A `Nmtoken` read [XML §2.3 \[7\]].
    fn nmtoken(&mut self, rule: &str) -> Result<()> {
        let version = self.version;
        let mut any = false;
        while self.peek().is_some_and(|c| is_name_char(version, c)) {
            self.bump();
            any = true;
        }
        if any { Ok(()) } else { Err(self.malformed(&format!("{rule} requires a name token here [XML §2.3 [7]]"))) }
    }

    /// A `DefaultDecl` [XML §3.3.2 \[60\]]: the default, normalized for
    /// `kind`, its references expanded if `expand`.
    fn default_decl(&mut self, kind: Kind, expand: bool) -> Result<Option<String>> {
        if self.eat("#REQUIRED") || self.eat("#IMPLIED") {
            return Ok(None);
        }
        if self.eat("#FIXED") {
            self.require_space("[XML §3.3.2 [60]]")?;
        }
        let value = self.attribute_value(expand)?;
        Ok(Some(if kind == Kind::Cdata { value } else { tokenized(&value) }))
    }

    /// An `AttValue` read from its opening quote, normalized as CDATA
    /// [XML §2.3 \[10\], §3.3.3]: each white space character a space, each
    /// character reference its character, each entity reference its
    /// replacement text, normalized alike, if `expand`; else checked and
    /// dropped, as a declaration not processed has none [XML §5.1].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if it is not one, holds a '<' [XML §3.1
    /// WFC: No < in Attribute Values], or a reference breaks a
    /// constraint (`general_entity`).
    pub(super) fn attribute_value(&mut self, expand: bool) -> Result<String> {
        let Some(quote) = self.bump().filter(|&q| q == '"' || q == '\'') else {
            return Err(self.malformed("[XML §2.3 [10]] requires a quoted attribute value here"));
        };
        let base = self.depth_of_frames();
        let mut value = String::new();
        loop {
            let Some(c) = self.bump() else {
                if self.depth_of_frames() > base && self.leave() {
                    continue;
                }
                return Err(self.malformed("an attribute value has no closing quote [XML §2.3 [10]]"));
            };
            let literal = self.depth_of_frames() == base;
            if c == quote && literal {
                return Ok(value);
            }
            match c {
                '<' => return Err(self.malformed("an attribute value has a '<' [XML §3.1 WFC: No < in Attribute Values]")),
                '&' => self.attribute_reference(&mut value, expand)?,
                ' ' | '\t' | '\n' | '\r' => value.push(' '),
                other => value.push(other),
            }
        }
    }

    /// A reference in an attribute value read after its `&`: its
    /// character added to `value`, or its entity's replacement text read
    /// next, if `expand`.
    fn attribute_reference(&mut self, value: &mut String, expand: bool) -> Result<()> {
        if self.eat("#") {
            value.push(self.char_reference()?);
            return Ok(());
        }
        let name = self.reference_name()?;
        if !expand {
            return Ok(());
        }
        match self.general_entity(&name, true)? {
            Expansion::Char(c) => value.push(c),
            Expansion::Text(text) => self.enter(&name, text, false)?,
        }
        Ok(())
    }
}
