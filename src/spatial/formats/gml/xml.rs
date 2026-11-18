//! A minimal XML reader for GML, ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.60: elements with attributes and text, the five predefined
//! entities and character references, comments, processing
//! instructions, a document type declaration and CDATA sections.
//! Namespace prefixes are dropped, so `gml:Point` is `Point`.

use crate::error::{Result, SqlError};
use crate::spatial::curves::fault::{Fault, fault};

/// An element: its local name, attributes, child elements and text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// The name without its namespace prefix.
    pub name: String,
    /// The attributes, by local name, in document order.
    pub attrs: Vec<(String, String)>,
    /// The child elements.
    pub children: Vec<Node>,
    /// The character data directly inside, joined.
    pub text: String,
}

impl Node {
    /// The value of attribute `name`, by local name, any case.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The children named `name`.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// The clause GML's rules sit in.
pub const CLAUSE: &str = "§5.1.60";

/// A 2FF24 error for `detail`.
pub fn invalid(detail: &str) -> SqlError {
    fault(Fault::InvalidGml, CLAUSE, detail)
}

/// The deepest nesting read, against runaway documents.
const MAX_DEPTH: usize = 256;

/// The document element of `text`.
///
/// # Errors
///
/// SQLSTATE 2FF24 for text that is not well formed or has no single
/// document element.
pub fn parse(text: &str) -> Result<Node> {
    let mut p = Parser { text, at: 0 };
    p.misc()?;
    let root = p.element(0)?;
    p.misc()?;
    if p.at < text.len() {
        return Err(invalid("content follows the document element"));
    }
    Ok(root)
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        self.text.get(self.at..).unwrap_or_default()
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        self.at = self.at.saturating_add(rest.len().saturating_sub(rest.trim_start().len()));
    }

    /// Consumes through `end`, the text before it returned.
    fn through(&mut self, end: &str) -> Result<&str> {
        let rest = self.text.get(self.at..).unwrap_or_default();
        let found = rest.find(end).ok_or_else(|| invalid(&format!("'{end}' is missing")))?;
        self.at = self.at.saturating_add(found).saturating_add(end.len());
        Ok(rest.get(..found).unwrap_or_default())
    }

    /// Skips space, comments, processing instructions and a document
    /// type declaration.
    fn misc(&mut self) -> Result<()> {
        loop {
            self.skip_space();
            let rest = self.rest();
            let end = if rest.starts_with("<?") {
                "?>"
            } else if rest.starts_with("<!--") {
                "-->"
            } else if rest.starts_with("<!DOCTYPE") {
                ">"
            } else {
                return Ok(());
            };
            self.through(end)?;
        }
    }

    fn name(&mut self) -> Result<String> {
        let rest = self.rest();
        let length = rest.find(|c: char| c.is_whitespace() || matches!(c, '>' | '/' | '=')).unwrap_or(rest.len());
        if length == 0 {
            return Err(invalid("a name is missing"));
        }
        let name = rest.get(..length).unwrap_or_default().to_owned();
        self.at = self.at.saturating_add(length);
        Ok(name)
    }

    fn element(&mut self, depth: usize) -> Result<Node> {
        if depth > MAX_DEPTH || !self.rest().starts_with('<') {
            return Err(invalid("an element is expected"));
        }
        self.at = self.at.saturating_add(1);
        let qualified = self.name()?;
        let mut node = Node { name: local(&qualified).to_owned(), ..Node::default() };
        loop {
            self.skip_space();
            if self.rest().starts_with("/>") {
                self.at = self.at.saturating_add(2);
                return Ok(node);
            }
            if self.rest().starts_with('>') {
                self.at = self.at.saturating_add(1);
                break;
            }
            node.attrs.push(self.attribute()?);
        }
        self.content(&mut node, depth)?;
        let closing = self.name()?;
        if closing != qualified {
            return Err(invalid(&format!("<{qualified}> is closed by </{closing}>")));
        }
        self.skip_space();
        self.through(">")?;
        Ok(node)
    }

