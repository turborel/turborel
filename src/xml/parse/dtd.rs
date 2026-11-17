//! The document type declaration [XML §2.8 \[28\] to \[29\]] and the
//! declarations of its internal subset: entities [XML §4.2], attribute
//! lists (`attlist.rs`), element types [XML §3.2] and notations [XML
//! §4.7], with the parameter entities it references between them and the
//! conditional sections their replacement texts hold [XML §3.4].
//!
//! [ISO/IEC 9075-14:2006 draft §10.16 GR7c, GR7d] The internal subset's
//! entities are expanded and its attribute defaults applied. \[GR7e\]
//! implementation-defined: the external subset is not read. A processor
//! that does not read it, or a parameter entity referenced, processes no
//! entity or attribute-list declaration after the first reference it does
//! not read, unless the document is standalone [XML §5.1]. Element type
//! declarations are checked and not kept: a processor that does not
//! validate needs none, and the data model takes no element content
//! white space from them, which [XQueryDM §6.7.3] leaves optional.

use std::collections::HashMap;
use std::rc::Rc;

use super::Parser;
use super::attlist::Declared;
use super::refs::predefined;
use crate::error::Result;
use crate::xml::chars::{is_pubid_char, is_space};
use crate::xml::{MAX_NESTING, beyond};

/// [XML §3.4 \[62\]] The keyword of an included section.
const INCLUDE: &str = "INCLUDE";
/// [XML §3.4 \[63\]] The keyword of an ignored section.
const IGNORE: &str = "IGNORE";

/// What an entity's declaration gives it.
#[derive(Debug, Clone)]
pub(super) enum Value {
    /// An internal entity's replacement text [XML §4.5].
    Internal(Rc<[char]>),
    /// An external parsed entity, never read.
    External,
    /// An unparsed entity [XML §4.2.2 \[76\]].
    Unparsed,
}

/// A declared entity.
#[derive(Debug)]
pub(super) struct Entity {
    pub(super) value: Value,
    /// Whether it is declared in a parameter entity's replacement text.
    pub(super) in_parameter: bool,
}

/// What the document type declaration has declared, and what of it was
/// read.
#[derive(Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "each is a fact XML §4.1 and §5.1 name, not a state of one machine")]
pub(super) struct Dtd {
    general: HashMap<String, Entity>,
    parameter: HashMap<String, Entity>,
    attributes: HashMap<String, Vec<Declared>>,
    /// Whether it names an external subset.
    external_subset: bool,
    /// Whether it references a parameter entity.
    references: bool,
    /// Whether it references a parameter entity not read.
    unread: bool,
    /// Whether the document says it is standalone [XML §2.9].
    pub(super) standalone: bool,
    /// How many texts were being read where each conditional section
    /// still open began.
    includes: Vec<usize>,
    /// Whether the internal subset is being read.
    pub(super) in_subset: bool,
}

impl Dtd {
    /// Whether entity and attribute-list declarations read now are
    /// processed [XML §5.1].
    pub(super) fn processes(&self) -> bool {
        !self.unread || self.standalone
    }

    /// Whether every entity referenced is declared by what is read, as
    /// WFC: Entity Declared requires of a document of no external subset
    /// and no parameter entity reference, or a standalone one [XML §4.1].
    pub(super) fn must_declare(&self) -> bool {
        (!self.external_subset && !self.references) || self.standalone
    }

    /// The general entity `name`, if declared.
    pub(super) fn general(&self, name: &str) -> Option<&Entity> {
        self.general.get(name)
    }

    /// The declared attributes of the element type `element`.
    pub(super) fn attributes(&self, element: &str) -> &[Declared] {
        self.attributes.get(element).map(Vec::as_slice).unwrap_or_default()
    }

    /// `declared` declared for `element`, unless it is already: the first
    /// declaration binds [XML §3.3].
    pub(super) fn declare_attribute(&mut self, element: &str, declared: Declared) {
        let list = self.attributes.entry(element.to_owned()).or_default();
        if !list.iter().any(|d| d.name == declared.name) {
            list.push(declared);
        }
    }
}

