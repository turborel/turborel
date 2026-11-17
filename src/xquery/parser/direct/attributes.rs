//! The attributes of a direct element constructor [XQuery §3.7.1.1,
//! §3.7.1.2] [XQuery Appendix A.1 \[97\] to \[100\]], and the static
//! errors their names and values as written show.

use std::mem;

use super::{Common, Parser};
use crate::error::Result;
use crate::xml::{Version, XML_NAMESPACE, XML_PREFIX};
use crate::xquery::ast::{AttributeValuePart, DirAttribute, QName};
use crate::xquery::error::{ErrorCode, xquery_error};

/// [XQuery §3.7.1.2] The name of a namespace declaration attribute that
/// declares the default element/type namespace, and the prefix of one
/// that declares a prefix.
const XMLNS: &str = "xmlns";
/// [XQuery Appendix A.1 \[97\]] What is between an attribute's name and
/// its value.
const EQUALS: char = '=';

impl Parser {
    /// `DirAttributeList` [XQuery Appendix A.1 \[97\]], to the `>` or `/>`
    /// after it, which is not read: each attribute after whitespace.
    pub(super) fn dir_attribute_list(&mut self) -> Result<Vec<DirAttribute>> {
        let mut attributes: Vec<DirAttribute> = Vec::new();
        loop {
            let spaced = self.lexer.space();
            let at = self.lexer.position();
            if self.lexer.looking_at(super::EMPTY_TAG_CLOSE) || self.lexer.looking_at(super::TAG_CLOSE) {
                return Ok(attributes);
            }
            let name = self.lexer.qname().filter(|_| spaced).ok_or_else(|| {
                self.lexer.error(
                    at,
                    "a start tag holds its name, then attributes each after whitespace, then '>' or '/>' [XQuery Appendix A.1 [96], [97]]",
                )
            })?;
            self.lexer.space();
            if !self.lexer.eat_char(EQUALS) {
                return Err(self.lexer.error(self.lexer.position(), "an attribute's name is followed by '=' [XQuery Appendix A.1 [97]]"));
            }
            self.lexer.space();
            let value = self.dir_attribute_value()?;
            let attribute = DirAttribute { name, value };
            self.check_attribute(&attributes, &attribute, at)?;
            attributes.push(attribute);
        }
    }

    /// `DirAttributeValue` [XQuery Appendix A.1 \[98\]]: its parts between
    /// its quotes, the quote doubled standing for one.
    fn dir_attribute_value(&mut self) -> Result<Vec<AttributeValuePart>> {
        let start = self.lexer.position();
        let quote = self
            .lexer
            .peek_char()
            .filter(|&c| c == '"' || c == '\'')
            .ok_or_else(|| self.lexer.error(start, "an attribute's value is in quotes [XQuery Appendix A.1 [98]]"))?;
        self.lexer.bump();
        let mut parts = Vec::new();
        let mut chars = String::new();
        loop {
            if self.lexer.eat_char(quote) {
                if !self.lexer.eat_char(quote) {
                    break;
                }
                chars.push(quote);
                continue;
            }
            let part = match self.common_content()? {
                Some(Common::Brace(c)) => {
                    chars.push(c);
                    continue;
                }
                Some(Common::Reference(c)) => AttributeValuePart::Reference(c),
                Some(Common::Enclosed(expr)) => AttributeValuePart::Enclosed(expr),
                None => {
                    chars.push(self.attribute_char(start)?);
                    continue;
                }
            };
            flush(&mut chars, &mut parts);
            parts.push(part);
        }
        flush(&mut chars, &mut parts);
        Ok(parts)
    }

    /// The next character of an attribute value begun at `start`: a
    /// `QuotAttrContentChar` or `AposAttrContentChar`, no `<` [XQuery
    /// Appendix A.2.1 \[149\], \[150\]].
    fn attribute_char(&mut self, start: usize) -> Result<char> {
        let at = self.lexer.position();
        match self.lexer.bump() {
            Some('<') => Err(self.lexer.error(at, "an attribute's value holds no '<' [XQuery Appendix A.2.1 [149], [150]]")),
            Some(c) => Ok(c),
            None => Err(self.lexer.error(start, "an attribute's value is not closed by its quote [XQuery Appendix A.1 [98]]")),
        }
    }

