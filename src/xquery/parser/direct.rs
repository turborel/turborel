//! The direct constructors [XQuery §3.7.1, §3.7.2] [XQuery Appendix A.1
//! \[95\] to \[108\]], read character by character as their whitespace is
//! explicit [Appendix A.2.4.2]: no comment is recognized in them, and the
//! characters of their content are text [Appendix A.1.3, grammar-note:
//! comments]. Each enclosed expression is read as tokens again.
//!
//! The static errors of namespace declaration attributes and of
//! attributes named alike are raised here where the names as written show
//! them [XQuery §3.7.1.1, §3.7.1.2]; attributes whose prefixes differ and
//! are bound to one namespace are told apart only once the static context
//! resolves them.

mod attributes;

use std::mem;

use super::Parser;
use crate::error::Result;
use crate::xquery::ast::{Constructor, DirContent, DirElement, DirPi, Expr, QName};
use crate::xquery::lexer::Sym;

/// [XQuery Appendix A.1 \[96\]] What closes an empty element's start tag.
const EMPTY_TAG_CLOSE: &str = "/>";
/// What closes a start or end tag.
const TAG_CLOSE: &str = ">";
/// What opens an end tag.
const END_TAG_OPEN: &str = "</";
/// What opens a nested element.
const TAG_OPEN: &str = "<";
/// [XQuery Appendix A.1 \[103\]] What opens a direct comment constructor.
const COMMENT_OPEN: &str = "<!--";
/// What closes one.
const COMMENT_CLOSE: &str = "-->";
/// What a comment's contents never hold [XQuery Appendix A.1 \[104\]].
const DOUBLE_HYPHEN: &str = "--";
/// [XQuery Appendix A.1 \[105\]] What opens a direct processing instruction
/// constructor.
const PI_OPEN: &str = "<?";
/// What closes one.
const PI_CLOSE: &str = "?>";
/// [XQuery Appendix A.1 \[107\]] What opens a CDATA section.
const CDATA_OPEN: &str = "<![CDATA[";
/// What closes one.
const CDATA_CLOSE: &str = "]]>";
/// [XQuery Appendix A.1 \[102\]] What stands for a `{` in content.
const OPEN_BRACE_ESCAPE: &str = "{{";
/// What stands for a `}`.
const CLOSE_BRACE_ESCAPE: &str = "}}";

/// Which direct constructor a token begins [XQuery Appendix A.1 \[95\]].
enum Direct {
    /// `<`.
    Element,
    /// `<!--`.
    Comment,
    /// `<?`.
    Pi,
}

/// A `CommonContent` [XQuery Appendix A.1 \[102\]] read.
pub(super) enum Common {
    /// `{{` or `}}`: the brace it stands for, a character as written.
    Brace(char),
    /// A `PredefinedEntityRef` or `CharRef`: the character it stands for.
    Reference(char),
    /// An `EnclosedExpr` \[29\].
    Enclosed(Expr),
}

impl Parser {
    /// `DirectConstructor` [XQuery Appendix A.1 \[95\]], which `<`, `<!--`
    /// or `<?` begins in an operand's place.
    pub(super) fn direct_constructor(&mut self) -> Result<Expr> {
        let (at, end) = self.peek_token(0).map_or((0, 0), |t| (t.at, t.end));
        let direct = if self.is_sym(0, Sym::CommentStart) {
            Direct::Comment
        } else if self.is_sym(0, Sym::PiStart) {
            Direct::Pi
        } else {
            Direct::Element
        };
        self.reread_from(end);
        let constructor = match direct {
            Direct::Element => Constructor::Element(self.dir_element(at)?),
            Direct::Comment => Constructor::Comment(self.dir_comment(at)?),
            Direct::Pi => Constructor::Pi(self.dir_pi(at)?),
        };
        Ok(Expr::Constructor(Box::new(constructor)))
    }