impl Parser {
    /// A document type declaration read after its `<!DOCTYPE` [XML §2.8
    /// \[28\]].
    pub(super) fn doctype(&mut self) -> Result<()> {
        self.require_space("[XML §2.8 [28]]")?;
        let name = self.name("[XML §2.8 [28]]")?;
        self.qname(&name)?;
        let spaced = self.space();
        if spaced && (self.looking_at("SYSTEM") || self.looking_at("PUBLIC")) {
            self.external_id(false)?;
            self.dtd.external_subset = true;
            self.space();
        }
        if self.eat("[") {
            self.dtd.in_subset = true;
            self.internal_subset()?;
            self.dtd.in_subset = false;
            self.space();
        }
        self.expect(">", "[XML §2.8 [28]]")
    }

    /// An `ExternalID` [XML §4.2.2 \[75\]], or, of a notation's
    /// declaration, if `notation`, a `PublicID` too [XML §4.7 \[83\]].
    fn external_id(&mut self, notation: bool) -> Result<()> {
        if self.eat("SYSTEM") {
            self.require_space("[XML §4.2.2 [75]]")?;
            return self.literal(false);
        }
        self.expect("PUBLIC", "[XML §4.2.2 [75]]")?;
        self.require_space("[XML §4.2.2 [75]]")?;
        self.literal(true)?;
        let spaced = self.space();
        let quoted = matches!(self.peek(), Some('"' | '\''));
        if notation && !quoted {
            return Ok(());
        }
        if !spaced {
            return Err(self.malformed("[XML §4.2.2 [75]] requires white space before the system literal"));
        }
        self.literal(false)
    }

    /// A `PubidLiteral` if `public`, else a `SystemLiteral` [XML §2.3
    /// \[11\], \[12\]].
    fn literal(&mut self, public: bool) -> Result<()> {
        let Some(quote) = self.bump().filter(|&q| q == '"' || q == '\'') else {
            return Err(self.malformed("[XML §2.3 [11], [12]] requires a quoted literal here"));
        };
        loop {
            match self.bump() {
                None => return Err(self.malformed("a literal has no closing quote [XML §2.3 [11], [12]]")),
                Some(c) => {
                    if c == quote {
                        return Ok(());
                    }
                    if public && !is_pubid_char(c) {
                        return Err(self.malformed(&format!("a public identifier has the character #x{:X} [XML §2.3 [13]]", u32::from(c))));
                    }
                }
            }
        }
    }

    /// The internal subset read after its `[` to its `]` [XML §2.8
    /// \[28b\]], with the replacement texts of the parameter entities it
    /// references between declarations [XML §2.8 \[28a\]].
    fn internal_subset(&mut self) -> Result<()> {
        let base = self.depth_of_frames();
        loop {
            self.space();
            if self.at_end() {
                if self.depth_of_frames() > base {
                    self.leave_parameter()?;
                    continue;
                }
                return Err(self.malformed("the internal subset has no ']' to end it [XML §2.8 [28]]"));
            }
            let nested = self.depth_of_frames() > base;
            if !nested && self.eat("]") {
                return Ok(());
            }
            if self.eat("]]>") {
                self.end_include()?;
            } else if self.eat("%") {
                self.declaration_separator()?;
            } else if nested && self.eat("<![") {
                self.conditional_section()?;
            } else {
                self.markup_decl()?;
            }
        }
    }

    /// A parameter entity's replacement text left at its end, every
    /// conditional section it began ended [XML §2.8 WFC: PE Between
    /// Declarations].
    fn leave_parameter(&mut self) -> Result<()> {
        if self.dtd.includes.last() == Some(&self.depth_of_frames()) {
            return Err(self.malformed(
                "a conditional section does not end in the parameter entity it begins in [XML §2.8 WFC: PE Between Declarations]",
            ));
        }
        self.leave();
        Ok(())
    }

