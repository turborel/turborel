//! A tree built in document order [XQueryDM §6]: a document node, then
//! each element as its start tag is read, its attributes with it, then
//! its children, closed when its end tag is. Adjacent text is one text
//! node, and no text node is empty [XQueryDM §6.7.3].

use std::collections::HashMap;

use super::{Binding, Kind, NONE, Node, QName, Tree};
use crate::error::Result;
use crate::xml::{MAX_NESTING, MAX_NODES, MAX_TEXT, beyond};

/// An attribute of an element being started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Specified {
    /// Its name.
    pub name: QName,
    /// Its value, normalized [XML 1.0 §3.3.3].
    pub value: String,
    /// Whether it is an ID [XQueryDM §6.3.3].
    pub is_id: bool,
    /// Whether it is an IDREF or IDREFS [XQueryDM §6.3.3].
    pub is_idrefs: bool,
}

/// An element being started: its name, the namespace bindings it
/// declares and its attributes, none of them a namespace declaration
/// [XQueryDM §6.2.3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementStart {
    /// Its name.
    pub name: QName,
    /// The bindings it declares.
    pub bindings: Vec<Binding>,
    /// Its attributes.
    pub attributes: Vec<Specified>,
}

/// An open node: a document or element node whose children are being
/// read, and its last child so far.
#[derive(Debug, Clone, Copy)]
struct Open {
    node: u32,
    last_child: u32,
}

/// A tree being built.
#[derive(Debug)]
pub struct Builder {
    tree: Tree,
    open: Vec<Open>,
    interned: HashMap<QName, u32>,
    /// The octets of the names interned and the bindings added so far.
    names_text: usize,
    /// How many nodes are open before any element is: one, the document
    /// node, or none for a tree whose root has no parent.
    base: usize,
}

/// A count of a tree's tables as a link, or the error of a tree too big
/// to link.
fn link(count: usize) -> Result<u32> {
    u32::try_from(count).ok().filter(|&c| c != NONE).ok_or_else(|| beyond("it has too many nodes"))
}

impl Builder {
    /// A tree of a document node and nothing else yet [XQueryDM §6.1].
    #[must_use]
    pub fn document() -> Builder {
        let root = Node {
            kind: Kind::Document,
            name: NONE,
            parent: NONE,
            first_child: NONE,
            next_sibling: NONE,
            attributes: 0,
            bindings: 0..0,
            text: 0..0,
            is_id: false,
            is_idrefs: false,
        };
        Builder {
            tree: Tree { nodes: vec![root], ..Tree::default() },
            open: vec![Open { node: 0, last_child: NONE }],
            interned: HashMap::new(),
            names_text: 0,
            base: 1,
        }
    }

    /// A tree with no node yet, whose first node is its root and has no
    /// parent: an element, attribute, text, comment or processing
    /// instruction node standing alone [XQueryDM §6.2.1, §6.3.1].
    #[must_use]
    pub fn parentless() -> Builder {
        Builder { tree: Tree::default(), open: Vec::new(), interned: HashMap::new(), names_text: 0, base: 0 }
    }

