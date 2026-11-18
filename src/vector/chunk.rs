//! \[EXTENSION\] Chunking, the vector design's §12.3:
//!
//! ```text
//! CHUNK_TEXT(expr,
//!            BY { CHARS | WORDS | SENTENCES | PARAGRAPHS | TOKENS model },
//!            MAX n, OVERLAP m,
//!            [ SPLIT_ON 'regex' ], [ NORMALIZE WHITESPACE ])
//!   -> table (chunk_id, chunk_offset, chunk_length, chunk_text)
//! ```
//!
//! A text is cut into units of the kind BY names, and a chunk is at most
//! MAX units in a row, each chunk starting OVERLAP units before the last
//! one ended, so MAX and OVERLAP both count the unit BY names. Its text
//! is the text from its first unit's start to its last unit's end, what
//! lies between units included. Readings of the design, as it leaves
//! them open:
//!
//! * A character is a Unicode scalar value. A word is a run of characters
//!   that are not white space. A sentence ends after `.`, `!` or `?`, and
//!   any closing quotes or brackets after it, before white space or the
//!   text's end; a paragraph ends at a blank line, which also ends a
//!   sentence. Sentences and paragraphs are trimmed of white space.
//! * TOKENS are the model's tokens (`Embedder::tokens`).
//! * SPLIT_ON cuts the text at each match of an XQuery regular expression
//!   (the matcher of LIKE_REGEX, RULES.md D61) first. No chunk crosses a
//!   match, and the matches are in none.
//! * NORMALIZE WHITESPACE makes each run of white space in a chunk's text
//!   one space, and trims it; its offset and length still give where it
//!   lies in the text.
//! * chunk_id counts chunks from 1; chunk_offset is the character its
//!   text starts at, from 1, as POSITION counts; chunk_length its
//!   characters in the text.
//! * Left out, BY is WORDS, MAX 256 and OVERLAP 0; OVERLAP is less than
//!   MAX. An empty text has no chunks.

use crate::error::{Code, Result, SqlError};
use crate::regex::Regex;

/// \[EXTENSION\] MAX left out: §12.5's example's 256.
pub const DEFAULT_MAX: usize = 256;
/// \[DESIGN\] The most units a chunk may be set to hold.
pub const MOST_MAX: usize = 1_000_000;
/// \[DESIGN\] The most matches of SPLIT_ON a text is cut at.
const MOST_SPLITS: usize = 1_000_000;
/// The characters that end a sentence.
const SENTENCE_ENDS: [char; 3] = ['.', '!', '?'];
/// Characters that may close a sentence after its end.
const CLOSERS: [char; 6] = ['"', '\'', ')', ']', '\u{201d}', '\u{2019}'];
/// The quote of a string in a specification.
const QUOTE: char = '\'';

/// \[EXTENSION\] The unit chunks are counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Characters.
    Characters,
    /// Words: runs of characters not white space.
    Words,
    /// Sentences.
    Sentences,
    /// Paragraphs, parted by blank lines.
    Paragraphs,
    /// A model's tokens.
    Tokens,
}

/// The key words of the specification, as written.
const BY: &str = "BY";
const MAX: &str = "MAX";
const OVERLAP: &str = "OVERLAP";
const SPLIT_ON: &str = "SPLIT_ON";
const NORMALIZE: &str = "NORMALIZE";
const WHITESPACE: &str = "WHITESPACE";
/// The words naming each unit; CHARS and CHARACTERS name one.
pub const UNITS: [(&str, Unit); 6] = [
    ("CHARS", Unit::Characters),
    ("CHARACTERS", Unit::Characters),
    ("WORDS", Unit::Words),
    ("SENTENCES", Unit::Sentences),
    ("PARAGRAPHS", Unit::Paragraphs),
    ("TOKENS", Unit::Tokens),
];

