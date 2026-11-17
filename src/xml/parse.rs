//! A string parsed as an XML value [ISO/IEC 9075-14:2006 draft §10.16]:
//! a textual XML 1.0 or XML 1.1 document, or content, as Namespaces in XML
//! modifies them [GR2 to GR5], read into a tree of the data model
//! [GR7, GR9] [XQueryDM §6], its white space stripped as OPTION says
//! \[GR8\].
//!
//! turborel supports Feature X211, XML 1.1 support, so a text of either
//! version is read \[GR6a\]. \[GR5\] A textual XML 1.1 content is read as
//! a well-formed external parsed entity of [XML 1.1] and [Namespaces 1.1]:
//! the draft names XML 1.0 in GR5 as in GR4, which would make GR5 the
//! same rule.
//!
//! \[GR7e\] implementation-defined: no external DTD subset and no
//! external entity is read; a document that has an external subset is
//! read by its internal subset alone, as a processor that does not
//! validate reads it [XML §5.1]. An entity reference that would need an
//! entity or a declaration not read cannot be expanded, and the data
//! model has every entity expanded [XQueryDM §3.2], so it is the
//! exception GR6 raises. \[GR7a ii, iii\] implementation-defined: the
//! document node keeps no notation and no unparsed entity. \[GR7b\] No
//! node has a base URI.
//!
//! A text is read by a `Parser` (`parse/input.rs`): its XML or text
//! declaration (`parse/decl.rs`), its document type declaration
//! (`parse/dtd.rs`, `parse/attlist.rs`), then its content
//! (`parse/content.rs`), names resolved by Namespaces in XML
//! (`parse/names.rs`). A binary string is decoded first
//! (`parse/encoding.rs`). No part recurses: entities are read on a stack
//! of the texts being read, and elements on a stack of those open.

mod attlist;
mod content;
mod decl;
mod dtd;
mod encoding;
mod input;
mod names;
mod refs;

use std::rc::Rc;

use super::chars::Version;
use super::tree::{Builder, Tree};
use crate::error::{Code, Result, SqlError, SqlState};

/// The SYNTAX of a parse [ISO/IEC 9075-14:2006 draft §10.16 GR1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// DOCUMENT: the text is a document.
    Document,
    /// CONTENT: the text is a document or content.
    Content,
}

/// The OPTION of a parse [ISO/IEC 9075-14:2006 draft §10.16 GR1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whitespace {
    /// PRESERVE WHITESPACE: every character is kept.
    Preserve,
    /// STRIP WHITESPACE: text of white space alone is removed \[GR8\].
    Strip,
}

/// A text being read: the document entity, or the replacement text of an
/// entity it references.
#[derive(Debug)]
struct Frame {
    chars: Rc<[char]>,
    at: usize,
    /// The entity's name, none for the document entity.
    entity: Option<Rc<str>>,
    /// Whether it is a parameter entity's.
    parameter: bool,
}

/// An element whose start tag is read and whose end tag is not yet.
#[derive(Debug)]
struct OpenElement {
    /// Its name as written, which its end tag must match.
    name: String,
    /// How many texts were being read when it started: its end tag is
    /// in the same one [XML §4.3.2].
    frames: usize,
    /// Whether it is potentially whitespace-strippable \[GR8a\].
    strippable: bool,
}

/// What the document node's own children have been: what decides whether
/// the text is a document or content \[GR6\].
#[derive(Debug, Default)]
struct TopLevel {
    /// How many elements.
    elements: usize,
    /// Whether text other than white space, a reference or a CDATA
    /// section was a child, which no document has outside its element.
    other: bool,
    /// Whether it had a document type declaration.
    doctype: bool,
}

/// A text being parsed as an XML value.
#[derive(Debug)]
struct Parser {
    frames: Vec<Frame>,
    version: Version,
    syntax: Syntax,
    whitespace: Whitespace,
    /// How many characters entities' replacement texts have added.
    expanded: usize,
    decl: decl::Declaration,
    dtd: dtd::Dtd,
    builder: Builder,
    open: Vec<OpenElement>,
    scope: names::Scope,
    /// The characters read since the last markup, the next text node's.
    pending: String,
    top: TopLevel,
}

