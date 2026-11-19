//! Content read into the tree [XML §3 \[39\], §3.1 \[40\] to \[44\]]: elements, their
//! attributes, character data, references, CDATA sections, comments and
//! processing instructions [XML §2.4 to §2.7], and, before the first
//! element, the document type declaration [XML §2.8].
//!
//! [ISO/IEC 9075-14:2006 draft §10.16 GR7a i] The document node's
//! children are what the content production makes, so the text is read
//! as content throughout, and what it held beside its elements decides
//! whether it is a document (`Parser::verdict`). Characters read since
//! the last markup are one text node [XQueryDM §6.7.3], removed if they
//! are white space alone where GR8 strips it: of the document node,
//! under STRIP WHITESPACE \[GR8c\] and, outside its element, of every
//! document, of which they are no character [XML §2.8 \[27\]]; of a
//! potentially whitespace-strippable element under STRIP WHITESPACE
//! [GR8a, GR8b].

use super::attlist::{Kind, tokenized};
use super::names::RawAttribute;
use super::refs::Expansion;
use super::{OpenElement, Parser, Syntax, Whitespace};
use crate::error::Result;
use crate::xml::chars::is_space;
use crate::xml::tree::{ElementStart, XML_NAMESPACE};

/// [XML §2.10] The local name of `xml:space`.
const SPACE: &str = "space";
/// [XML §2.10] The value of `xml:space` that keeps white space.
const PRESERVE: &str = "preserve";
/// [XML §2.10] The value of `xml:space` that lets it go.
const DEFAULT: &str = "default";
/// [XML §2.6 \[17\]] The target no processing instruction has, in any case.
const RESERVED_TARGET: &str = "xml";

impl Parser {
    /// The text read as content to its end, every entity it references
    /// with it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if it is not well-formed, or an entity
    /// reference cannot be expanded; SQLCODE -900 if it is beyond
    /// turborel's limits.
    pub(super) fn content(&mut self) -> Result<()> {
        loop {
            if self.at_end() {
                if self.depth_of_frames() > 1 {
                    self.leave_entity()?;
                    continue;
                }
                break;
            }
            if self.peek() == Some('<') {
                self.markup()?;
            } else if self.eat("&") {
                self.content_reference()?;
            } else {
                self.char_data()?;
            }
        }
        self.flush()?;
        match self.open.last() {
            Some(open) => Err(self.malformed(&format!("the element <{}> has no end tag [XML §3 [39]]", open.name))),
            None => Ok(()),
        }
    }

    /// An entity's replacement text left at its end, every element begun
    /// in it ended in it [XML §4.3.2].
    fn leave_entity(&mut self) -> Result<()> {
        if let Some(open) = self.open.last().filter(|o| o.frames == self.depth_of_frames()) {
            return Err(self.malformed(&format!("the element <{}> begun in an entity does not end in it [XML §4.3.2]", open.name)));
        }
        self.leave();
        Ok(())
    }

    /// Markup beginning with `<`.
    fn markup(&mut self) -> Result<()> {
        if self.eat("</") {
            self.end_tag()
        } else if self.eat("<!--") {
            self.flush()?;
            let text = self.comment_text()?;
            self.builder.comment(&text)
        } else if self.eat("<![CDATA[") {
            self.top_level_other()?;
            let text = self.until("]]>", "a CDATA section [XML §2.7 [18]]")?;
            self.pending.push_str(&text);
            Ok(())
        } else if self.eat("<!DOCTYPE") {
            self.doctype_here()
        } else if self.eat("<?") {
            self.flush()?;
            let (target, text) = self.pi_parts()?;
            self.builder.processing_instruction(&target, &text)
        } else {
            self.bump();
            self.start_tag()
        }
    }

    /// The document type declaration read after its `<!DOCTYPE`: in the
    /// document entity, once, before any element or text but white space
    /// [XML §2.8 \[22\]].
    fn doctype_here(&mut self) -> Result<()> {
        self.flush()?;
        let first = self.depth_of_frames() == 1 && self.top.elements == 0 && !self.top.other && !self.top.doctype;
        if !first {
            return Err(self.malformed("a document type declaration is in the prolog, before the element [XML §2.8 [22]]"));
        }
        self.top.doctype = true;
        self.dtd.standalone = self.decl.standalone();
        self.doctype()
    }

