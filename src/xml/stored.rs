//! \[DESIGN\] RULES.md D106: the stored form of an XML value, version 1,
//! in the fields of D30. It is read and written in one pass, in document
//! order, as Pal et al. store XML: binary, streamable, its names
//! tokenized.
//!
//! ```text
//! u8 version (1), u32 item count, then each item:
//!   u8 0, a node: u32 name count, each name
//!       u8 flags (1 prefix, 2 namespace URI), [str prefix], [str URI], str local,
//!     u32 annotation count, each a type annotation
//!       u32 name, u8 typing (0 xs:anySimpleType, 1 mixed, 2 empty,
//!       3 element-only, 4 atomic, 5 list, 6 items of several types),
//!       [an atomic type, of 4 and 5], [u32 count, each an atomic type, of 6],
//!       an atomic type u8 type, u32 its user-defined type's name + 1 (0 none),
//!     then its events, to the end of its root:
//!       u8 1 document                                  opens
//!       u8 2 element  u32 name, u32 annotation (0 none, else its number + 1),
//!                     u8 flags (1 nilled, 2 ID, 4 IDREF),
//!                     u32 bindings, each u8 has prefix, [str prefix], str URI,
//!                     u32 attributes, each u32 name, u8 flags (1 ID, 2 IDREF),
//!                     u32 annotation, str value
//!                                                      opens
//!       u8 3 end                                       closes
//!       u8 4 text str | u8 5 comment str | u8 6 processing instruction u32 name, str
//!       u8 7 attribute u32 name, u8 flags, u32 annotation, str  a root alone
//!   u8 1, an atomic value: u8 type, str canonical lexical form, then of
//!       an xs:QName or xs:NOTATION str namespace URI, empty of none, then
//!       u8 0 of a built-in type, or 1 and its user-defined type's name,
//!       u8 flags (1 prefix, 2 namespace URI), [str prefix], [str URI],
//!       str local; the
//!       types 0 xs:string, 1 xdt:untypedAtomic, 2 xs:boolean,
//!       3 xs:decimal, 4 xs:integer, 5 xs:float, 6 xs:double, 7 xs:date,
//!       8 xs:time, 9 xs:dateTime, 10 xdt:yearMonthDuration,
//!       11 xdt:dayTimeDuration, 12 xs:hexBinary, and the rest as
//!       `ATOMIC_TYPES` numbers them
//! ```
//!
//! A node's type annotation, nilled, is-id and is-idrefs properties are
//! kept [XQueryDM §3.3], and the user-defined atomic type of an atomic
//! value, of an atomic item or of a node's typed value [XQueryDM §2.6.1,
//! §3.3.1.2]. The annotations were added to version 1 in place, as no
//! database of an earlier layout is read (RULES.md D106).
//!
//! A node stored is copied, as storing a value copies it
//! [ISO/IEC 9075-14:2006 draft §10.2 GR1b, §10.18]: its tree is its
//! subtree alone, and an element that had a parent declares every binding
//! in its scope [XQueryDM §6.2.1].

use std::collections::HashMap;

use annotations::{decode_annotations, put_typing, skip_annotations, value_types};
use atomic::{decode_atomic, put_atomic, put_name, read_name};
pub use events::{Event, Events};

use super::sequence::{AtomicType, Item, Node, Sequence};
use super::tree::{Annotation, Binding, Builder, ElementStart, Kind, NodeId, QName, Specified, Tree, XML_PREFIX};
use crate::bytes::{Put, Reader};
use crate::error::Result;

/// The version of the stored form (RULES.md D106).
pub const FORMAT_VERSION: u8 = 1;

// Item tags.
/// A node and the nodes below it.
const NODE: u8 = 0;
/// An atomic value.
const ATOMIC: u8 = 1;

// Event tags.
/// A document node, opened.
const DOCUMENT: u8 = 1;
/// An element, opened.
const ELEMENT: u8 = 2;
/// The end of the node opened last.
const END: u8 = 3;
/// A text node.
const TEXT: u8 = 4;
/// A comment node.
const COMMENT: u8 = 5;
/// A processing instruction node.
const PROCESSING_INSTRUCTION: u8 = 6;
/// An attribute standing alone.
const ATTRIBUTE: u8 = 7;