    /// The `]]>` of the included section begun last in the text being
    /// read [XML §3.4 \[62\]].
    fn end_include(&mut self) -> Result<()> {
        if self.dtd.includes.last() == Some(&self.depth_of_frames()) {
            self.dtd.includes.pop();
            Ok(())
        } else {
            Err(self.malformed("']]>' ends no conditional section [XML §3.4 [62]]"))
        }
    }

    /// A `markupdecl` [XML §2.8 \[29\]].
    fn markup_decl(&mut self) -> Result<()> {
        if self.eat("<!ENTITY") {
            self.entity_decl()
        } else if self.eat("<!ATTLIST") {
            self.attlist_decl()
        } else if self.eat("<!ELEMENT") {
            self.element_decl()
        } else if self.eat("<!NOTATION") {
            self.notation_decl()
        } else if self.eat("<!--") {
            self.comment_text().map(drop)
        } else if self.eat("<?") {
            self.pi_parts().map(drop)
        } else {
            Err(self.malformed("[XML §2.8 [28b]] requires a markup declaration, a parameter entity reference or white space here"))
        }
    }

    /// A parameter entity reference between declarations read after its
    /// `%` [XML §2.8 \[28a\]]: an internal entity's replacement text read
    /// next; any other not read, so declarations after it are not
    /// processed [XML §5.1].
    fn declaration_separator(&mut self) -> Result<()> {
        let name = self.reference_name()?;
        self.dtd.references = true;
        match self.dtd.parameter.get(&name).map(|e| e.value.clone()) {
            Some(Value::Internal(text)) => self.enter(&name, text, true),
            Some(Value::External | Value::Unparsed) | None => {
                self.dtd.unread = true;
                Ok(())
            }
        }
    }

    /// A conditional section read after its `<![` [XML §3.4 \[61\]]: its
    /// keyword written, or the replacement text of a parameter entity.
    fn conditional_section(&mut self) -> Result<()> {
        self.space();
        let keyword: String = if self.eat("%") {
            let name = self.reference_name()?;
            self.dtd.references = true;
            match self.dtd.parameter.get(&name).map(|e| e.value.clone()) {
                Some(Value::Internal(text)) => text.iter().collect::<String>().trim_matches(is_space).to_owned(),
                Some(Value::External | Value::Unparsed) | None => {
                    return Err(self.malformed(&format!(
                        "the keyword of a conditional section is the entity '{name}', which turborel does not read [XML §5.1]"
                    )));
                }
            }
        } else {
            self.name("[XML §3.4 [61]]")?
        };
        self.space();
        self.expect("[", "[XML §3.4 [62], [63]]")?;
        match keyword.as_str() {
            INCLUDE => {
                self.dtd.includes.push(self.depth_of_frames());
                Ok(())
            }
            IGNORE => self.ignore_section(),
            _ => Err(self.malformed(&format!("a conditional section is INCLUDE or IGNORE, not '{keyword}' [XML §3.4 [61]]"))),
        }
    }

    /// An ignored section's contents read to its `]]>`, nested sections
    /// with them [XML §3.4 \[63\] to \[65\]].
    fn ignore_section(&mut self) -> Result<()> {
        let mut depth = 1_usize;
        while depth > 0 {
            if self.eat("<![") {
                depth = depth.saturating_add(1);
            } else if self.eat("]]>") {
                depth = depth.saturating_sub(1);
            } else if self.bump().is_none() {
                return Err(self.malformed("an ignored section has no ']]>' in the entity it begins in [XML §3.4 [63]]"));
            }
        }
        Ok(())
    }