/// \[EXTENSION\] How a text is chunked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The unit.
    pub by: Unit,
    /// The model whose tokens TOKENS counts.
    pub model: Option<String>,
    /// The most units of a chunk, at least 1.
    pub max: usize,
    /// The units a chunk repeats of the one before, less than `max`.
    pub overlap: usize,
    /// The regular expression the text is cut at first.
    pub split_on: Option<String>,
    /// Whether white space in a chunk's text is made single spaces.
    pub normalize: bool,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec { by: Unit::Words, model: None, max: DEFAULT_MAX, overlap: 0, split_on: None, normalize: false }
    }
}

fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] a chunking specification {why}"))
}

/// The rest of a quoted string, its opening quote read, a doubled quote
/// one quote, kept with a quote before it to mark it quoted.
fn quoted_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<String> {
    let mut quoted = String::from(QUOTE);
    loop {
        let Some(next) = chars.next() else { return Err(refused("has a quoted string not closed")) };
        if next != QUOTE {
            quoted.push(next);
        } else if chars.next_if_eq(&QUOTE).is_some() {
            quoted.push(QUOTE);
        } else {
            return Ok(quoted);
        }
    }
}

/// The words, numbers and quoted strings of a specification, the words
/// in upper case.
fn words(text: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == ',' {
            chars.next();
        } else if c == QUOTE {
            chars.next();
            out.push(quoted_string(&mut chars)?);
        } else {
            let word: String = std::iter::from_fn(|| chars.next_if(|n| !n.is_whitespace() && *n != ',' && *n != QUOTE)).collect();
            out.push(word.to_ascii_uppercase());
        }
    }
    Ok(out)
}

type Words = std::iter::Peekable<std::vec::IntoIter<String>>;

impl Spec {
    /// The specification `text` writes, as §12.3's arguments are written
    /// after the text: `BY WORDS MAX 256 OVERLAP 32 NORMALIZE WHITESPACE`,
    /// commas between them or not, key words in any case.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is not one, names a part twice, or sets OVERLAP
    /// not less than MAX or MAX past 1000000.
    pub fn parse(text: &str) -> Result<Spec> {
        let mut spec = Spec::default();
        let mut seen: Vec<String> = Vec::new();
        let mut words = words(text)?.into_iter().peekable();
        while let Some(word) = words.next() {
            if seen.contains(&word) {
                return Err(refused(&format!("names {word} twice")));
            }
            spec.part(&word, &mut words)?;
            seen.push(word);
        }
        spec.checked()
    }

    /// Reads the part `word` begins.
    fn part(&mut self, word: &str, words: &mut Words) -> Result<()> {
        match word {
            BY => self.by_unit(words)?,
            MAX => self.max = count(words.next())?,
            OVERLAP => self.overlap = count(words.next())?,
            SPLIT_ON => self.split_on = Some(quoted(words.next())?),
            NORMALIZE => {
                words.next_if(|w| w == WHITESPACE).ok_or_else(|| refused("has NORMALIZE without WHITESPACE"))?;
                self.normalize = true;
            }
            _ => return Err(refused(&format!("has {word} where BY, MAX, OVERLAP, SPLIT_ON or NORMALIZE WHITESPACE goes"))),
        }
        Ok(())
    }

    /// Reads BY's unit, and TOKENS' model.
    fn by_unit(&mut self, words: &mut Words) -> Result<()> {
        let unit = words.next().unwrap_or_default();
        let (_, by) = UNITS.iter().find(|(name, _)| *name == unit).ok_or_else(|| refused(&format!("has {unit} where a unit goes")))?;
        self.by = *by;
        if *by == Unit::Tokens {
            let model = words.next().ok_or_else(|| refused("names no model after TOKENS"))?;
            self.model = Some(model.strip_prefix(QUOTE).map_or_else(|| model.clone(), str::to_owned));
        }
        Ok(())
    }

    /// Itself, if MAX and OVERLAP are in range.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if not.
    pub fn checked(self) -> Result<Spec> {
        if self.max == 0 || self.max > MOST_MAX || self.overlap >= self.max {
            return Err(refused("has MAX from 1 to 1000000 and OVERLAP less than MAX"));
        }
        Ok(self)
    }