// Flags.
/// A name's prefix follows.
const HAS_PREFIX: u8 = 1;
/// A name's namespace URI follows.
const HAS_URI: u8 = 2;
/// An attribute is an ID.
const IS_ID: u8 = 1;
/// An attribute is an IDREF or IDREFS.
const IS_IDREFS: u8 = 2;
/// An element is nilled.
const ELEMENT_NILLED: u8 = 1;
/// An element is an ID.
const ELEMENT_ID: u8 = 2;
/// An element is an IDREF or IDREFS.
const ELEMENT_IDREFS: u8 = 4;

/// The atomic types, each stored as its place here, its tag, written
/// beside it. The tags are part of the stored form, version 1: a type
/// added is added at the end, and no tag is renumbered, so each value
/// stored keeps its type.
const ATOMIC_TYPES: [AtomicType; 44] = [
    AtomicType::String,             // 0
    AtomicType::UntypedAtomic,      // 1
    AtomicType::Boolean,            // 2
    AtomicType::Decimal,            // 3
    AtomicType::Integer,            // 4
    AtomicType::Float,              // 5
    AtomicType::Double,             // 6
    AtomicType::Date,               // 7
    AtomicType::Time,               // 8
    AtomicType::DateTime,           // 9
    AtomicType::YearMonthDuration,  // 10
    AtomicType::DayTimeDuration,    // 11
    AtomicType::HexBinary,          // 12
    AtomicType::Duration,           // 13
    AtomicType::GYearMonth,         // 14
    AtomicType::GYear,              // 15
    AtomicType::GMonthDay,          // 16
    AtomicType::GDay,               // 17
    AtomicType::GMonth,             // 18
    AtomicType::Base64Binary,       // 19
    AtomicType::AnyUri,             // 20
    AtomicType::QName,              // 21
    AtomicType::Notation,           // 22
    AtomicType::NormalizedString,   // 23
    AtomicType::Token,              // 24
    AtomicType::Language,           // 25
    AtomicType::NmToken,            // 26
    AtomicType::Name,               // 27
    AtomicType::NcName,             // 28
    AtomicType::Id,                 // 29
    AtomicType::IdRef,              // 30
    AtomicType::Entity,             // 31
    AtomicType::NonPositiveInteger, // 32
    AtomicType::NegativeInteger,    // 33
    AtomicType::Long,               // 34
    AtomicType::Int,                // 35
    AtomicType::Short,              // 36
    AtomicType::Byte,               // 37
    AtomicType::NonNegativeInteger, // 38
    AtomicType::UnsignedLong,       // 39
    AtomicType::UnsignedInt,        // 40
    AtomicType::UnsignedShort,      // 41
    AtomicType::UnsignedByte,       // 42
    AtomicType::PositiveInteger,    // 43
];

/// What a reader of the stored form names in its errors.
const WHAT: &str = "an XML value";

/// A count as a `u32`, which every count of a tree within `MAX_TEXT` is.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The stored form of `value`.
#[must_use]
pub fn encode(value: &Sequence) -> Vec<u8> {
    let mut out = vec![FORMAT_VERSION];
    out.put_u32(count(value.items().len()));
    for item in value.items() {
        match item {
            Item::Node(node) => {
                out.put_u8(NODE);
                encode_node(node, &mut out);
            }
            Item::Atomic(atomic) => {
                out.put_u8(ATOMIC);
                put_atomic(&mut out, atomic);
            }
        }
    }
    out
}

/// The names of a node's subtree, each once, numbered as written, and its
/// type annotations, each once, numbered from 1.
#[derive(Default)]
struct Names<'a> {
    list: Vec<&'a QName>,
    numbers: HashMap<&'a QName, u32>,
    annotations: Vec<&'a Annotation>,
    annotated: HashMap<&'a Annotation, u32>,
}

impl<'a> Names<'a> {
    /// The number of `name`, given it if it has none.
    fn number(&mut self, name: &'a QName) -> u32 {
        let next = count(self.list.len());
        *self.numbers.entry(name).or_insert_with(|| {
            self.list.push(name);
            next
        })
    }