    /// An entity declaration read after its `<!ENTITY` [XML §4.2 \[70\] to
    /// \[74\]], declared if declarations are processed and it is the
    /// first of its name [XML §4.2].
    fn entity_decl(&mut self) -> Result<()> {
        self.require_space("[XML §4.2 [71]]")?;
        let parameter = self.eat("%");
        if parameter {
            self.require_space("[XML §4.2 [72]]")?;
        }
        let name = self.name("[XML §4.2 [71]]")?;
        self.colon_free(&name)?;
        self.require_space("[XML §4.2 [71]]")?;
        let value = self.entity_definition(parameter)?;
        self.space();
        self.expect(">", "[XML §4.2 [71]]")?;
        if !self.dtd.processes() {
            return Ok(());
        }
        if !parameter && predefined(&name).is_some() {
            return self.predefined_declaration(&name, &value);
        }
        let entity = Entity { value, in_parameter: self.in_parameter_entity() };
        let table = if parameter { &mut self.dtd.parameter } else { &mut self.dtd.general };
        table.entry(name).or_insert(entity);
        Ok(())
    }

    /// An `EntityDef` or `PEDef` [XML §4.2 \[73\], \[74\]].
    fn entity_definition(&mut self, parameter: bool) -> Result<Value> {
        if matches!(self.peek(), Some('"' | '\'')) {
            return Ok(Value::Internal(self.entity_value()?.into()));
        }
        self.external_id(false)?;
        let spaced = self.space();
        if !self.eat("NDATA") {
            return Ok(Value::External);
        }
        if parameter || !spaced {
            return Err(self.malformed("[XML §4.2 [73], [74], §4.2.2 [76]] an NDATA declaration follows white space, of a general entity"));
        }
        self.require_space("[XML §4.2.2 [76]]")?;
        let notation = self.name("[XML §4.2.2 [76]]")?;
        self.colon_free(&notation)?;
        Ok(Value::Unparsed)
    }

