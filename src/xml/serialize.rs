//! An XML value serialized as text by the XML output method
//! [ISO/IEC 9075-14:2006 draft §10.15 GR11] [Serialization §5], after
//! sequence normalization [Serialization §2]: atomic values as their
//! lexical forms, adjacent ones a space apart, document nodes as their
//! children, and no attribute alone.
//!
//! \[GR10\] implementation-dependent: no CDATA section, no document type
//! declaration, no URI escaping, no content type, no Unicode
//! normalization and no character map. \[GR5\] Nothing is indented. Each
//! character the text could not hold, or a parse would change, is a
//! character reference [Serialization §5]; a name or a character the
//! version cannot hold is an error [Serialization §5.1.1].
//!
//! \[DESIGN\] RULES.md D106: each tree is walked by its links, so nothing
//! recurses; the namespace declarations written are kept on a stack.

use std::fmt::Write;

use super::chars::{COLON, Version, is_char, is_literal, is_name};
use super::sequence::{Item, Node, Sequence};
use super::tree::{Kind, NodeId, QName, Tree, XML_PREFIX};
use crate::error::{Code, Result, SqlError, SqlState};

/// How a value is serialized: the parameters of [Serialization §3] that
/// SQL/XML sets [§10.15 GR4 to GR9].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameters {
    /// The version of XML written [Serialization §5.1.1].
    pub version: Version,
    /// Whether an XML declaration is written, `omit-xml-declaration` no
    /// [Serialization §5.1.5].
    pub declaration: bool,
    /// The encoding the declaration names [Serialization §5.1.2].
    pub encoding: &'static str,
    /// Whether a prefix a namespace declaration undeclares is written as
    /// undeclared, `undeclare-prefixes` [Serialization §5.1.7], which XML
    /// 1.0 cannot write.
    pub undeclare: bool,
}

/// The text `value` serializes to by `parameters`.
///
/// # Errors
///
/// SQLSTATE 2200W, XQuery serialization error [§10.15 GR11a i], if it
/// holds an attribute alone [Serialization §2, err:SENR0001], or a name
/// or character the version cannot hold [Serialization §5.1.1,
/// err:SERE0005, err:SERE0006], or a comment or processing instruction
/// no text can hold [Serialization §5, err:SERE0003].
pub fn serialize(value: &Sequence, parameters: &Parameters) -> Result<String> {
    let mut writer = Writer { out: String::new(), parameters: *parameters, scope: Vec::new(), marks: Vec::new() };
    if parameters.declaration {
        let _ = write!(writer.out, "<?xml version=\"{}\" encoding=\"{}\"?>", parameters.version.number(), parameters.encoding);
    }
    let mut after_atomic = false;
    for item in value.items() {
        match item {
            Item::Atomic(atomic) => {
                if after_atomic {
                    writer.out.push(' ');
                }
                writer.text(&atomic.lexical)?;
                after_atomic = true;
            }
            Item::Node(node) => {
                writer.top(node)?;
                after_atomic = false;
            }
        }
    }
    Ok(writer.out)
}

/// The error of a value that cannot be serialized, `why`, by the rule of
/// [Serialization] `rule`.
fn unserializable(why: &str, rule: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §10.15 GR11a i] the XML value cannot be serialized: {why} {rule}"))
        .with_state(SqlState::XQUERY_SERIALIZATION_ERROR)
}

/// A namespace binding written: its prefix, none for the default
/// namespace, and its URI, empty to undeclare it.
type Binding = (Option<Box<str>>, Box<str>);

/// The text being written.
struct Writer {
    out: String,
    parameters: Parameters,
    /// The namespace bindings written and in scope, each prefix's last.
    scope: Vec<Binding>,
    /// Where each open element's bindings begin in `scope`.
    marks: Vec<usize>,
}

impl Writer {
    /// A node of the value: a document node's children, or the node; no
    /// attribute [Serialization §2].
    fn top(&mut self, node: &Node) -> Result<()> {
        let tree = node.tree();
        match node.kind() {
            Some(Kind::Document) => {
                for child in tree.children(node.id()) {
                    self.subtree(tree, child)?;
                }
                Ok(())
            }
            Some(Kind::Attribute) | None => {
                Err(unserializable("it holds an attribute that is no element's", "[Serialization §2] err:SENR0001"))
            }
            Some(Kind::Element | Kind::Text | Kind::Comment | Kind::ProcessingInstruction) => self.subtree(tree, node.id()),
        }
    }

    /// The node `root` of `tree` and every node below it, in document
    /// order, its element's ancestors' bindings declared on it.
    fn subtree(&mut self, tree: &Tree, root: NodeId) -> Result<()> {
        let mut at = Some(root);
        while let Some(node) = at {
            let opened = self.open(tree, node, node == root)?;
            at = match tree.first_child(node).filter(|_| opened) {
                Some(child) => Some(child),
                None => self.after(tree, root, node),
            };
        }
        Ok(())
    }

