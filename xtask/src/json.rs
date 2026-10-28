//! A small JSON reader, for the messages cargo and clippy print with
//! `--message-format=json`. The tool uses only std (RULES.md R-TOOL-2), and
//! these messages nest fields of the same names (a diagnostic's `children`
//! have their own `spans` and `code`), so they are parsed, not searched.

use crate::error::{Error, Result};

// RFC 8259 §7: a `\u` escape is four hex digits, and a character beyond the
// Basic Multilingual Plane is written as a UTF-16 surrogate pair.
const UNICODE_ESCAPE_DIGITS: u8 = 4;
const HEX_RADIX: u32 = 16;
const HIGH_SURROGATES: u32 = 0xD800;
const LOW_SURROGATES: u32 = 0xDC00;
/// Bits each surrogate carries.
const SURROGATE_BITS: u32 = 10;
/// The first code point a surrogate pair stands for.
const SUPPLEMENTARY_PLANES: u32 = 0x1_0000;

/// Deepest nesting read (R-CF-6). Cargo's messages nest about six levels.
const MAX_DEPTH: usize = 64;

/// A JSON value. Numbers keep their text: the tool only compares them.
#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value of field `key`, if this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Json> {
        let Json::Object(fields) = self else { return None };
        fields.iter().find(|(name, _)| name == key).map(|(_, value)| value)
    }

    /// The text, if this is a string.
    pub fn text(&self) -> Option<&str> {
        if let Json::Text(text) = self { Some(text) } else { None }
    }

    /// The elements, if this is an array; otherwise none.
    pub fn items(&self) -> &[Json] {
        if let Json::Array(items) = self { items } else { &[] }
    }
}

/// Parses one JSON value that makes up all of `text`.
pub fn parse(text: &str) -> Result<Json> {
    let mut reader = Reader { chars: text.chars().collect(), at: 0 };
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at == reader.chars.len() { Ok(value) } else { Err(reader.error("text after the value")) }
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn error(&self, what: &str) -> Error {
        Error::new(format!("JSON: {what} at character {}", self.at))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        self.at = self.at.saturating_add(1);
        c
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn expect(&mut self, wanted: char) -> Result<()> {
        self.skip_space();
        if self.bump() == Some(wanted) { Ok(()) } else { Err(self.error(&format!("expected '{wanted}'"))) }
    }

    fn value(&mut self, depth: usize) -> Result<Json> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        self.skip_space();
        match self.peek() {
            Some('{') => self.object(depth),
            Some('[') => self.array(depth),
            Some('"') => self.string().map(Json::Text),
            Some('t') => self.word("true", Json::Bool(true)),
            Some('f') => self.word("false", Json::Bool(false)),
            Some('n') => self.word("null", Json::Null),
            Some(_) => Ok(Json::Number(self.number()?)),
            None => Err(self.error("end of text")),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json> {
        for wanted in word.chars() {
            if self.bump() != Some(wanted) {
                return Err(self.error(&format!("expected {word}")));
            }
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<String> {
        let mut text = String::new();
        while let Some(c) = self.peek().filter(|c| c.is_ascii_digit() || "+-.eE".contains(*c)) {
            text.push(c);
            self.bump();
        }
        if text.is_empty() { Err(self.error("expected a value")) } else { Ok(text) }
    }

    fn array(&mut self, depth: usize) -> Result<Json> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth.saturating_add(1))?);
            self.skip_space();
            match self.bump() {
                Some(',') => {}
                Some(']') => return Ok(Json::Array(items)),
                Some(_) | None => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json> {
        self.expect('{')?;
        let mut fields = Vec::new();
        self.skip_space();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip_space();
            let name = self.string()?;
            self.expect(':')?;
            fields.push((name, self.value(depth.saturating_add(1))?));
            self.skip_space();
            match self.bump() {
                Some(',') => {}
                Some('}') => return Ok(Json::Object(fields)),
                Some(_) | None => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    /// A string, with its escapes undone.
    fn string(&mut self) -> Result<String> {
        self.expect('"')?;
        let mut text = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(text),
                Some('\\') => text.push(self.escape()?),
                Some(c) => text.push(c),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    /// The character an escape after `\` stands for. A `\u` surrogate pair is
    /// combined; a lone surrogate becomes U+FFFD.
    fn escape(&mut self) -> Result<char> {
        match self.bump() {
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('r') => Ok('\r'),
            Some('b') => Ok('\u{8}'),
            Some('f') => Ok('\u{c}'),
            Some('u') => self.unicode(),
            Some(c) => Ok(c),
            None => Err(self.error("unterminated escape")),
        }
    }

    fn unicode(&mut self) -> Result<char> {
        let high = self.hex4()?;
        if !(HIGH_SURROGATES..LOW_SURROGATES).contains(&high) {
            return Ok(char::from_u32(high).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        // A high surrogate must be followed by `\u` and a low one.
        let backslash = self.bump();
        let u = self.bump();
        if backslash != Some('\\') || u != Some('u') {
            return Ok(char::REPLACEMENT_CHARACTER);
        }
        let low = self.hex4()?;
        let combined = high
            .checked_sub(HIGH_SURROGATES)
            .and_then(|h| low.checked_sub(LOW_SURROGATES).map(|l| (h << SURROGATE_BITS | l).saturating_add(SUPPLEMENTARY_PLANES)));
        Ok(combined.and_then(char::from_u32).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits: String = (0..UNICODE_ESCAPE_DIGITS).filter_map(|_| self.bump()).collect();
        u32::from_str_radix(&digits, HEX_RADIX).map_err(|_| self.error("bad \\u escape"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-DEV-2 nested objects and arrays are read whole, so a
    /// field is found where it is, not in a nested value of the same name.
    #[test]
    fn nested_fields_are_told_apart() {
        let text = r#"{"message":{"children":[{"code":null,"spans":[]}],"level":"warning","code":{"code":"clippy::panic"}}}"#;
        let json = parse(text).unwrap();
        let message = json.get("message").unwrap();
        assert_eq!(message.get("code").and_then(|c| c.get("code")).and_then(Json::text), Some("clippy::panic"));
        assert_eq!(message.get("children").map(|c| c.items().len()), Some(1));
        assert_eq!(message.get("level").and_then(Json::text), Some("warning"));
    }

    /// Verifies: [TOOL] R-DEV-2 strings are unescaped, numbers and literals read.
    #[test]
    fn scalars_and_escapes() {
        let json = parse(r#"[ "a\"b\\cé😀", 12.5e3, true, false, null ]"#).unwrap();
        let expected = vec![
            Json::Text("a\"b\\c\u{e9}\u{1F600}".into()),
            Json::Number("12.5e3".into()),
            Json::Bool(true),
            Json::Bool(false),
            Json::Null,
        ];
        assert_eq!(json, Json::Array(expected));
    }

    /// Verifies: [TOOL] R-CF-6 malformed and over-deep input is an error, not a panic.
    #[test]
    fn bad_input_is_an_error() {
        for text in ["", "{", "[1,", r#"{"a" 1}"#, r#""open"#, "[1] 2", "tru"] {
            assert!(parse(text).is_err(), "{text:?}");
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(parse(&deep).is_err());
    }
}
