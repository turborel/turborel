//! \[DESIGN\] RULES.md D86: the stored form read as events, one at a time,
//! in document order, building no tree: what an XML index derives its keys
//! from in one pass (`index/xml.rs`). Its text is borrowed from the
//! stored bytes, and only the names of the item read are held.

use super::{
    ATOMIC, ATTRIBUTE, COMMENT, DOCUMENT, ELEMENT, END, FORMAT_VERSION, NODE, PROCESSING_INSTRUCTION, TEXT, WHAT, decode_names,
    skip_annotations,
};
use crate::bytes::Reader;
use crate::error::Result;
use crate::xml::tree::QName;

/// An event of the stored form: a node opened, closed or met, or an item
/// begun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    /// A node item begins; `Events::names` are its names.
    Node,
    /// An atomic item, which has no node.
    Atomic,
    /// A document node, opened.
    Document,
    /// An element, opened: its name's number among the item's names, and
    /// its attributes', each with its value.
    Element {
        /// The number of its name.
        name: u32,
        /// Each attribute's name's number and its value.
        attributes: Vec<(u32, &'a str)>,
    },
    /// The node opened last, closed.
    End,
    /// A text node.
    Text(&'a str),
    /// A comment or processing instruction node.
    Other,
    /// An attribute that is its item's root, standing alone.
    Attribute {
        /// The number of its name.
        name: u32,
        /// Its value.
        value: &'a str,
    },
}

/// A reader of the stored form's events (`Events::next_event`).
pub struct Events<'a> {
    reader: Reader<'a>,
    /// Items not yet begun.
    items: u32,
    /// The names of the node item being read.
    names: Vec<QName>,
    /// Nodes open in the node item being read; none between items.
    open: usize,
    /// Whether a node item is being read.
    within: bool,
}

impl<'a> Events<'a> {
    /// A reader of the events of `bytes`, a stored form.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they begin with no stored form of version 1.
    pub fn new(bytes: &'a [u8]) -> Result<Events<'a>> {
        let mut reader = Reader::new(bytes, WHAT);
        let version = reader.u8()?;
        if version != FORMAT_VERSION {
            return Err(reader.corrupt(&format!("its version is {version}, not {FORMAT_VERSION}")));
        }
        let items = reader.u32()?;
        Ok(Events { reader, items, names: Vec::new(), open: 0, within: false })
    }

    /// The names of the node item being read, by number.
    #[must_use]
    pub fn names(&self) -> &[QName] {
        &self.names
    }

    /// The next event; none after the last item.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are no stored form: a field past the end
    /// or malformed, an unknown tag, an end closing no node, or bytes after
    /// the last item.
    pub fn next_event(&mut self) -> Result<Option<Event<'a>>> {
        if self.within {
            return self.node_event().map(Some);
        }
        if self.items == 0 {
            self.reader.finish()?;
            return Ok(None);
        }
        self.items = self.items.saturating_sub(1);
        match self.reader.u8()? {
            NODE => {
                self.names = decode_names(&mut self.reader)?;
                skip_annotations(&mut self.reader)?;
                self.within = true;
                Ok(Some(Event::Node))
            }
            ATOMIC => {
                self.reader.u8()?;
                self.reader.bytes()?;
                Ok(Some(Event::Atomic))
            }
            tag => Err(self.reader.corrupt(&format!("an item's tag is {tag}"))),
        }
    }

    /// The next event of the node item being read, which ends it if it
    /// leaves no node open.
    fn node_event(&mut self) -> Result<Event<'a>> {
        let event = match self.reader.u8()? {
            DOCUMENT => Event::Document,
            ELEMENT => self.element()?,
            END => {
                if self.open == 0 {
                    return Err(self.reader.corrupt("an end closes no node"));
                }
                Event::End
            }
            TEXT => Event::Text(self.text()?),
            COMMENT => {
                self.reader.bytes()?;
                Event::Other
            }
            PROCESSING_INSTRUCTION => {
                self.reader.u32()?;
                self.reader.bytes()?;
                Event::Other
            }
            ATTRIBUTE => {
                let (name, value) = self.attribute()?;
                Event::Attribute { name, value }
            }
            tag => return Err(self.reader.corrupt(&format!("an event's tag is {tag}"))),
        };
        self.open = match event {
            Event::Document | Event::Element { .. } => self.open.saturating_add(1),
            Event::End => self.open.saturating_sub(1),
            Event::Node | Event::Atomic | Event::Text(_) | Event::Other | Event::Attribute { .. } => self.open,
        };
        self.within = self.open > 0;
        Ok(event)
    }

    /// A string field, borrowed.
    fn text(&mut self) -> Result<&'a str> {
        let bytes = self.reader.bytes()?;
        std::str::from_utf8(bytes).map_err(|_| self.reader.corrupt("a string is not UTF-8"))
    }

    /// An element's event after its tag: its name, its annotation, flags
    /// and bindings passed over, and its attributes.
    fn element(&mut self) -> Result<Event<'a>> {
        let name = self.reader.u32()?;
        self.reader.u32()?;
        self.reader.u8()?;
        for _ in 0..self.reader.u32()? {
            if self.reader.u8()? != 0 {
                self.reader.bytes()?;
            }
            self.reader.bytes()?;
        }
        let mut attributes = Vec::new();
        for _ in 0..self.reader.u32()? {
            attributes.push(self.attribute()?);
        }
        Ok(Event::Element { name, attributes })
    }

    /// An attribute: its name's number and its value, its flags and
    /// annotation passed over.
    fn attribute(&mut self) -> Result<(u32, &'a str)> {
        let name = self.reader.u32()?;
        self.reader.u8()?;
        self.reader.u32()?;
        Ok((name, self.text()?))
    }
}