    /// It written as `parse` reads it.
    pub fn written(&self) -> String {
        let unit = UNITS.iter().find(|(_, u)| *u == self.by).map_or("WORDS", |(name, _)| *name);
        let model = self.model.as_deref().map(|m| format!(" {}", quote(m))).unwrap_or_default();
        let split = self.split_on.as_deref().map(|p| format!(" {SPLIT_ON} {}", quote(p))).unwrap_or_default();
        let normalize = if self.normalize { format!(" {NORMALIZE} {WHITESPACE}") } else { String::new() };
        format!("{BY} {unit}{model} {MAX} {} {OVERLAP} {}{split}{normalize}", self.max, self.overlap)
    }

    /// Its splitter compiled: SPLIT_ON's expression, once for every text.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is out of range; SQLSTATE 2201S if SPLIT_ON is
    /// no regular expression.
    pub fn compiled(self) -> Result<Chunker> {
        let spec = self.checked()?;
        let split = spec.split_on.as_deref().map(|p| Regex::new(p, "")).transpose()?;
        Ok(Chunker { spec, split })
    }
}

/// `text` as a quoted string, its quotes doubled.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace(QUOTE, "''"))
}

/// The count a word gives.
fn count(word: Option<String>) -> Result<usize> {
    word.and_then(|w| w.parse::<usize>().ok()).ok_or_else(|| refused("has no whole number where a count goes"))
}

/// The text of a quoted word.
fn quoted(word: Option<String>) -> Result<String> {
    word.and_then(|w| w.strip_prefix(QUOTE).map(str::to_owned)).ok_or_else(|| refused("has no quoted string after SPLIT_ON"))
}

/// \[EXTENSION\] A chunk of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Its number, from 1.
    pub id: usize,
    /// The character its text starts at, from 1.
    pub offset: usize,
    /// Its characters in the text.
    pub length: usize,
    /// Its text, its white space made single spaces if asked.
    pub text: String,
}

/// A span of characters from 0, its end excluded.
pub type Span = (usize, usize);

/// \[EXTENSION\] A specification ready to chunk texts.
#[derive(Debug, Clone)]
pub struct Chunker {
    spec: Spec,
    split: Option<Regex>,
}

impl Chunker {
    /// Its specification.
    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// The chunks of `text`, the spans of its tokens given by `tokens`
    /// for TOKENS.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if SPLIT_ON's search passes its steps.
    pub fn chunks(&self, text: &str, tokens: &dyn Fn(&str) -> Vec<Span>) -> Result<Vec<Chunk>> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        for (start, end) in self.segments(&chars)? {
            let piece = chars.get(start..end).unwrap_or_default();
            let units = match self.spec.by {
                Unit::Characters => (0..piece.len()).map(|at| (at, at.saturating_add(1))).collect(),
                Unit::Words => words_of(piece),
                Unit::Sentences => sentences_of(piece),
                Unit::Paragraphs => paragraphs_of(piece),
                Unit::Tokens => tokens(&piece.iter().collect::<String>()),
            };
            self.window(&chars, start, &units, &mut out);
        }
        Ok(out)
    }

    /// Adds the chunks of `units`, spans from `start`, to `out`.
    fn window(&self, chars: &[char], start: usize, units: &[Span], out: &mut Vec<Chunk>) {
        let step = self.spec.max.saturating_sub(self.spec.overlap).max(1);
        let mut first = 0_usize;
        while first < units.len() {
            let last = first.saturating_add(self.spec.max).min(units.len()).saturating_sub(1);
            let (Some(&(from, _)), Some(&(_, to))) = (units.get(first), units.get(last)) else { return };
            out.push(chunk(chars, (start.saturating_add(from), start.saturating_add(to)), out.len(), self.spec.normalize));
            if last.saturating_add(1) >= units.len() {
                return;
            }
            first = first.saturating_add(step);
        }
    }

    /// The spans between SPLIT_ON's matches, or the whole text.
    fn segments(&self, chars: &[char]) -> Result<Vec<Span>> {
        let Some(regex) = self.split.as_ref() else { return Ok(vec![(0, chars.len())]) };
        let mut out = Vec::new();
        let mut start = 0_usize;
        for found in regex.list(chars, 0, MOST_SPLITS)? {
            // A match of nothing cuts nothing.
            let Some((from, to)) = found.get(0).filter(|(from, to)| to > from) else { continue };
            out.push((start, from));
            start = to;
        }
        out.push((start, chars.len()));
        Ok(out)
    }
}