    /// The node after `node` in document order below `root`, each element
    /// left on the way closed; none at the end of `root`.
    fn after(&mut self, tree: &Tree, root: NodeId, node: NodeId) -> Option<NodeId> {
        let mut at = node;
        while at != root {
            if let Some(sibling) = tree.next_sibling(at) {
                return Some(sibling);
            }
            let parent = tree.parent(at)?;
            self.close(tree, parent);
            at = parent;
        }
        None
    }

    /// The node `id` written, an element's start tag if it has children,
    /// with every binding in scope if `first`; whether it was opened, so
    /// its children follow and its end tag after them.
    fn open(&mut self, tree: &Tree, id: NodeId, first: bool) -> Result<bool> {
        match tree.kind(id) {
            Some(Kind::Element) => self.start_tag(tree, id, first),
            Some(Kind::Text) => self.text(tree.content(id)).map(|()| false),
            Some(Kind::Comment) => self.comment(tree.content(id)).map(|()| false),
            Some(Kind::ProcessingInstruction) => {
                let target = tree.name(id).map(|n| &*n.local).unwrap_or_default();
                self.processing_instruction(target, tree.content(id)).map(|()| false)
            }
            Some(Kind::Document | Kind::Attribute) | None => Ok(false),
        }
    }

    /// An element's start tag, or its empty-element tag if it has no
    /// children [XML §3.1 [40], [44]]; whether it was opened.
    fn start_tag(&mut self, tree: &Tree, id: NodeId, first: bool) -> Result<bool> {
        let name = tree.name(id).ok_or_else(|| unserializable("an element has no name", "[Serialization §5] err:SERE0003"))?;
        self.marks.push(self.scope.len());
        let declarations = self.declarations(tree, id, first, name)?;
        self.out.push('<');
        self.name(name)?;
        for (prefix, uri) in declarations {
            self.out.push_str(" xmlns");
            if let Some(p) = prefix {
                self.out.push(COLON);
                self.out.push_str(&p);
            }
            self.out.push_str("=\"");
            self.attribute_value(&uri)?;
            self.out.push('"');
        }
        for attribute in tree.attributes(id) {
            let attribute_name =
                tree.name(attribute).ok_or_else(|| unserializable("an attribute has no name", "[Serialization §5] err:SERE0003"))?;
            self.out.push(' ');
            self.name(attribute_name)?;
            self.out.push_str("=\"");
            self.attribute_value(tree.content(attribute))?;
            self.out.push('"');
        }
        if tree.first_child(id).is_some() {
            self.out.push('>');
            Ok(true)
        } else {
            self.out.push_str("/>");
            self.close_scope();
            Ok(false)
        }
    }

    /// The namespace declarations element `id`, of name `name`, writes,
    /// brought into scope: those it makes, or, if `first`, every binding
    /// in its scope, less those already written; then one for each prefix
    /// of its name or its attributes' names not bound to its URI, so a
    /// parse of the text names them as the tree does [Serialization §5].
    fn declarations(&mut self, tree: &Tree, id: NodeId, first: bool, name: &QName) -> Result<Vec<Binding>> {
        let made: Vec<Binding> = if first {
            tree.in_scope(id).into_iter().filter(|(p, _)| *p != Some(XML_PREFIX)).map(|(p, u)| (p.map(Box::from), Box::from(u))).collect()
        } else {
            tree.declared(id).iter().map(|b| (b.prefix.clone(), b.uri.clone())).collect()
        };
        let mut written: Vec<Binding> = Vec::new();
        for (prefix, uri) in made {
            let undeclares_prefix = prefix.is_some() && uri.is_empty();
            if (undeclares_prefix && !self.parameters.undeclare) || self.bound(prefix.as_deref()) == &*uri {
                continue;
            }
            written.push((prefix, uri));
        }
        self.require(&mut written, name.prefix.as_deref(), name.uri.as_deref().unwrap_or_default())?;
        for attribute in tree.attributes(id).filter_map(|a| tree.name(a)) {
            match attribute.prefix.as_deref() {
                Some(prefix) => self.require(&mut written, Some(prefix), attribute.uri.as_deref().unwrap_or_default())?,
                None => {
                    if attribute.uri.is_some() {
                        return Err(unserializable("an attribute in a namespace has no prefix", "[Serialization §5] err:SERE0003"));
                    }
                }
            }
        }
        self.scope.extend(written.iter().cloned());
        Ok(written)
    }