    /// An `EntityValue` read from its opening quote [XML §2.3 \[9\]], as
    /// its replacement text: each character reference its character, and
    /// each entity reference kept as written [XML §4.5].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if it references a parameter entity, which
    /// the internal subset never does within a declaration [XML §2.8 WFC:
    /// PEs in Internal Subset].
    fn entity_value(&mut self) -> Result<Vec<char>> {
        let Some(quote) = self.bump() else {
            return Err(self.malformed("[XML §2.3 [9]] requires a quoted entity value here"));
        };
        let mut text = Vec::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.malformed("an entity value has no closing quote in the entity it begins in [XML §2.3 [9]]"));
            };
            if c == quote {
                return Ok(text);
            }
            match c {
                '%' => return Err(self.malformed("an entity value references a parameter entity [XML §2.8 WFC: PEs in Internal Subset]")),
                '&' => {
                    if self.eat("#") {
                        text.push(self.char_reference()?);
                    } else {
                        let name = self.reference_name()?;
                        text.push('&');
                        text.extend(name.chars());
                        text.push(';');
                    }
                }
                other => text.push(other),
            }
        }
    }

    /// The error unless the predefined entity `name` is declared as XML
    /// requires: `lt` and `amp` as a character reference to their
    /// character, the others as theirs or a reference to it [XML §4.6].
    /// Its meaning is the predefined one either way.
    fn predefined_declaration(&self, name: &str, value: &Value) -> Result<()> {
        let character = predefined(name).map(u32::from);
        let text: String = match value {
            Value::Internal(text) => text.iter().collect(),
            Value::External | Value::Unparsed => String::new(),
        };
        let escaped = referenced(&text) == character;
        let plain = text.chars().map(u32::from).eq(character) && name != "lt" && name != "amp";
        if escaped || plain {
            Ok(())
        } else {
            Err(self.malformed(&format!("the predefined entity '{name}' is declared otherwise than as its character [XML §4.6]")))
        }
    }

    /// An element type declaration read after its `<!ELEMENT` [XML §3.2
    /// \[45\]].
    fn element_decl(&mut self) -> Result<()> {
        self.require_space("[XML §3.2 [45]]")?;
        let name = self.name("[XML §3.2 [45]]")?;
        self.qname(&name)?;
        self.require_space("[XML §3.2 [45]]")?;
        if !(self.eat("EMPTY") || self.eat("ANY")) {
            self.expect("(", "[XML §3.2 [46]]")?;
            self.space();
            if self.eat("#PCDATA") { self.mixed()? } else { self.children()? }
        }
        self.space();
        self.expect(">", "[XML §3.2 [45]]")
    }

    /// The rest of a `Mixed` content model after its `#PCDATA` [XML
    /// §3.2.2 \[51\]].
    fn mixed(&mut self) -> Result<()> {
        let mut names = false;
        loop {
            self.space();
            if self.eat(")") {
                break;
            }
            self.expect("|", "[XML §3.2.2 [51]]")?;
            self.space();
            let name = self.name("[XML §3.2.2 [51]]")?;
            self.qname(&name)?;
            names = true;
        }
        if names {
            self.expect("*", "[XML §3.2.2 [51]]")
        } else {
            self.eat("*");
            Ok(())
        }
    }

    /// The rest of a `children` content model after its first `(` [XML
    /// §3.2.1 \[47\] to \[50\]]: choices and sequences, each of one
    /// separator, nested at most `MAX_NESTING` deep.
    fn children(&mut self) -> Result<()> {
        let mut groups: Vec<Option<char>> = vec![None];
        loop {
            self.space();
            if self.eat("(") {
                if groups.len() >= MAX_NESTING {
                    return Err(beyond(&format!("a content model nests deeper than {MAX_NESTING}")));
                }
                groups.push(None);
                continue;
            }
            let name = self.name("[XML §3.2.1 [48]]")?;
            self.qname(&name)?;
            self.quantifier();
            if self.after_particle(&mut groups)? {
                return Ok(());
            }
        }
    }

    /// The groups a content particle ends, and the separator after it;
    /// whether it ended the model.
    fn after_particle(&mut self, groups: &mut Vec<Option<char>>) -> Result<bool> {
        loop {
            self.space();
            if self.eat(")") {
                groups.pop();
                self.quantifier();
                if groups.is_empty() {
                    return Ok(true);
                }
                continue;
            }
            let Some(separator) = self.peek().filter(|&c| c == '|' || c == ',') else {
                return Err(self.malformed("[XML §3.2.1 [49], [50]] requires '|', ',' or ')' here"));
            };
            self.bump();
            let group = groups.last_mut().ok_or_else(|| self.malformed("[XML §3.2.1 [47]] a content model is a choice or a sequence"))?;
            if group.is_some_and(|s| s != separator) {
                return Err(self.malformed("a choice or sequence mixes '|' and ',' [XML §3.2.1 [49], [50]]"));
            }
            *group = Some(separator);
            return Ok(false);
        }
    }

    /// A `?`, `*` or `+` read, if one is next [XML §3.2.1 \[47\], \[48\]].
    fn quantifier(&mut self) {
        if matches!(self.peek(), Some('?' | '*' | '+')) {
            self.bump();
        }
    }

    /// A notation declaration read after its `<!NOTATION` [XML §4.7 \[82\]].
    fn notation_decl(&mut self) -> Result<()> {
        self.require_space("[XML §4.7 [82]]")?;
        let name = self.name("[XML §4.7 [82]]")?;
        self.colon_free(&name)?;
        self.require_space("[XML §4.7 [82]]")?;
        self.external_id(true)?;
        self.space();
        self.expect(">", "[XML §4.7 [82]]")
    }
}

/// The code a text that is a character reference alone references, if it
/// is one [XML §4.1 \[66\]].
fn referenced(text: &str) -> Option<u32> {
    let digits = text.strip_prefix("&#")?.strip_suffix(';')?;
    match digits.strip_prefix('x') {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => digits.parse().ok(),
    }
}

#[cfg(test)]
mod tests;