    fn attribute(&mut self) -> Result<(String, String)> {
        let name = self.name()?;
        self.skip_space();
        if !self.rest().starts_with('=') {
            return Err(invalid(&format!("attribute {name} has no value")));
        }
        self.at = self.at.saturating_add(1);
        self.skip_space();
        let quote =
            self.rest().chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| invalid("an attribute value is not quoted"))?;
        self.at = self.at.saturating_add(1);
        let raw = self.through(if quote == '"' { "\"" } else { "'" })?.to_owned();
        Ok((local(&name).to_owned(), unescape(&raw)?))
    }

    /// Reads children and text up to the closing tag's name.
    fn content(&mut self, node: &mut Node, depth: usize) -> Result<()> {
        loop {
            let rest = self.rest();
            if rest.starts_with("</") {
                self.at = self.at.saturating_add(2);
                return Ok(());
            }
            if rest.starts_with("<!--") {
                self.through("-->")?;
            } else if rest.starts_with("<![CDATA[") {
                self.at = self.at.saturating_add("<![CDATA[".len());
                let data = self.through("]]>")?.to_owned();
                node.text.push_str(&data);
            } else if rest.starts_with("<?") {
                self.through("?>")?;
            } else if rest.starts_with('<') {
                node.children.push(self.element(depth.saturating_add(1))?);
            } else if rest.is_empty() {
                return Err(invalid(&format!("<{}> is not closed", node.name)));
            } else {
                let length = rest.find('<').unwrap_or(rest.len());
                let raw = rest.get(..length).unwrap_or_default().to_owned();
                node.text.push_str(&unescape(&raw)?);
                self.at = self.at.saturating_add(length);
            }
        }
    }
}

/// The name after its namespace prefix.
fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, after)| after)
}

/// `raw` with its entity and character references replaced.
fn unescape(raw: &str) -> Result<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('&') {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start.saturating_add(1)..).unwrap_or_default();
        let end = after.find(';').ok_or_else(|| invalid("an entity reference has no ';'"))?;
        let entity = after.get(..end).unwrap_or_default();
        out.push(reference(entity).ok_or_else(|| invalid(&format!("unknown entity &{entity};")))?);
        rest = after.get(end.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

fn reference(entity: &str) -> Option<char> {
    match entity {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = entity.strip_prefix('#')?;
            let code = match number.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// `text` with `<`, `>`, `&`, `"` and `'` escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60] Elements,
    /// attributes, text, references, comments and CDATA are read, and
    /// prefixes dropped.
    #[test]
    fn reads_documents() {
        let doc = "<?xml version='1.0'?><!-- c --><gml:Point xmlns:gml=\"http://www.opengis.net/gml/3.2\" srsName='A&amp;B'>\n <gml:pos>1 &#50;<![CDATA[ 3]]><!-- x --></gml:pos><e/></gml:Point> ";
        let root = parse(doc).unwrap();
        assert_eq!(root.name, "Point");
        assert_eq!(root.attr("srsname"), Some("A&B"));
        assert_eq!(root.child("pos").map(|p| p.text.as_str()), Some("1 2 3"));
        assert_eq!(root.all("e").count(), 1);
        assert_eq!(escape("<a&'\">"), "&lt;a&amp;&apos;&quot;&gt;");
        assert_eq!(unescape("&#x41;&gt;").unwrap(), "A>");
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60] Text that
    /// is not well formed raises 2FF24.
    #[test]
    fn rejects_malformed() {
        for doc in ["", "<a>", "<a></b>", "<a x=1/>", "<a x/>", "<a>&nope;</a>", "<a>&amp</a>", "<a/><b/>", "text", "<a><!-- </a>", "< a/>"]
        {
            assert_eq!(parse(doc).unwrap_err().sqlstate().as_str(), "2FF24", "{doc}");
        }
        let deep = "<a>".repeat(300) + &"</a>".repeat(300);
        assert!(parse(&deep).is_err());
    }
}