    /// `prefix`, or the default namespace if none, bound to `uri` by
    /// `written` or, failing it, by what is written, else bound so by a
    /// declaration added to `written`; `xml` is bound already.
    fn require(&self, written: &mut Vec<Binding>, prefix: Option<&str>, uri: &str) -> Result<()> {
        if prefix == Some(XML_PREFIX) {
            return Ok(());
        }
        let declared = written.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, u)| u.to_string());
        let bound = declared.as_deref().unwrap_or_else(|| self.bound(prefix));
        if bound == uri {
            return Ok(());
        }
        if declared.is_some() {
            return Err(unserializable("one prefix names two namespaces on one element", "[Serialization §5] err:SERE0003"));
        }
        written.push((prefix.map(Box::from), uri.into()));
        Ok(())
    }

    /// The URI `prefix`, or the default namespace if none, is bound to in
    /// what is written: empty if none is, or it is undeclared.
    fn bound(&self, prefix: Option<&str>) -> &str {
        self.scope.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, u)| &**u).unwrap_or_default()
    }

    /// The end tag of element `id` [XML §3.1 [42]].
    fn close(&mut self, tree: &Tree, id: NodeId) {
        if let Some(name) = tree.name(id) {
            self.out.push_str("</");
            if let Some(prefix) = &name.prefix {
                self.out.push_str(prefix);
                self.out.push(COLON);
            }
            self.out.push_str(&name.local);
            self.out.push('>');
        }
        self.close_scope();
    }

    /// The last open element's bindings taken out of scope.
    fn close_scope(&mut self) {
        if let Some(mark) = self.marks.pop() {
            self.scope.truncate(mark);
        }
    }

    /// `name` written, each part a name of the version [Serialization
    /// §5.1.1, err:SERE0005].
    fn name(&mut self, name: &QName) -> Result<()> {
        let version = self.parameters.version;
        let parts = name.prefix.iter().map(|p| &**p).chain(std::iter::once(&*name.local));
        for part in parts {
            if !is_name(version, part) || part.contains(COLON) {
                return Err(unserializable(
                    &format!("'{part}' is no name of XML {}", version.number()),
                    "[Serialization §5.1.1] err:SERE0005",
                ));
            }
        }
        if let Some(prefix) = &name.prefix {
            self.out.push_str(prefix);
            self.out.push(COLON);
        }
        self.out.push_str(&name.local);
        Ok(())
    }

    /// The error unless `c` is a character of the version [Serialization
    /// §5.1.1, err:SERE0006].
    fn check(&self, c: char) -> Result<()> {
        if is_char(self.parameters.version, c) {
            return Ok(());
        }
        let version = self.parameters.version.number();
        Err(unserializable(&format!("the character #x{:X} is none of XML {version}", u32::from(c)), "[Serialization §5.1.1] err:SERE0006"))
    }

    /// Text written, each character that is markup, a line end a parse
    /// changes, or a control character, a reference [Serialization §5].
    fn text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            self.check(c)?;
            match c {
                '&' => self.out.push_str("&amp;"),
                '<' => self.out.push_str("&lt;"),
                '>' => self.out.push_str("&gt;"),
                '\n' | '\t' => self.out.push(c),
                other => self.character(other),
            }
        }
        Ok(())
    }

    /// An attribute's value written, as text is, and its quote, line feed
    /// and tab as references, which a parse would normalize to spaces
    /// [Serialization §5] [XML §3.3.3].
    fn attribute_value(&mut self, value: &str) -> Result<()> {
        for c in value.chars() {
            self.check(c)?;
            match c {
                '&' => self.out.push_str("&amp;"),
                '<' => self.out.push_str("&lt;"),
                '>' => self.out.push_str("&gt;"),
                '"' => self.out.push_str("&quot;"),
                other => self.character(other),
            }
        }
        Ok(())
    }

    /// `c` written as it is, or as a character reference if it is a
    /// control character, a carriage return, NEL or the line separator
    /// [Serialization §5].
    fn character(&mut self, c: char) {
        if c.is_control() || c == '\u{2028}' {
            let _ = write!(self.out, "&#x{:X};", u32::from(c));
        } else {
            self.out.push(c);
        }
    }

    /// A comment written [XML §2.5 [15]]: its content holds no `--`, ends
    /// with no `-`, and holds only characters the version writes.
    fn comment(&mut self, content: &str) -> Result<()> {
        self.literal(content)?;
        if content.contains("--") || content.ends_with('-') {
            return Err(unserializable("a comment holds '--' or ends with '-'", "[Serialization §5] err:SERE0003"));
        }
        self.out.push_str("<!--");
        self.out.push_str(content);
        self.out.push_str("-->");
        Ok(())
    }

    /// A processing instruction written [XML §2.6 [16]]: its target a
    /// name, its content holding no `?>` and only characters the version
    /// writes.
    fn processing_instruction(&mut self, target: &str, content: &str) -> Result<()> {
        self.literal(content)?;
        if content.contains("?>") || !is_name(self.parameters.version, target) || target.contains(COLON) {
            return Err(unserializable("a processing instruction has no target or holds '?>'", "[Serialization §5] err:SERE0003"));
        }
        self.out.push_str("<?");
        self.out.push_str(target);
        if !content.is_empty() {
            self.out.push(' ');
            self.out.push_str(content);
        }
        self.out.push_str("?>");
        Ok(())
    }

    /// The error unless every character of `content`, which no reference
    /// can write, may be written as it is in the version [Serialization
    /// §5.1.1, err:SERE0006].
    fn literal(&self, content: &str) -> Result<()> {
        let version = self.parameters.version;
        match content.chars().find(|&c| !is_literal(version, c)) {
            Some(c) => Err(unserializable(
                &format!(
                    "a comment or processing instruction holds #x{:X}, which XML {} writes only as a reference",
                    u32::from(c),
                    version.number()
                ),
                "[Serialization §5.1.1] err:SERE0006",
            )),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
