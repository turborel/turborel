//! \[DESIGN\] RULES.md D86: the nodes an XML index keys, derived from a
//! value's stored form in one pass over its events (`xml::Events`), no
//! tree built. Every element and attribute below each node item is keyed,
//! the item being the context a path starts from: its path, the first
//! octets of its string value, and its ORDPATH [Pal et al. §2.2, §3.1.1].
//!
//! An element's string value is its text descendants' content in document
//! order [XQueryDM §6.2.3], known as it ends; each element open keeps the
//! first `VALUE_PREFIX_BYTES` octets of its own, and whether there were
//! more. An element's text is all its ancestors' too, so an ancestor's
//! octets are never fewer: text is added to the elements open from the
//! innermost out, up to the first that is full, so each element takes at
//! most its prefix's octets, however deep the nesting.

use super::VALUE_PREFIX_BYTES;
use super::keys::{ATTRIBUTE, ELEMENT, put_ordinal};
use crate::error::{Code, Result, SqlError};
use crate::xml::{Event, Events, MAX_NESTING, QName};

/// \[DESIGN\] How many elements and attributes of one value an XML index
/// keys at most (RULES.md D18, D86): a value with more is refused as it
/// is indexed, SQLCODE -900, so the work and the entries one row's value
/// makes are bounded. The value is no measurement: it is a quarter of
/// `xml::MAX_NODES`, a document of some megabytes.
pub const MAX_KEYED_NODES: usize = 1 << 18;

/// A node keyed: its path, `[kind][tokens]` (`keys.rs`), the first octets
/// of its string value and whether they are all of it, and its ORDPATH's
/// ordinals.
pub(super) struct Keyed<'a> {
    pub(super) path: &'a [u8],
    pub(super) text: &'a [u8],
    pub(super) whole: bool,
    pub(super) ordpath: &'a [u8],
}

/// The token of each name, of an item's names, or none if it has none.
pub(super) type Tokens<'n> = dyn FnMut(&QName) -> Result<Option<u64>> + 'n;

/// What is done with each node keyed, in the order derived.
pub(super) type Visit<'v> = dyn FnMut(&Keyed<'_>) -> Result<()> + 'v;

/// A node open: an element, or its item's root.
struct Open {
    /// The ordinals of its name's token and of each element's above it to
    /// its item, as a path holds them; empty for the item's root; none if
    /// a name has no token, so neither it nor what is below it is keyed.
    tokens: Option<Vec<u8>>,
    /// Whether it is keyed: an element below its item's root.
    keyed: bool,
    /// The octets of its ORDPATH.
    label: usize,
    /// The component its next child takes.
    next: u64,
    /// The first octets of its string value.
    text: Vec<u8>,
    /// Whether its string value has more octets than `text`.
    cut: bool,
}

/// The derivation of one value's nodes.
struct Deriving<'d, 'n, 'v> {
    open: Vec<Open>,
    /// The ORDPATH of the node met last, its ordinals.
    ordpath: Vec<u8>,
    /// The token of each name of the item read, by number.
    tokens: Vec<Option<u64>>,
    /// The items begun.
    items: u64,
    /// The nodes keyed.
    keyed: usize,
    names: &'d mut Tokens<'n>,
    visit: &'d mut Visit<'v>,
    /// The path of the node keyed last.
    path: Vec<u8>,
}

/// Derives the nodes `bytes`, a stored form, keys, each given to `visit`,
/// each name's token by `names`; how many.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are no stored form, or nest past
/// `xml::MAX_NESTING`; -900 past `MAX_KEYED_NODES`; `names`' and
/// `visit`'s.
pub(super) fn derive(bytes: &[u8], names: &mut Tokens<'_>, visit: &mut Visit<'_>) -> Result<usize> {
    let mut events = Events::new(bytes)?;
    let mut deriving =
        Deriving { open: Vec::new(), ordpath: Vec::new(), tokens: Vec::new(), items: 0, keyed: 0, names, visit, path: Vec::new() };
    while let Some(event) = events.next_event()? {
        if event == Event::Node {
            deriving.tokens = events.names().iter().map(|n| (deriving.names)(n)).collect::<Result<_>>()?;
        }
        deriving.event(event)?;
    }
    Ok(deriving.keyed)
}

/// The component of the `n`th child, from 0: the odd numbers, as a load
/// labels nodes [Pal et al. §2.2].
fn component(n: u64) -> u64 {
    n.saturating_mul(2).saturating_add(1)
}