/// The tree a text `text` makes, parsed by `syntax` and `whitespace`
/// [ISO/IEC 9075-14:2006 draft §10.16].
///
/// # Errors
///
/// SQLSTATE 2200M, invalid XML document, if `syntax` is DOCUMENT and
/// `text` is no textual XML 1.0 or XML 1.1 document \[GR6a i\]; SQLSTATE
/// 2200N, invalid XML content, if `syntax` is CONTENT and `text` is no
/// textual content of either \[GR6a ii\], or an entity reference cannot
/// be expanded; SQLCODE -900 if it is beyond `MAX_NESTING` or
/// `MAX_TEXT`.
pub fn parse(text: &str, syntax: Syntax, whitespace: Whitespace) -> Result<Tree> {
    let chars: Vec<char> = text.chars().collect();
    let mut parser = Parser::new(syntax, whitespace);
    parser.begin(&chars)?;
    parser.content()?;
    parser.verdict()?;
    parser.builder.finish()
}

/// The tree a binary string `bytes` makes, decoded by its encoding
/// [XML §4.3.3, Appendix F] then parsed as `parse` parses.
///
/// # Errors
///
/// As `parse`, and SQLSTATE 2200M or 2200N if `bytes` is in an encoding
/// turborel does not read, or not in the one it declares [XML §4.3.3].
pub fn parse_bytes(bytes: &[u8], syntax: Syntax, whitespace: Whitespace) -> Result<Tree> {
    let text = encoding::decode(bytes).map_err(|why| invalid(syntax, &why, None))?;
    parse(&text, syntax, whitespace)
}

/// The exception of a text that is not what `syntax` requires, `why`,
/// at `place` if known [ISO/IEC 9075-14:2006 draft §10.16 GR6a].
fn invalid(syntax: Syntax, why: &str, place: Option<&str>) -> SqlError {
    let (rule, what, state) = match syntax {
        Syntax::Document => ("GR6a i", "a well-formed XML document", SqlState::INVALID_XML_DOCUMENT),
        Syntax::Content => ("GR6a ii", "well-formed XML content", SqlState::INVALID_XML_CONTENT),
    };
    let at = place.map(|p| format!(" ({p})")).unwrap_or_default();
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §10.16 {rule}] the string is not {what}: {why}{at}")).with_state(state)
}

impl Parser {
    /// A parser of nothing yet.
    fn new(syntax: Syntax, whitespace: Whitespace) -> Parser {
        Parser {
            frames: Vec::new(),
            version: Version::V10,
            syntax,
            whitespace,
            expanded: 0,
            decl: decl::Declaration::default(),
            dtd: dtd::Dtd::default(),
            builder: Builder::document(),
            open: Vec::new(),
            scope: names::Scope::default(),
            pending: String::new(),
            top: TopLevel::default(),
        }
    }

    /// The text read as a document or as content, as `syntax` requires
    /// \[GR6a\]: a document has one element, no text but white space beside
    /// it and an XML declaration if any [XML §2.1 \[1\], §2.8 \[22\]];
    /// content has no document type declaration, and a text declaration
    /// if any [XML §4.3.2 \[78\]].
    fn verdict(&self) -> Result<()> {
        let document = self.document_form();
        match (self.syntax, document) {
            (Syntax::Document, Ok(())) | (Syntax::Content, Ok(())) => Ok(()),
            (Syntax::Document, Err(why)) => Err(invalid(self.syntax, why, None)),
            (Syntax::Content, Err(why)) => self.entity_form().map_err(|other| invalid(self.syntax, &format!("{why}, and {other}"), None)),
        }
    }

    /// Why the text is no document, if it is not.
    fn document_form(&self) -> std::result::Result<(), &'static str> {
        if self.top.elements != 1 {
            return Err(if self.top.elements == 0 {
                "a document has an element [XML §2.1 [1]]"
            } else {
                "a document has one element [XML §2.1 [1]]"
            });
        }
        if self.top.other {
            return Err("a document has no text, reference or CDATA section outside its element [XML §2.8 [27]]");
        }
        self.decl.as_xml_declaration()
    }

    /// Why the text is no well-formed external parsed entity, if it is
    /// not.
    fn entity_form(&self) -> std::result::Result<(), &'static str> {
        if self.top.doctype {
            return Err("content has no document type declaration [XML §4.3.2 [78]]");
        }
        self.decl.as_text_declaration()
    }
}

#[cfg(test)]
mod tests;
