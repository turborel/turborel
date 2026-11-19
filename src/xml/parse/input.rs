//! The characters a parser reads: the document entity's, its line ends
//! normalized [XML §2.11], then the replacement texts of the entities it
//! references, each read on a stack of frames, so an entity's markup
//! begins and ends in it [XML §4.3.2] and no entity references itself
//! [XML §4.1 WFC: No Recursion].

use std::rc::Rc;

use super::{Frame, Parser, invalid};
use crate::error::{Result, SqlError};
use crate::xml::chars::{CARRIAGE_RETURN, LINE_FEED, Version, is_literal, is_name_char, is_name_start, is_space};
use crate::xml::{MAX_TEXT, beyond};

/// [XML 1.1 §2.11] The next line character, a line end of XML 1.1.
const NEXT_LINE: char = '\u{85}';
/// [XML 1.1 §2.11] The line separator, a line end of XML 1.1.
const LINE_SEPARATOR: char = '\u{2028}';

/// `chars` with their line ends normalized for `version` [XML §2.11]:
/// each carriage return and line feed, and each carriage return alone, a
/// line feed; of XML 1.1 also each carriage return and next line, each
/// next line alone and each line separator.
pub(crate) fn normalized(version: Version, chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut after_return = false;
    for &c in chars {
        let follows_return = after_return;
        after_return = c == CARRIAGE_RETURN;
        let v11 = version == Version::V11;
        let next_line = v11 && c == NEXT_LINE;
        if follows_return && (c == LINE_FEED || next_line) {
            continue;
        }
        let line_end = c == CARRIAGE_RETURN || next_line || (v11 && c == LINE_SEPARATOR);
        out.push(if line_end { LINE_FEED } else { c });
    }
    out
}

impl Parser {
    /// The text being read, if any.
    fn frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    /// The next character of the text being read, if it has one.
    pub(super) fn peek(&self) -> Option<char> {
        self.frame().and_then(|f| f.chars.get(f.at).copied())
    }

    /// The character `ahead` after the next, if the text has it.
    pub(super) fn peek_ahead(&self, ahead: usize) -> Option<char> {
        self.frame().and_then(|f| f.chars.get(f.at.saturating_add(ahead)).copied())
    }

    /// The next character, read.
    pub(super) fn bump(&mut self) -> Option<char> {
        let frame = self.frames.last_mut()?;
        let c = frame.chars.get(frame.at).copied()?;
        frame.at = frame.at.saturating_add(1);
        Some(c)
    }

    /// Whether the text being read is at its end.
    pub(super) fn at_end(&self) -> bool {
        self.peek().is_none()
    }