    /// How many elements are open: how deep the node whose children are
    /// being read is below the root, or the document node.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.open.len().saturating_sub(self.base)
    }

    /// The index of `name` in the tree's names, kept there once.
    fn intern(&mut self, name: &QName) -> Result<u32> {
        if let Some(&at) = self.interned.get(name) {
            return Ok(at);
        }
        let at = link(self.tree.names.len())?;
        let octets = name.local.len().saturating_add(name.prefix.as_ref().map_or(0, |p| p.len()));
        self.text_grows(octets)?;
        self.names_text = self.names_text.saturating_add(octets);
        self.tree.names.push(name.clone());
        self.interned.insert(name.clone(), at);
        Ok(at)
    }

    /// The error if the tree's text grows by `more` octets past
    /// `MAX_TEXT`.
    fn text_grows(&self, more: usize) -> Result<()> {
        let total = self.tree.text.len().checked_add(self.names_text).and_then(|n| n.checked_add(more));
        if total.is_some_and(|t| t <= MAX_TEXT) { Ok(()) } else { Err(beyond("its text is longer than its limit")) }
    }

    /// The error if `more` nodes or bindings would make the tree hold more
    /// than `MAX_NODES` of them.
    fn room_for(&self, more: usize) -> Result<()> {
        let held = self.tree.nodes.len().saturating_add(self.tree.bindings.len());
        if held.saturating_add(more) > MAX_NODES {
            Err(beyond(&format!("it has more than {MAX_NODES} nodes and bindings")))
        } else {
            Ok(())
        }
    }

    /// `text` added to the tree's text, as a range of it.
    fn push_text(&mut self, text: &str) -> Result<std::ops::Range<u32>> {
        self.text_grows(text.len())?;
        let start = link(self.tree.text.len())?;
        self.tree.text.push_str(text);
        Ok(start..link(self.tree.text.len())?)
    }

    /// A node of `kind` added as the last child of the open node, or, for
    /// an attribute, after its element; its index.
    fn add(&mut self, kind: Kind, name: u32, text: std::ops::Range<u32>) -> Result<u32> {
        self.room_for(1)?;
        let at = link(self.tree.nodes.len())?;
        let open = self.open.last().copied();
        if open.is_none() && !self.tree.nodes.is_empty() {
            return Err(beyond("a tree has one root"));
        }
        let node = Node {
            kind,
            name,
            parent: open.map_or(NONE, |o| o.node),
            first_child: NONE,
            next_sibling: NONE,
            attributes: 0,
            bindings: 0..0,
            text,
            is_id: false,
            is_idrefs: false,
        };
        self.tree.nodes.push(node);
        if let Some(parent) = open.filter(|_| kind != Kind::Attribute) {
            self.link_child(parent, at);
        }
        Ok(at)
    }

    /// `child` linked as the last child of `open`: its first child if it
    /// has none, else its last child's next sibling.
    fn link_child(&mut self, open: Open, child: u32) {
        let first = open.last_child == NONE;
        let before = if first { open.node } else { open.last_child };
        if let Some(n) = self.tree.nodes.get_mut(super::wide(before)) {
            if first {
                n.first_child = child;
            } else {
                n.next_sibling = child;
            }
        }
        if let Some(o) = self.open.last_mut() {
            o.last_child = child;
        }
    }

    /// An element started as the last child of the open node, its
    /// attributes after it, and opened, so the nodes added next are its
    /// children.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if elements would nest deeper than `MAX_NESTING`, or
    /// the tree would be too big.
    pub fn start_element(&mut self, start: ElementStart) -> Result<()> {
        if self.depth() >= MAX_NESTING {
            return Err(beyond(&format!("its elements nest deeper than {MAX_NESTING}")));
        }
        let name = self.intern(&start.name)?;
        let element = self.add(Kind::Element, name, 0..0)?;
        self.room_for(start.bindings.len())?;
        let octets = start.bindings.iter().map(|b| b.uri.len().saturating_add(b.prefix.as_ref().map_or(0, |p| p.len()))).sum::<usize>();
        self.text_grows(octets)?;
        self.names_text = self.names_text.saturating_add(octets);
        let first = link(self.tree.bindings.len())?;
        self.tree.bindings.extend(start.bindings);
        let bindings = first..link(self.tree.bindings.len())?;
        let count = link(start.attributes.len())?;
        self.open.push(Open { node: element, last_child: NONE });
        for attribute in start.attributes {
            let attribute_name = self.intern(&attribute.name)?;
            let text = self.push_text(&attribute.value)?;
            let at = self.add(Kind::Attribute, attribute_name, text)?;
            if let Some(n) = self.tree.nodes.get_mut(super::wide(at)) {
                n.is_id = attribute.is_id;
                n.is_idrefs = attribute.is_idrefs;
            }
        }
        if let Some(n) = self.tree.nodes.get_mut(super::wide(element)) {
            n.attributes = count;
            n.bindings = bindings;
        }
        Ok(())
    }

    /// The open element closed, so the nodes added next follow it.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if no element is open, which no parse leads to.
    pub fn end_element(&mut self) -> Result<()> {
        if self.open.len() <= self.base {
            return Err(beyond("an element is closed that was not started"));
        }
        self.open.pop();
        Ok(())
    }

    /// `text` added to the open node's children: to its last child if
    /// that is a text node, so adjacent text is one node, and nothing if
    /// it is empty [XQueryDM §6.7.3].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the tree's text would pass `MAX_TEXT`.
    pub fn text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let last = self.open.last().map_or(NONE, |o| o.last_child);
        let extends =
            self.tree.nodes.get(super::wide(last)).is_some_and(|n| n.kind == Kind::Text && super::wide(n.text.end) == self.tree.text.len());
        if extends {
            let range = self.push_text(text)?;
            if let Some(n) = self.tree.nodes.get_mut(super::wide(last)) {
                n.text.end = range.end;
            }
            return Ok(());
        }
        let range = self.push_text(text)?;
        self.add(Kind::Text, NONE, range).map(|_| ())
    }

    /// A comment of `content` added to the open node's children [XQueryDM
    /// §6.6].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the tree's text would pass `MAX_TEXT`.
    pub fn comment(&mut self, content: &str) -> Result<()> {
        let range = self.push_text(content)?;
        self.add(Kind::Comment, NONE, range).map(|_| ())
    }

    /// A processing instruction of `target` and `content` added to the
    /// open node's children [XQueryDM §6.5].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the tree's text would pass `MAX_TEXT`.
    pub fn processing_instruction(&mut self, target: &str, content: &str) -> Result<()> {
        let name = self.intern(&QName::local(target))?;
        let range = self.push_text(content)?;
        self.add(Kind::ProcessingInstruction, name, range).map(|_| ())
    }

    /// An attribute standing alone, the root of a tree with no node yet
    /// [XQueryDM §6.3.1].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the tree has a node, which no attribute is added to
    /// but by `start_element`, or the tree's text would pass `MAX_TEXT`.
    pub fn attribute(&mut self, attribute: &Specified) -> Result<()> {
        if !self.open.is_empty() || !self.tree.nodes.is_empty() {
            return Err(beyond("an attribute stands alone or is its element's"));
        }
        let name = self.intern(&attribute.name)?;
        let text = self.push_text(&attribute.value)?;
        let at = self.add(Kind::Attribute, name, text)?;
        if let Some(n) = self.tree.nodes.get_mut(super::wide(at)) {
            n.is_id = attribute.is_id;
            n.is_idrefs = attribute.is_idrefs;
        }
        Ok(())
    }

    /// The tree built.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if an element is still open, which no parse leads to.
    pub fn finish(self) -> Result<Tree> {
        if self.open.len() != self.base || self.tree.nodes.is_empty() {
            return Err(beyond("an element was started and not closed, or no node was added"));
        }
        Ok(self.tree)
    }
}