    /// The number of the annotation of node `id` of `tree`, plus 1; 0 if
    /// it has none.
    fn annotation(&mut self, tree: &'a Tree, id: NodeId) -> u32 {
        let Some(annotation) = tree.annotation(id) else { return 0 };
        self.number(&annotation.name);
        for t in value_types(&annotation.typing) {
            if let Some(name) = &t.name {
                self.number(name);
            }
        }
        let next = count(self.annotations.len()).saturating_add(1);
        *self.annotated.entry(annotation).or_insert_with(|| {
            self.annotations.push(annotation);
            next
        })
    }
}

/// `node` and the nodes below it, stored: its names, then its events.
fn encode_node(node: &Node, out: &mut Vec<u8>) {
    let tree = node.tree();
    let mut names = Names::default();
    let mut events = Vec::new();
    let mut at = Some(node.id());
    while let Some(id) = at {
        let opened = encode_event(tree, id, id == node.id(), &mut names, &mut events);
        at = match tree.first_child(id).filter(|_| opened) {
            Some(child) => Some(child),
            None => after(tree, node.id(), id, opened, &mut events),
        };
    }
    out.put_u32(count(names.list.len()));
    for name in names.list {
        put_name(out, name);
    }
    out.put_u32(count(names.annotations.len()));
    for annotation in &names.annotations {
        out.put_u32(names.numbers.get(&annotation.name).copied().unwrap_or(0));
        put_typing(out, &annotation.typing, &names.numbers);
    }
    out.extend(events);
}

/// The node after `id` in document order below `root`, an end event
/// written for each node left, `id` too if it was opened with no child;
/// none at the end of `root`.
fn after(tree: &Tree, root: NodeId, id: NodeId, opened: bool, events: &mut Vec<u8>) -> Option<NodeId> {
    if opened {
        events.put_u8(END);
    }
    let mut at = id;
    while at != root {
        if let Some(sibling) = tree.next_sibling(at) {
            return Some(sibling);
        }
        at = tree.parent(at)?;
        events.put_u8(END);
    }
    None
}

/// The event of node `id`, the root of what is stored if `root`; whether
/// it opened a node, whose children follow it.
fn encode_event<'a>(tree: &'a Tree, id: NodeId, root: bool, names: &mut Names<'a>, events: &mut Vec<u8>) -> bool {
    match tree.kind(id) {
        Some(Kind::Document) => {
            events.put_u8(DOCUMENT);
            true
        }
        Some(Kind::Element) => {
            encode_element(tree, id, root, names, events);
            true
        }
        Some(Kind::Text) => {
            events.put_u8(TEXT);
            events.put_str(tree.content(id));
            false
        }
        Some(Kind::Comment) => {
            events.put_u8(COMMENT);
            events.put_str(tree.content(id));
            false
        }
        Some(Kind::ProcessingInstruction) => {
            events.put_u8(PROCESSING_INSTRUCTION);
            events.put_u32(tree.name(id).map_or(0, |n| names.number(n)));
            events.put_str(tree.content(id));
            false
        }
        Some(Kind::Attribute) => {
            events.put_u8(ATTRIBUTE);
            encode_attribute(tree, id, names, events);
            false
        }
        None => false,
    }
}

/// An element's event: its name, its bindings, every one in its scope
/// if it is the root stored, and its attributes.
fn encode_element<'a>(tree: &'a Tree, id: NodeId, root: bool, names: &mut Names<'a>, events: &mut Vec<u8>) {
    events.put_u8(ELEMENT);
    events.put_u32(tree.name(id).map_or(0, |n| names.number(n)));
    events.put_u32(names.annotation(tree, id));
    let flags = [(tree.nilled(id), ELEMENT_NILLED), (tree.is_id(id), ELEMENT_ID), (tree.is_idrefs(id), ELEMENT_IDREFS)];
    events.put_u8(flags.iter().filter(|(on, _)| *on).fold(0, |all, (_, flag)| all | flag));
    let bindings: Vec<(Option<&str>, &str)> = if root && tree.parent(id).is_some() {
        tree.in_scope(id).into_iter().filter(|(p, _)| *p != Some(XML_PREFIX)).collect()
    } else {
        tree.declared(id).iter().map(|b| (b.prefix.as_deref(), &*b.uri)).collect()
    };
    events.put_u32(count(bindings.len()));
    for (prefix, uri) in bindings {
        events.put_u8(u8::from(prefix.is_some()));
        if let Some(p) = prefix {
            events.put_str(p);
        }
        events.put_str(uri);
    }
    events.put_u32(count(tree.attributes(id).count()));
    for attribute in tree.attributes(id) {
        encode_attribute(tree, attribute, names, events);
    }
}