    /// Whether the text being read goes on with `text`.
    pub(super) fn looking_at(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(i, c)| self.peek_ahead(i) == Some(c))
    }

    /// `text` read if the text goes on with it; whether it did.
    pub(super) fn eat(&mut self, text: &str) -> bool {
        let found = self.looking_at(text);
        if found {
            for _ in text.chars() {
                self.bump();
            }
        }
        found
    }

    /// `text` read, which `rule` requires next.
    pub(super) fn expect(&mut self, text: &str, rule: &str) -> Result<()> {
        if self.eat(text) { Ok(()) } else { Err(self.malformed(&format!("{rule} requires '{text}' here"))) }
    }

    /// White space read, `S?`; whether there was any [XML §2.3 \[3\]].
    pub(super) fn space(&mut self) -> bool {
        let mut any = false;
        while self.peek().is_some_and(is_space) {
            self.bump();
            any = true;
        }
        any
    }

    /// White space read, which `rule` requires, `S`.
    pub(super) fn require_space(&mut self, rule: &str) -> Result<()> {
        if self.space() { Ok(()) } else { Err(self.malformed(&format!("{rule} requires white space here"))) }
    }

    /// A `Name` read, which `rule` requires [XML §2.3 \[5\]].
    pub(super) fn name(&mut self, rule: &str) -> Result<String> {
        let version = self.version;
        if !self.peek().is_some_and(|c| is_name_start(version, c)) {
            return Err(self.malformed(&format!("{rule} requires a name here [XML §2.3 [5]]")));
        }
        let mut name = String::new();
        while let Some(c) = self.peek().filter(|&c| is_name_char(version, c)) {
            name.push(c);
            self.bump();
        }
        Ok(name)
    }

    /// The characters read up to `end`, which is read too and is not
    /// among them; `construct` names what is read, which ends in the text
    /// it begins in.
    pub(super) fn until(&mut self, end: &str, construct: &str) -> Result<String> {
        let mut read = String::new();
        loop {
            if self.eat(end) {
                return Ok(read);
            }
            match self.bump() {
                Some(c) => read.push(c),
                None => return Err(self.malformed(&format!("{construct} has no '{end}' to end it in the entity it begins in"))),
            }
        }
    }

    /// The document entity, `chars`, read from its start after its XML
    /// declaration, if it has one, set its version: its line ends
    /// normalized [XML §2.11], and each character one that may be written
    /// [XML §2.2 \[2\]] [XML 1.1 §2.2 \[2a\]].
    pub(super) fn begin(&mut self, chars: &[char]) -> Result<()> {
        let (declaration, after) = self.declaration(chars)?;
        self.decl = declaration;
        let rest = chars.get(after..).unwrap_or_default();
        if let Some(bad) = rest.iter().position(|&c| !is_literal(self.version, c)) {
            let at = after.saturating_add(bad);
            let code = rest.get(bad).map_or(0, |&c| u32::from(c));
            let place = place_of(chars.get(..at).unwrap_or_default());
            return Err(invalid(
                self.syntax,
                &format!("the character #x{code:X} may not be written in XML {} [XML §2.2 [2]]", self.version.number()),
                Some(&place),
            ));
        }
        let text = normalized(self.version, rest);
        let mut head = chars.get(..after).map(<[char]>::to_vec).unwrap_or_default();
        head.extend(text);
        self.frames.push(Frame { chars: head.into(), at: after, entity: None, parameter: false });
        Ok(())
    }

    /// The replacement text `chars` of entity `name` read next, a
    /// parameter entity's if `parameter`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200M or 2200N if the entity is being read already [XML
    /// §4.1 WFC: No Recursion]; SQLCODE -900 if entities' replacement
    /// texts would add more than `MAX_TEXT` characters.
    pub(super) fn enter(&mut self, name: &str, chars: Rc<[char]>, parameter: bool) -> Result<()> {
        if self.reading(name, parameter) {
            return Err(self.malformed(&format!("the entity '{name}' references itself [XML §4.1 WFC: No Recursion]")));
        }
        self.expanded = self.expanded.saturating_add(chars.len());
        if self.expanded > MAX_TEXT {
            return Err(beyond(&format!("its entities expand to more than {MAX_TEXT} characters")));
        }
        self.frames.push(Frame { chars, at: 0, entity: Some(name.into()), parameter });
        Ok(())
    }

    /// Whether the entity `name` is being read.
    pub(super) fn reading(&self, name: &str, parameter: bool) -> bool {
        self.frames.iter().any(|f| f.parameter == parameter && f.entity.as_deref() == Some(name))
    }

    /// How many texts are being read.
    pub(super) fn depth_of_frames(&self) -> usize {
        self.frames.len()
    }

    /// The text being read left, at its end; whether it was an entity's,
    /// not the document entity, which is never left.
    pub(super) fn leave(&mut self) -> bool {
        if self.frames.len() > 1 {
            self.frames.pop();
            true
        } else {
            false
        }
    }

    /// Whether the text being read is a parameter entity's.
    pub(super) fn in_parameter_entity(&self) -> bool {
        self.frames.iter().any(|f| f.parameter)
    }

    /// The error of a text that is not well-formed, `why`, where the
    /// document entity is being read, and in which entity.
    ///
    /// A declaration of the internal subset that is malformed where a `%`
    /// is next is malformed by a parameter entity reference within it.
    pub(super) fn malformed(&self, why: &str) -> SqlError {
        if self.dtd.in_subset && self.peek() == Some('%') {
            return self.located("a parameter entity is referenced within a declaration [XML §2.8 WFC: PEs in Internal Subset]");
        }
        self.located(why)
    }

    /// The error of a text that is not well-formed, `why`, at the place
    /// being read.
    fn located(&self, why: &str) -> SqlError {
        let mut place = self.frames.first().map(|f| place_of(f.chars.get(..f.at).unwrap_or_default())).unwrap_or_default();
        if let Some(name) = self.frames.last().and_then(|f| f.entity.as_deref()) {
            place = format!("{place}, in the entity '{name}'");
        }
        invalid(self.syntax, why, Some(&place))
    }
}

/// Where the character after `before` is: its line and column, each
/// counted from 1.
fn place_of(before: &[char]) -> String {
    let line = before.iter().filter(|&&c| c == LINE_FEED).count().saturating_add(1);
    let column = before.iter().rev().take_while(|&&c| c != LINE_FEED).count().saturating_add(1);
    format!("at line {line}, column {column}")
}