    /// `DirElemConstructor` [XQuery Appendix A.1 \[96\]], after its `<` at
    /// `start`, one level deeper (`MAX_NESTING`).
    fn dir_element(&mut self, start: usize) -> Result<DirElement> {
        self.nested(|p| {
            let name = p.lexer.qname().ok_or_else(|| {
                p.lexer.error(p.lexer.position(), "a direct element constructor's '<' is followed by its QName [XQuery Appendix A.1 [96]]")
            })?;
            let attributes = p.dir_attribute_list()?;
            if p.lexer.eat(EMPTY_TAG_CLOSE) {
                return Ok(DirElement { name, attributes, content: Vec::new() });
            }
            if !p.lexer.eat(TAG_CLOSE) {
                return Err(p.lexer.error(p.lexer.position(), "a start tag closes by '>' or '/>' [XQuery Appendix A.1 [96]]"));
            }
            let content = p.dir_elem_content(&name, start)?;
            Ok(DirElement { name, attributes, content })
        })
    }

    /// The content of the element named `name` begun at `start` [XQuery
    /// Appendix A.1 \[101\]], to its end tag, whose name is the start
    /// tag's as written [XQuery §3.7.1].
    fn dir_elem_content(&mut self, name: &QName, start: usize) -> Result<Vec<DirContent>> {
        let mut content = Vec::new();
        let mut chars = String::new();
        loop {
            if self.lexer.eat(END_TAG_OPEN) {
                flush(&mut chars, &mut content);
                self.end_tag(name)?;
                return Ok(content);
            }
            if let Some(part) = self.content_part(&mut chars, start)? {
                flush(&mut chars, &mut content);
                content.push(part);
            }
        }
    }

    /// The end tag of `name`, after its `</` [XQuery Appendix A.1 \[96\]].
    fn end_tag(&mut self, name: &QName) -> Result<()> {
        let at = self.lexer.position();
        let end = self.lexer.qname();
        if end.as_ref() != Some(name) {
            let shown = end.map_or_else(|| "no name".to_owned(), |e| format!("'{e}'"));
            return Err(self.lexer.error(at, &format!("the end tag has {shown}, and the start tag '{name}' [XQuery §3.7.1]")));
        }
        self.lexer.space();
        if !self.lexer.eat(TAG_CLOSE) {
            return Err(self.lexer.error(self.lexer.position(), "an end tag closes by '>' [XQuery Appendix A.1 [96]]"));
        }
        Ok(())
    }

    /// The next part of an element's content [XQuery Appendix A.1 \[101\]]
    /// that is no character: none if it was a character, added to
    /// `chars`.
    fn content_part(&mut self, chars: &mut String, start: usize) -> Result<Option<DirContent>> {
        let at = self.lexer.position();
        if self.lexer.eat(COMMENT_OPEN) {
            return self.dir_comment(at).map(|text| Some(DirContent::Comment(text)));
        }
        if self.lexer.eat(CDATA_OPEN) {
            return self.cdata_section(at).map(|text| Some(DirContent::CData(text)));
        }
        if self.lexer.eat(PI_OPEN) {
            return self.dir_pi(at).map(|pi| Some(DirContent::Pi(pi)));
        }
        if self.lexer.eat(TAG_OPEN) {
            return self.dir_element(at).map(|element| Some(DirContent::Element(element)));
        }
        match self.common_content()? {
            Some(Common::Brace(c)) => chars.push(c),
            Some(Common::Reference(c)) => return Ok(Some(DirContent::Reference(c))),
            Some(Common::Enclosed(expr)) => return Ok(Some(DirContent::Enclosed(expr))),
            None => match self.lexer.bump() {
                Some(c) => chars.push(c),
                None => return Err(self.lexer.error(start, "a direct element constructor has no end tag [XQuery Appendix A.1 [96]]")),
            },
        }
        Ok(None)
    }

    /// `CommonContent` [XQuery Appendix A.1 \[102\]], if a `{`, `}` or `&`
    /// is next. A `}` alone is a syntax error [XQuery §3.7.1].
    pub(super) fn common_content(&mut self) -> Result<Option<Common>> {
        if self.lexer.eat(OPEN_BRACE_ESCAPE) {
            return Ok(Some(Common::Brace('{')));
        }
        if self.lexer.eat(CLOSE_BRACE_ESCAPE) {
            return Ok(Some(Common::Brace('}')));
        }
        match self.lexer.peek_char() {
            Some('{') => self.raw_enclosed().map(|expr| Some(Common::Enclosed(expr))),
            Some('}') => Err(self.lexer.error(self.lexer.position(), "a '}' in content is written '}}' [XQuery §3.7.1]")),
            Some('&') => self.lexer.reference().map(|c| Some(Common::Reference(c))),
            Some(_) | None => Ok(None),
        }
    }