/// An attribute: its name, its flags and its value.
fn encode_attribute<'a>(tree: &'a Tree, id: NodeId, names: &mut Names<'a>, events: &mut Vec<u8>) {
    events.put_u32(tree.name(id).map_or(0, |n| names.number(n)));
    events.put_u8(if tree.is_id(id) { IS_ID } else { 0 } | if tree.is_idrefs(id) { IS_IDREFS } else { 0 });
    events.put_u32(names.annotation(tree, id));
    events.put_str(tree.content(id));
}

/// The value `bytes` store.
///
/// # Errors
///
/// SQLCODE -916 if they are no stored form of version 1: a field past the
/// end or malformed, an unknown tag, a name not numbered, events that do
/// not make one tree, or bytes after the last item.
pub fn decode(bytes: &[u8]) -> Result<Sequence> {
    let mut reader = Reader::new(bytes, WHAT);
    let version = reader.u8()?;
    if version != FORMAT_VERSION {
        return Err(reader.corrupt(&format!("its version is {version}, not {FORMAT_VERSION}")));
    }
    let items = reader.u32()?;
    let mut sequence = Vec::new();
    for _ in 0..items {
        sequence.push(match reader.u8()? {
            NODE => Item::Node(Node::root(decode_node(&mut reader)?)),
            ATOMIC => Item::Atomic(decode_atomic(&mut reader)?),
            tag => return Err(reader.corrupt(&format!("an item's tag is {tag}"))),
        });
    }
    reader.finish()?;
    Ok(Sequence::new(sequence))
}

/// A node's tree read: its names, then its events to the end of its
/// root.
fn decode_node(reader: &mut Reader<'_>) -> Result<Tree> {
    let names = decode_names(reader)?;
    let annotations = decode_annotations(reader, &names)?;
    let names = Tables { names, annotations };
    let first = reader.u8()?;
    let document = first == DOCUMENT;
    let (mut builder, mut open) = if document { (Builder::document(), 1_usize) } else { (Builder::parentless(), 0) };
    let mut tag = if document { reader.u8()? } else { first };
    loop {
        open = decode_event(reader, &names, &mut builder, tag, open)?;
        if open == 0 {
            break;
        }
        tag = reader.u8()?;
    }
    builder.finish().map_err(|e| reader.corrupt(&format!("its events make no tree: {}", e.described())))
}

/// The event of tag `tag` read into `builder`, `open` nodes open before
/// it; how many are open after it.
fn decode_event(reader: &mut Reader<'_>, names: &Tables, builder: &mut Builder, tag: u8, open: usize) -> Result<usize> {
    let built = match tag {
        ELEMENT => {
            let (start, typed) = decode_element(reader, names)?;
            builder.start_element(start).and_then(|()| annotate(builder, &typed)).map(|()| open.saturating_add(1))
        }
        END => {
            if open == 0 {
                return Err(reader.corrupt("an end closes no node"));
            }
            let closes_element = builder.depth() > 0;
            let closed = open.saturating_sub(1);
            if closes_element { builder.end_element().map(|()| closed) } else { Ok(closed) }
        }
        TEXT => {
            let text = reader.str()?;
            builder.text(&text).map(|()| open)
        }
        COMMENT => {
            let text = reader.str()?;
            builder.comment(&text).map(|()| open)
        }
        PROCESSING_INSTRUCTION => {
            let target = name(reader, &names.names)?;
            let text = reader.str()?;
            builder.processing_instruction(&target.local, &text).map(|()| open)
        }
        ATTRIBUTE => {
            let (attribute, annotation) = decode_attribute(reader, names)?;
            let typed = Typed { element: None, attributes: vec![(annotation, attribute.is_id, attribute.is_idrefs)] };
            builder.attribute(&attribute).and_then(|()| annotate(builder, &typed)).map(|()| open)
        }
        _ => return Err(reader.corrupt(&format!("an event's tag is {tag}"))),
    };
    built.map_err(|e| reader.corrupt(&format!("its events make no tree: {}", e.described())))
}