    /// A comment's text, read after its `<!--` to its `-->`, which holds
    /// no `--` [XML §2.5 \[15\]].
    pub(super) fn comment_text(&mut self) -> Result<String> {
        let text = self.until("--", "a comment [XML §2.5 [15]]")?;
        if self.eat(">") { Ok(text) } else { Err(self.malformed("a comment holds no '--' and ends with no '-' [XML §2.5 [15]]")) }
    }

    /// A processing instruction's target and content, read after its `<?`
    /// to its `?>` [XML §2.6 \[16\], \[17\]]: its content without the white
    /// space after its target.
    pub(super) fn pi_parts(&mut self) -> Result<(String, String)> {
        let target = self.name("[XML §2.6 [16]]")?;
        if target.eq_ignore_ascii_case(RESERVED_TARGET) {
            return Err(
                self.malformed("the target 'xml' is reserved, and an XML declaration begins the text [XML §2.6 [17]] [XML §2.8 [22]]")
            );
        }
        self.colon_free(&target)?;
        if self.eat("?>") {
            return Ok((target, String::new()));
        }
        self.require_space("[XML §2.6 [16]]")?;
        let text = self.until("?>", "a processing instruction [XML §2.6 [16]]")?;
        Ok((target, text))
    }

    /// Character data read to the next markup or reference [XML §2.4
    /// \[14\]].
    fn char_data(&mut self) -> Result<()> {
        while let Some(c) = self.peek().filter(|&c| c != '<' && c != '&') {
            if c == ']' && self.looking_at("]]>") {
                return Err(self.malformed("character data holds no ']]>' [XML §2.4 [14]]"));
            }
            self.pending.push(c);
            self.bump();
        }
        Ok(())
    }

    /// A reference in content read after its `&`: its character added to
    /// the text, or its entity's replacement text read next [XML §4.4.2,
    /// §4.6].
    fn content_reference(&mut self) -> Result<()> {
        self.top_level_other()?;
        if self.eat("#") {
            let c = self.char_reference()?;
            self.pending.push(c);
            return Ok(());
        }
        let name = self.reference_name()?;
        match self.general_entity(&name, false)? {
            Expansion::Char(c) => self.pending.push(c),
            Expansion::Text(text) => self.enter(&name, text, false)?,
        }
        Ok(())
    }

    /// The error, of DOCUMENT, if no element is open: a document has no
    /// reference, CDATA section or text but white space outside its
    /// element [XML §2.1 \[1\], §2.8 \[27\]]; noted, of CONTENT.
    fn top_level_other(&mut self) -> Result<()> {
        if !self.open.is_empty() {
            return Ok(());
        }
        self.top.other = true;
        match self.syntax {
            Syntax::Document => {
                Err(self.malformed("a document has no reference, CDATA section or text but white space outside its element [XML §2.1 [1]]"))
            }
            Syntax::Content => Ok(()),
        }
    }