/// The chunk of characters `span` of `chars`, the `before`th found.
fn chunk(chars: &[char], (from, to): Span, before: usize, normalize: bool) -> Chunk {
    let text: String = chars.get(from..to).unwrap_or_default().iter().collect();
    let text = if normalize { text.split_whitespace().collect::<Vec<_>>().join(" ") } else { text };
    Chunk { id: before.saturating_add(1), offset: from.saturating_add(1), length: to.saturating_sub(from), text }
}

/// The words of `chars`: runs not white space.
pub fn words_of(chars: &[char]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in chars.iter().enumerate() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                out.push((from, at));
                start = None;
            }
            (false, Some(_)) | (true, None) => {}
        }
    }
    if let Some(from) = start {
        out.push((from, chars.len()));
    }
    out
}

/// Where each blank line starts and ends: a line break, white space
/// holding another line break, and the white space after it.
fn blank_lines(chars: &[char]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0_usize;
    while let Some(&c) = chars.get(at) {
        let next = at.saturating_add(1);
        if c == '\n' {
            let gap = chars.get(next..).unwrap_or_default().iter().take_while(|w| w.is_whitespace()).count();
            let end = next.saturating_add(gap);
            if chars.get(next..end).unwrap_or_default().contains(&'\n') {
                out.push((at, end));
                at = end;
                continue;
            }
        }
        at = next;
    }
    out
}

/// The paragraphs of `chars`, trimmed.
fn paragraphs_of(chars: &[char]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = 0_usize;
    for (from, to) in blank_lines(chars).into_iter().chain(std::iter::once((chars.len(), chars.len()))) {
        out.extend(trimmed(chars, (start, from)));
        start = to;
    }
    out
}

/// The sentences of `chars`, trimmed: each paragraph cut after each end
/// of a sentence that white space or the paragraph's end follows.
fn sentences_of(chars: &[char]) -> Vec<Span> {
    let ends = |at: usize| chars.get(at).is_some_and(|c| SENTENCE_ENDS.contains(c));
    let closes = |at: usize| chars.get(at).is_some_and(|c| CLOSERS.contains(c) || SENTENCE_ENDS.contains(c));
    let mut out = Vec::new();
    for (from, to) in paragraphs_of(chars) {
        let (mut start, mut at) = (from, from);
        while at < to {
            let end = ends(at);
            at = at.saturating_add(1);
            if !end {
                continue;
            }
            while at < to && closes(at) {
                at = at.saturating_add(1);
            }
            if at >= to || chars.get(at).is_some_and(|c| c.is_whitespace()) {
                out.extend(trimmed(chars, (start, at)));
                start = at;
            }
        }
        out.extend(trimmed(chars, (start, to)));
    }
    out
}

/// `span` without white space at either end; none if that is all it has.
fn trimmed(chars: &[char], (from, to): Span) -> Option<Span> {
    let inside = chars.get(from..to)?;
    let lead = inside.iter().take_while(|c| c.is_whitespace()).count();
    let trail = inside.iter().rev().take_while(|c| c.is_whitespace()).count();
    let (start, end) = (from.saturating_add(lead), to.saturating_sub(trail));
    (start < end).then_some((start, end))
}

// Its tests are `embed/tests/chunk.rs`, with the CHUNK_TEXT routine's.