/// A node's table of names read.
fn decode_names(reader: &mut Reader<'_>) -> Result<Vec<QName>> {
    let n = reader.u32()?;
    let mut names = Vec::new();
    for _ in 0..n {
        names.push(read_name(reader)?);
    }
    Ok(names)
}

/// A name read by its number.
fn name<'n>(reader: &mut Reader<'_>, names: &'n [QName]) -> Result<&'n QName> {
    let number = reader.u32()?;
    usize::try_from(number).ok().and_then(|n| names.get(n)).ok_or_else(|| reader.corrupt(&format!("the name {number} is not numbered")))
}

/// A node's tables: its names and its type annotations.
struct Tables {
    names: Vec<QName>,
    annotations: Vec<Annotation>,
}

/// The annotation numbered `number` plus 1, none of 0.
fn annotation_of(reader: &mut Reader<'_>, tables: &Tables) -> Result<Option<Annotation>> {
    let number = reader.u32()?;
    let Some(index) = number.checked_sub(1) else { return Ok(None) };
    let found = usize::try_from(index).ok().and_then(|i| tables.annotations.get(i));
    found.cloned().map(Some).ok_or_else(|| reader.corrupt(&format!("the annotation {number} is not numbered")))
}

/// What an element or attribute event says of its nodes' types: the
/// element's annotation and its nilled, is-id and is-idrefs properties,
/// and each attribute's annotation, is-id and is-idrefs.
struct Typed {
    element: Option<(Option<Annotation>, (bool, bool, bool))>,
    attributes: Vec<(Option<Annotation>, bool, bool)>,
}

/// The nodes just made annotated as `typed` says: the element open and
/// its attributes, or an attribute standing alone, the root.
fn annotate(builder: &mut Builder, typed: &Typed) -> Result<()> {
    let element = if typed.element.is_some() { builder.current() } else { None };
    if let (Some((Some(annotation), flags)), Some(at)) = (&typed.element, element) {
        builder.annotate(at, annotation, *flags)?;
    }
    for (i, (annotation, is_id, is_idrefs)) in typed.attributes.iter().enumerate() {
        let at = match element {
            Some(e) => e.nth_attribute(i),
            None => Some(NodeId::ROOT),
        };
        if let (Some(annotation), Some(at)) = (annotation, at) {
            builder.annotate(at, annotation, (false, *is_id, *is_idrefs))?;
        }
    }
    Ok(())
}

/// An element's event read after its tag, and what it says of types.
fn decode_element(reader: &mut Reader<'_>, tables: &Tables) -> Result<(ElementStart, Typed)> {
    let element = name(reader, &tables.names)?.clone();
    let annotation = annotation_of(reader, tables)?;
    let flags = reader.u8()?;
    let element_flags = (flags & ELEMENT_NILLED != 0, flags & ELEMENT_ID != 0, flags & ELEMENT_IDREFS != 0);
    let mut bindings = Vec::new();
    for _ in 0..reader.u32()? {
        let prefix = if reader.u8()? != 0 { Some(reader.str()?.into()) } else { None };
        bindings.push(Binding { prefix, uri: reader.str()?.into() });
    }
    let mut attributes = Vec::new();
    let mut typed = Typed { element: Some((annotation, element_flags)), attributes: Vec::new() };
    for _ in 0..reader.u32()? {
        let (attribute, attribute_annotation) = decode_attribute(reader, tables)?;
        typed.attributes.push((attribute_annotation, attribute.is_id, attribute.is_idrefs));
        attributes.push(attribute);
    }
    Ok((ElementStart { name: element, bindings, attributes }, typed))
}

/// An attribute read: its name, its flags, its annotation and its value.
fn decode_attribute(reader: &mut Reader<'_>, tables: &Tables) -> Result<(Specified, Option<Annotation>)> {
    let attribute = name(reader, &tables.names)?.clone();
    let flags = reader.u8()?;
    let annotation = annotation_of(reader, tables)?;
    let value = reader.str()?;
    Ok((Specified { name: attribute, value, is_id: flags & IS_ID != 0, is_idrefs: flags & IS_IDREFS != 0 }, annotation))
}

mod annotations;
mod atomic;
mod events;
#[cfg(test)]
mod tests;