    /// The characters read since the last markup made a text node, or
    /// removed where they are white space that is stripped.
    fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = std::mem::take(&mut self.pending);
        let white = text.chars().all(is_space);
        if !white {
            self.top_level_other()?;
        }
        let strip = self.whitespace == Whitespace::Strip;
        let removed = match self.open.last() {
            None => white && (strip || self.syntax == Syntax::Document),
            Some(open) => white && strip && open.strippable,
        };
        if removed { Ok(()) } else { self.builder.text(&text) }
    }

    /// A start tag read after its `<` [XML §3.1 \[40\], \[44\]]: its element
    /// started, and ended too if the tag is empty.
    fn start_tag(&mut self) -> Result<()> {
        self.flush()?;
        if self.open.is_empty() {
            self.top.elements = self.top.elements.saturating_add(1);
            if self.syntax == Syntax::Document && self.top.elements > 1 {
                return Err(self.malformed("a document has one element [XML §2.1 [1]]"));
            }
        }
        let raw = self.name("[XML §3.1 [40]]")?;
        let (specified, empty) = self.tag_attributes(&raw)?;
        let attributes = self.with_defaults(&raw, specified);
        let start = self.element_start(&raw, attributes)?;
        let strippable = self.strippable(&start);
        self.builder.start_element(start)?;
        if empty {
            self.scope.pop();
            return self.builder.end_element();
        }
        self.open.push(OpenElement { name: raw, frames: self.depth_of_frames(), strippable });
        Ok(())
    }

    /// The attributes a start tag specifies, each named once [XML §3.1
    /// WFC: Unique Att Spec], read to the tag's end; whether it is empty.
    fn tag_attributes(&mut self, element: &str) -> Result<(Vec<RawAttribute>, bool)> {
        let mut attributes: Vec<RawAttribute> = Vec::new();
        loop {
            let spaced = self.space();
            if self.eat("/>") {
                return Ok((attributes, true));
            }
            if self.eat(">") {
                return Ok((attributes, false));
            }
            if !spaced {
                return Err(self.malformed("[XML §3.1 [40]] requires white space before each attribute"));
            }
            let name = self.name("[XML §3.1 [41]]")?;
            if attributes.iter().any(|a| a.name == name) {
                return Err(self.malformed(&format!("<{element}> specifies '{name}' twice [XML §3.1 WFC: Unique Att Spec]")));
            }
            self.space();
            self.expect("=", "[XML §2.8 [25]]")?;
            self.space();
            let kind = self.dtd.attributes(element).iter().find(|d| d.name == name).map_or(Kind::Cdata, |d| d.kind);
            let read = self.attribute_value(true)?;
            let value = if kind == Kind::Cdata { read } else { tokenized(&read) };
            attributes.push(RawAttribute { name, value, kind });
        }
    }

    /// `specified` with each attribute the element type's declarations
    /// default and it does not specify [XML §3.3.2]
    /// [ISO/IEC 9075-14:2006 draft §10.16 GR7d].
    fn with_defaults(&self, element: &str, mut specified: Vec<RawAttribute>) -> Vec<RawAttribute> {
        for declared in self.dtd.attributes(element) {
            if let Some(default) = &declared.default
                && !specified.iter().any(|a| a.name == declared.name)
            {
                specified.push(RawAttribute { name: declared.name.clone(), value: default.clone(), kind: declared.kind });
            }
        }
        specified
    }

    /// Whether the element `start` starts is potentially
    /// whitespace-strippable [ISO/IEC 9075-14:2006 draft §10.16 GR8a]: a
    /// child of the document node unless its `xml:space` is preserve; any
    /// whose `xml:space` is default; a child of one that is, unless its
    /// `xml:space` is preserve.
    fn strippable(&self, start: &ElementStart) -> bool {
        let space = start
            .attributes
            .iter()
            .find(|a| a.name.uri.as_deref() == Some(XML_NAMESPACE) && &*a.name.local == SPACE)
            .map(|a| a.value.as_str());
        let preserve = space == Some(PRESERVE);
        match self.open.last() {
            None => !preserve,
            Some(parent) => space == Some(DEFAULT) || (parent.strippable && !preserve),
        }
    }

    /// An end tag read after its `</` [XML §3.1 \[42\]]: the element open
    /// last, of its name [XML §3 WFC: Element Type Match], begun in the
    /// text being read [XML §4.3.2], ended.
    fn end_tag(&mut self) -> Result<()> {
        let raw = self.name("[XML §3.1 [42]]")?;
        self.space();
        self.expect(">", "[XML §3.1 [42]]")?;
        let Some(open) = self.open.last() else {
            return Err(self.malformed(&format!("the end tag </{raw}> ends no element [XML §3 WFC: Element Type Match]")));
        };
        if open.name != raw {
            return Err(self.malformed(&format!("the end tag </{raw}> does not match <{}> [XML §3 WFC: Element Type Match]", open.name)));
        }
        if open.frames != self.depth_of_frames() {
            return Err(self.malformed(&format!("the element <{raw}> ends in another entity than it begins in [XML §4.3.2]")));
        }
        self.flush()?;
        self.open.pop();
        self.scope.pop();
        self.builder.end_element()
    }
}