    /// The static errors `attribute`, at `at`, raises beside those before
    /// it, `before`, that its name as written shows [XQuery §3.7.1.1,
    /// §3.7.1.2].
    fn check_attribute(&self, before: &[DirAttribute], attribute: &DirAttribute, at: usize) -> Result<()> {
        let place = self.lexer.place(at);
        let name = &attribute.name;
        if before.iter().any(|other| other.name == *name) {
            let code = if declared_prefix(name).is_some() { ErrorCode::XQST0071 } else { ErrorCode::XQST0040 };
            let why = format!("a direct element constructor has two attributes named '{name}' [XQuery §3.7.1.1, §3.7.1.2]");
            return Err(xquery_error(code, place, &why));
        }
        let Some(prefix) = declared_prefix(name) else {
            return Ok(());
        };
        let Some(uri) = uri_literal(&attribute.value) else {
            let why = "a namespace declaration attribute's value is a URI literal, with no enclosed expression [XQuery §3.7.1.2]";
            return Err(xquery_error(ErrorCode::XQST0022, place, why));
        };
        namespace_errors(prefix, &uri, self.lexer.version()).map_or(Ok(()), |(code, why)| Err(xquery_error(code, place, why)))
    }
}

/// The prefix a namespace declaration attribute named `name` declares,
/// the empty string for the default namespace; none if it is no such
/// attribute [XQuery §3.7.1.2].
fn declared_prefix(name: &QName) -> Option<&str> {
    match &name.prefix {
        None => (name.local == XMLNS).then_some(""),
        Some(prefix) => (prefix == XMLNS).then_some(name.local.as_str()),
    }
}

/// The value of an attribute's parts, if it is a `URILiteral`: no part
/// an enclosed expression [XQuery §3.7.1.2].
fn uri_literal(value: &[AttributeValuePart]) -> Option<String> {
    let mut uri = String::new();
    for part in value {
        match part {
            AttributeValuePart::Chars(text) => uri.push_str(text),
            AttributeValuePart::Reference(c) => uri.push(*c),
            AttributeValuePart::Enclosed(_) => return None,
        }
    }
    Some(uri)
}

/// The static error of binding `prefix`, the empty string for the
/// default namespace, to `uri`, with its reason [XQuery §3.7.1.2]: the
/// prefixes `xml` and `xmlns` are bound once and for all, and no other is
/// bound to `xml`'s namespace \[err:XQST0070\]; and a prefix is undeclared
/// only by Namespaces in XML 1.1, which XML 1.0's lexical rules do not
/// follow \[err:XQST0085\].
fn namespace_errors(prefix: &str, uri: &str, version: Version) -> Option<(ErrorCode, &'static str)> {
    if prefix == XML_PREFIX || prefix == XMLNS {
        return Some((ErrorCode::XQST0070, "a namespace declaration attribute declares the prefix 'xml' or 'xmlns' [XQuery §3.7.1.2]"));
    }
    if !prefix.is_empty() && uri == XML_NAMESPACE {
        return Some((
            ErrorCode::XQST0070,
            "a namespace declaration attribute binds a prefix other than 'xml' to the namespace of 'xml' [XQuery §3.7.1.2]",
        ));
    }
    if !prefix.is_empty() && uri.is_empty() && version == Version::V10 {
        return Some((
            ErrorCode::XQST0085,
            "a namespace declaration attribute undeclares a prefix, which Namespaces in XML 1.0 does not allow [XQuery §3.7.1.2]",
        ));
    }
    None
}

/// Adds the characters `chars` holds to `parts`, if any, as one part.
fn flush(chars: &mut String, parts: &mut Vec<AttributeValuePart>) {
    if !chars.is_empty() {
        parts.push(AttributeValuePart::Chars(mem::take(chars)));
    }
}