impl Deriving<'_, '_, '_> {
    /// Takes `event`.
    fn event(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Node | Event::Atomic => {
                self.ordpath.clear();
                put_ordinal(&mut self.ordpath, component(self.items));
                self.items = self.items.saturating_add(1);
                Ok(())
            }
            Event::Document => self.push(None),
            Event::Element { name, attributes } => {
                self.push(Some(name))?;
                attributes.into_iter().try_for_each(|(n, value)| self.attribute(n, value))
            }
            Event::End => self.end(),
            Event::Text(text) => {
                self.child();
                self.add_text(text.as_bytes());
                Ok(())
            }
            Event::Other => {
                self.child();
                Ok(())
            }
            // An attribute that is its item is no node below its item.
            Event::Attribute { .. } => Ok(()),
        }
    }

    /// The token of name `number` of the item read.
    fn token(&self, number: u32) -> Option<u64> {
        usize::try_from(number).ok().and_then(|n| self.tokens.get(n)).copied().flatten()
    }

    /// The next child of the node open last takes its place in the
    /// ORDPATH, after that node's.
    fn child(&mut self) {
        if let Some(parent) = self.open.last_mut() {
            self.ordpath.truncate(parent.label);
            put_ordinal(&mut self.ordpath, component(parent.next));
            parent.next = parent.next.saturating_add(1);
        }
    }

    /// Opens a document node, or an element named `name`: its item's
    /// root if nothing is open, which takes the item's ORDPATH.
    fn push(&mut self, name: Option<u32>) -> Result<()> {
        if self.open.len() > MAX_NESTING {
            return Err(SqlError::new(
                Code::Corrupt,
                format!("[DESIGN] a stored XML value nests deeper than {MAX_NESTING} (RULES.md D86)"),
            ));
        }
        self.child();
        // A document node is only ever an item's root, so `name` is none
        // only there.
        let tokens = match self.open.last() {
            None => Some(Vec::new()),
            Some(parent) => {
                let token = name.and_then(|n| self.token(n));
                token.zip(parent.tokens.as_ref()).map(|(t, above)| [ordinal(t), above.clone()].concat())
            }
        };
        let root = self.open.is_empty();
        let keyed = !root && tokens.is_some();
        self.open.push(Open { tokens, keyed, label: self.ordpath.len(), next: 0, text: Vec::new(), cut: root });
        Ok(())
    }

    /// An attribute of the element opened last, named `name`, of `value`.
    fn attribute(&mut self, name: u32, value: &str) -> Result<()> {
        self.child();
        let Some(path) = self.open.last().and_then(|e| e.tokens.as_ref()).zip(self.token(name)) else { return Ok(()) };
        self.path = [vec![ATTRIBUTE], ordinal(path.1), path.0.clone()].concat();
        let bytes = value.as_bytes();
        let text = bytes.get(..VALUE_PREFIX_BYTES).unwrap_or(bytes);
        self.keyed_node(text, text.len() == bytes.len())
    }

    /// Closes the node opened last, keyed if it is a keyed element.
    fn end(&mut self) -> Result<()> {
        let Some(closed) = self.open.pop() else { return Ok(()) };
        self.ordpath.truncate(closed.label);
        if let Some(tokens) = closed.tokens.as_ref().filter(|_| closed.keyed) {
            self.path = [&[ELEMENT][..], tokens].concat();
            self.keyed_node(&closed.text, !closed.cut)?;
        }
        Ok(())
    }

    /// Gives the node of `self.path` and `self.ordpath` to the visit.
    fn keyed_node(&mut self, text: &[u8], whole: bool) -> Result<()> {
        self.keyed = self.keyed.saturating_add(1);
        if self.keyed > MAX_KEYED_NODES {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[DESIGN] the XML value is beyond turborel's limits: an XML index keys at most {MAX_KEYED_NODES} of its elements and attributes (RULES.md D18, D86)"
                ),
            ));
        }
        (self.visit)(&Keyed { path: &self.path, text, whole, ordpath: &self.ordpath })
    }

    /// Adds `text` to the string values of the elements open, from the
    /// innermost out, up to the first that is full.
    fn add_text(&mut self, text: &[u8]) {
        for open in self.open.iter_mut().rev() {
            if open.cut {
                break;
            }
            let room = VALUE_PREFIX_BYTES.saturating_sub(open.text.len());
            let taken = text.get(..room).unwrap_or(text);
            open.text.extend_from_slice(taken);
            open.cut = taken.len() < text.len();
        }
    }
}

/// The ordinal of token `t`.
fn ordinal(t: u64) -> Vec<u8> {
    super::keys::ordinal_of(t)
}

// Unit tests: paths, values cut and whole, ORDPATHs and the bound
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