    /// An `EnclosedExpr` [XQuery Appendix A.1 \[29\]] in a constructor's
    /// content or attribute value, its `{` next: read as tokens, then
    /// characters again after its `}`.
    fn raw_enclosed(&mut self) -> Result<Expr> {
        self.lexer.bump();
        self.ahead.clear();
        let inner = self.expr()?;
        let close = self.expect_sym(Sym::RBrace, "an enclosed expression closes by '}' [XQuery Appendix A.1 [29]]")?;
        self.reread_from(close.end);
        Ok(inner)
    }

    /// `DirCommentContents` [XQuery Appendix A.1 \[104\]], after the `<!--`
    /// at `start`, to its `-->`: no two hyphens together, and no hyphen
    /// last [XQuery §3.7.2].
    fn dir_comment(&mut self, start: usize) -> Result<String> {
        let mut text = String::new();
        loop {
            if self.lexer.eat(COMMENT_CLOSE) {
                return Ok(text);
            }
            if self.lexer.looking_at(DOUBLE_HYPHEN) {
                return Err(self.lexer.error(
                    self.lexer.position(),
                    "a direct comment constructor's contents have no '--' and do not end with '-' [XQuery Appendix A.1 [104]]",
                ));
            }
            text.push(self.lexer.bump().ok_or_else(|| {
                self.lexer.error(start, "a direct comment constructor is not closed by '-->' [XQuery Appendix A.1 [103]]")
            })?);
        }
    }

    /// `DirPIConstructor` [XQuery Appendix A.1 \[105\]], after the `<?` at
    /// `start`: its `PITarget` \[152\], an `NCName` other than `xml` in any
    /// case [XML §2.6 \[17\]] [Namespaces 1.0 §6] [Namespaces 1.1 §7], then whitespace and its
    /// contents if it has any.
    fn dir_pi(&mut self, start: usize) -> Result<DirPi> {
        let at = self.lexer.position();
        let target = self.lexer.ncname().ok_or_else(|| {
            self.lexer.error(at, "a direct processing instruction constructor's '<?' is followed by its target [XQuery Appendix A.1 [105]]")
        })?;
        if target.eq_ignore_ascii_case(crate::xml::XML_PREFIX) {
            return Err(self.lexer.error(at, "a processing instruction's target is not 'xml' in any case [XQuery §3.7.2] [XML §2.6 [17]]"));
        }
        if self.lexer.eat(PI_CLOSE) {
            return Ok(DirPi { target, contents: String::new() });
        }
        if !self.lexer.space() {
            return Err(self.lexer.error(
                self.lexer.position(),
                "a processing instruction's target is followed by whitespace or '?>' [XQuery Appendix A.1 [105]]",
            ));
        }
        let contents = self.raw_until(
            PI_CLOSE,
            start,
            "a direct processing instruction constructor is not closed by '?>' [XQuery Appendix A.1 [105]]",
        )?;
        Ok(DirPi { target, contents })
    }

    /// `CDataSectionContents` [XQuery Appendix A.1 \[108\]], after the
    /// `<![CDATA[` at `start`, to its `]]>`.
    fn cdata_section(&mut self, start: usize) -> Result<String> {
        self.raw_until(CDATA_CLOSE, start, "a CDATA section is not closed by ']]>' [XQuery Appendix A.1 [107]]")
    }

    /// The characters to the first `close`, which is read past; `why` the
    /// error if the text ends first, at `start`.
    fn raw_until(&mut self, close: &str, start: usize, why: &str) -> Result<String> {
        let mut text = String::new();
        while !self.lexer.eat(close) {
            text.push(self.lexer.bump().ok_or_else(|| self.lexer.error(start, why))?);
        }
        Ok(text)
    }
}

/// Adds the characters `chars` holds to `content`, if any, as one part.
fn flush(chars: &mut String, content: &mut Vec<DirContent>) {
    if !chars.is_empty() {
        content.push(DirContent::Chars(mem::take(chars)));
    }
}

#[cfg(test)]
mod tests;
