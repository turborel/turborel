//! Constructors written back as text [XQuery Appendix A.1 \[94\] to
//! \[116\]]: a direct constructor's characters as they were, but a brace
//! doubled [XQuery §3.7.1] and a quotation mark of an attribute value
//! doubled [Appendix A.1 \[146\]], and what a reference stood for as a
//! reference again. As in `display/expr.rs`, what nests is written by
//! its `fmt` (RULES.md R-CF-6).

use std::fmt::{self, Display, Formatter};

use super::expr::{braced, keyword_braced};
use super::{reference, reference_or_char};
use crate::xquery::ast::{AttributeValuePart, ComputedName, Constructor, DirContent, DirElement, DirPi, Expr, PiName};
use crate::xquery::keywords::{ATTRIBUTE, COMMENT, DOCUMENT, ELEMENT, PROCESSING_INSTRUCTION, TEXT};

/// [XQuery Appendix A.2.1 \[148\] to \[150\]] The characters content and
/// attribute values write only as references.
const MARKUP: [char; 2] = ['<', '&'];

impl Display for Constructor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Constructor::Element(element) => element.fmt(f),
            Constructor::Comment(text) => write!(f, "<!--{text}-->"),
            Constructor::Pi(pi) => pi.fmt(f),
            Constructor::Document(content) => keyword_braced(f, DOCUMENT, content),
            Constructor::ComputedElement(name, content) => computed(f, ELEMENT, name, content.as_ref()),
            Constructor::ComputedAttribute(name, content) => computed(f, ATTRIBUTE, name, content.as_ref()),
            Constructor::Text(content) => keyword_braced(f, TEXT, content),
            Constructor::ComputedComment(content) => keyword_braced(f, COMMENT, content),
            Constructor::ComputedPi(target, content) => {
                f.write_str(PROCESSING_INSTRUCTION)?;
                f.write_str(" ")?;
                match target {
                    PiName::Name(name) => f.write_str(name)?,
                    PiName::Expr(expr) => name_expr(f, expr)?,
                }
                optionally_braced(f, content.as_ref())
            }
        }
    }
}

/// Writes a computed element or attribute constructor, `keyword` its
/// keyword [XQuery Appendix A.1 \[111\], \[113\]].
fn computed(f: &mut Formatter<'_>, keyword: &str, name: &ComputedName, content: Option<&Expr>) -> fmt::Result {
    f.write_str(keyword)?;
    f.write_str(" ")?;
    match name {
        ComputedName::Name(name) => name.fmt(f)?,
        ComputedName::Expr(expr) => name_expr(f, expr)?,
    }
    optionally_braced(f, content)
}

/// Writes a name expression, `{ expr }`.
fn name_expr(f: &mut Formatter<'_>, expr: &Expr) -> fmt::Result {
    f.write_str("{ ")?;
    expr.fmt(f)?;
    f.write_str(" }")
}

/// Writes ` { content }`, or ` { }` for none.
fn optionally_braced(f: &mut Formatter<'_>, content: Option<&Expr>) -> fmt::Result {
    match content {
        Some(content) => braced(f, content),
        None => f.write_str(" { }"),
    }
}

/// Writes an enclosed expression of a direct constructor, `{expr}`
/// [XQuery Appendix A.1 \[29\]].
fn enclosed(f: &mut Formatter<'_>, expr: &Expr) -> fmt::Result {
    f.write_str("{")?;
    expr.fmt(f)?;
    f.write_str("}")
}

impl Display for DirPi {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.contents.is_empty() { write!(f, "<?{}?>", self.target) } else { write!(f, "<?{} {}?>", self.target, self.contents) }
    }
}

impl Display for DirElement {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("<")?;
        self.name.fmt(f)?;
        for attribute in &self.attributes {
            f.write_str(" ")?;
            attribute.name.fmt(f)?;
            f.write_str("=\"")?;
            for part in &attribute.value {
                match part {
                    AttributeValuePart::Chars(text) => chars(f, text, Some('"'))?,
                    AttributeValuePart::Reference(c) => reference(f, *c)?,
                    AttributeValuePart::Enclosed(expr) => enclosed(f, expr)?,
                }
            }
            f.write_str("\"")?;
        }
        if self.content.is_empty() {
            return f.write_str("/>");
        }
        f.write_str(">")?;
        for part in &self.content {
            part.fmt(f)?;
        }
        f.write_str("</")?;
        self.name.fmt(f)?;
        f.write_str(">")
    }
}

impl Display for DirContent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DirContent::Chars(text) => chars(f, text, None),
            DirContent::Reference(c) => reference(f, *c),
            DirContent::CData(text) => write!(f, "<![CDATA[{text}]]>"),
            DirContent::Enclosed(expr) => enclosed(f, expr),
            DirContent::Element(element) => element.fmt(f),
            DirContent::Comment(text) => write!(f, "<!--{text}-->"),
            DirContent::Pi(pi) => pi.fmt(f),
        }
    }
}

/// Writes characters of content, or of an attribute value delimited by
/// `quote`: each brace and each `quote` doubled, markup as references.
fn chars(f: &mut Formatter<'_>, text: &str, quote: Option<char>) -> fmt::Result {
    for c in text.chars() {
        if c == '{' || c == '}' || Some(c) == quote {
            write!(f, "{c}{c}")?;
        } else {
            reference_or_char(f, c, &MARKUP)?;
        }
    }
    Ok(())
}
