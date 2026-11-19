//! The segmenter: one character at a time, where words begin and end and
//! where sentences and paragraphs end [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §4.1, §5.1.5]. It holds at most the word being read and one
//! joiner, or the last two letters of a run read as bigrams, never the
//! document.
//!
//! In a language read as bigrams (RULES.md D323) a run of the letters of
//! the scripts written without spaces, each with the marks after it, is
//! read as the overlapping pairs of its letters, each pair a word given
//! once its second letter is whole. A run of one letter is a word of one.
//! In a language with elisions a word's elided article or pronoun before
//! its first apostrophe is taken off it (D322).

use super::kind::{Kind, Tables};
use super::language::Profile;
use super::{Language, StopWords, Token};

/// The characters of a word kept in `Token::word` \[DESIGN\]. A longer run
/// keeps its first characters, so the memory a document takes is bounded,
/// while `Token::length` counts all of them.
pub const MAX_WORD_CHARS: u32 = 256;

/// The line terminators in a row, with only blanks between, that end a
/// paragraph.
const PARAGRAPH_LINES: u8 = 2;

/// MIDDLE DOT, which joins the letters around it in Catalan, `l·l`.
const MIDDLE_DOT: char = '\u{B7}';

/// The characters of a letter of a run read as bigrams kept with its
/// marks \[DESIGN\]; more marks are counted but not kept.
const MAX_UNIT_CHARS: u32 = 16;

/// A boundary seen since the last word, applied when the next word begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Boundary {
    /// None.
    None,
    /// A sentence end.
    Sentence,
    /// A paragraph end, which also ends the sentence.
    Paragraph,
}

/// A character read, and what it is.
#[derive(Debug, Clone, Copy)]
struct Seen {
    c: char,
    /// Its offset.
    at: u32,
    kind: Kind,
    /// Whether CARRIAGE RETURN came right before it.
    after_return: bool,
}

/// The word being read.
#[derive(Debug)]
struct Word {
    /// Its first `MAX_WORD_CHARS` characters, as written.
    text: String,
    /// The offset of its first character.
    start: u32,
    /// Its characters.
    length: u32,
    /// Whether it is an ideograph, which only marks may follow.
    ideograph: bool,
    /// Whether its last character is a letter, so an apostrophe may join.
    letter: bool,
}

impl Word {
    /// `c` added to it.
    fn push(&mut self, c: char) {
        if self.length < MAX_WORD_CHARS {
            self.text.push(c);
        }
        self.length = self.length.saturating_add(1);
    }
}

/// A letter of a run read as bigrams, with the marks after it.
#[derive(Debug)]
struct Unit {
    text: String,
    start: u32,
    length: u32,
}

impl Unit {
    /// The letter `c` at `start`.
    fn new(c: char, start: u32) -> Unit {
        Unit { text: c.to_string(), start, length: 1 }
    }

    /// The mark `c` added to it.
    fn push(&mut self, c: char) {
        if self.length < MAX_UNIT_CHARS {
            self.text.push(c);
        }
        self.length = self.length.saturating_add(1);
    }
}

/// The run being read as bigrams: its letter before the last, if it has
/// more than one, and its last.
#[derive(Debug)]
struct Run {
    previous: Option<Unit>,
    current: Unit,
}

/// The segmenter's state between characters.
#[derive(Debug)]
pub(super) struct Segmenter {
    language: Language,
    profile: &'static Profile,
    /// The run being read as bigrams, if one is.
    run: Option<Run>,
    /// The stop words `Token::stop` follows.
    stop: StopWords,
    /// The offset of the next character.
    offset: u32,
    /// The ordinal of the last word.
    ordinal: u32,
    sentence: u32,
    paragraph: u32,
    word: Option<Word>,
    /// A connector or apostrophe after the word, joining it only if a word
    /// character follows.
    joiner: Option<(char, Kind)>,
    pending: Boundary,
    /// Whether a sentence terminator came, with only closers after it, so a
    /// separator now ends the sentence.
    terminated: bool,
    /// The line terminators in a row, with only blanks between.
    lines: u8,
    /// Whether the last character was CARRIAGE RETURN.
    after_return: bool,
}

impl Segmenter {
    /// A segmenter at the start of a document in `language`, its stop
    /// words `stop`.
    pub(super) fn new(language: Language, stop: StopWords) -> Segmenter {
        Segmenter {
            language,
            profile: language.profile(),
            run: None,
            stop,
            offset: 0,
            ordinal: 0,
            sentence: 1,
            paragraph: 1,
            word: None,
            joiner: None,
            pending: Boundary::None,
            terminated: false,
            lines: 0,
            after_return: false,
        }
    }

    /// The next character `c` read; the word it ends, if any.
    pub(super) fn step(&mut self, tables: &Tables, c: char) -> Option<Token> {
        let at = self.offset;
        self.offset = self.offset.saturating_add(1);
        let kind = if self.profile.middle_dot && c == MIDDLE_DOT { Kind::Apostrophe } else { tables.kind(c) };
        let seen = Seen { c, at, kind, after_return: std::mem::take(&mut self.after_return) };
        if self.run.is_some() {
            return self.extend_run(tables, seen);
        }
        let Some((joiner, joiner_kind)) = self.joiner.take() else {
            return self.place(tables, seen);
        };
        let unit = self.unit(tables, c);
        if let Some(word) = self.word.as_mut().filter(|_| joins(joiner_kind, seen.kind) && !unit) {
            word.push(joiner);
            word.push(c);
            word.letter = seen.kind == Kind::Letter;
            return None;
        }
        let ended = self.end_word(tables);
        self.boundary(joiner_kind, false);
        self.open(tables, seen);
        ended
    }

    /// The end of the document; the word it ends, if any.
    pub(super) fn finish(&mut self, tables: &Tables) -> Option<Token> {
        self.joiner = None;
        if self.run.is_some() {
            return self.end_run(tables);
        }
        self.end_word(tables)
    }

    /// Whether `c` is a letter its language reads as bigrams.
    fn unit(&self, tables: &Tables, c: char) -> bool {
        self.language.reads_bigrams() && tables.is_unit(c)
    }

    /// The character `seen` after a letter of the run: a mark of it, the
    /// next letter, which gives the pair before it, or the run's end.
    fn extend_run(&mut self, tables: &Tables, seen: Seen) -> Option<Token> {
        let unit = self.unit(tables, seen.c);
        let run = self.run.as_mut()?;
        if seen.kind == Kind::Mark {
            run.current.push(seen.c);
            return None;
        }
        if !unit {
            let ended = self.end_run(tables);
            self.open(tables, seen);
            return ended;
        }
        let current = std::mem::replace(&mut run.current, Unit::new(seen.c, seen.at));
        let previous = run.previous.replace(current)?;
        let pair = run.previous.as_ref()?;
        let (text, length) = (format!("{}{}", previous.text, pair.text), previous.length.saturating_add(pair.length));
        Some(self.token(tables, &text, previous.start, length))
    }

    /// The run ended: its last pair, or its one letter.
    fn end_run(&mut self, tables: &Tables) -> Option<Token> {
        let Run { previous, current } = self.run.take()?;
        let Some(previous) = previous else { return Some(self.token(tables, &current.text, current.start, current.length)) };
        let text = format!("{}{}", previous.text, current.text);
        Some(self.token(tables, &text, previous.start, previous.length.saturating_add(current.length)))
    }

    /// The character `seen`, no joiner pending.
    fn place(&mut self, tables: &Tables, seen: Seen) -> Option<Token> {
        let Seen { c, kind, .. } = seen;
        if self.word.is_none() {
            self.open(tables, seen);
            return None;
        }
        let unit = self.unit(tables, c);
        let Some(word) = self.word.as_mut() else { return None };
        let joined = if word.ideograph { kind == Kind::Mark } else { continues(kind) && !unit };
        if joined {
            word.push(c);
            word.letter = kind == Kind::Letter;
            return None;
        }
        if joiner(kind, word) {
            self.joiner = Some((c, kind));
            return None;
        }
        let ended = self.end_word(tables);
        self.open(tables, seen);
        ended
    }

    /// The character `seen`, no word being read, beginning a word or a run
    /// read as bigrams, or taken as a boundary character.
    fn open(&mut self, tables: &Tables, seen: Seen) {
        let Seen { c, at, kind, after_return } = seen;
        let ideograph = kind == Kind::Ideograph;
        let unit = self.unit(tables, c);
        if !continues(kind) && !ideograph && !unit {
            self.boundary(kind, after_return);
            return;
        }
        if self.ordinal > 0 {
            self.apply();
        }
        self.pending = Boundary::None;
        self.terminated = false;
        self.lines = 0;
        if unit {
            self.run = Some(Run { previous: None, current: Unit::new(c, at) });
            return;
        }
        let mut word = Word { text: String::new(), start: at, length: 0, ideograph, letter: kind == Kind::Letter };
        word.push(c);
        self.word = Some(word);
    }

    /// The pending boundary applied to the word beginning.
    fn apply(&mut self) {
        match self.pending {
            Boundary::None => {}
            Boundary::Sentence => self.sentence = self.sentence.saturating_add(1),
            Boundary::Paragraph => {
                self.sentence = self.sentence.saturating_add(1);
                self.paragraph = self.paragraph.saturating_add(1);
            }
        }
    }

    /// A character of `kind` outside any word.
    fn boundary(&mut self, kind: Kind, after_return: bool) {
        match kind {
            Kind::Terminator => {
                self.terminated = true;
                self.lines = 0;
            }
            Kind::WideTerminator => {
                self.end(Boundary::Sentence);
                self.lines = 0;
            }
            Kind::Closer | Kind::Apostrophe | Kind::Connector => self.lines = 0,
            Kind::Blank => self.separate(),
            Kind::LineFeed => {
                if !after_return {
                    self.line();
                }
            }
            Kind::Return => {
                self.line();
                self.after_return = true;
            }
            Kind::Line => self.line(),
            Kind::Paragraph => self.end(Boundary::Paragraph),
            Kind::Letter | Kind::Number | Kind::Mark | Kind::Ideograph | Kind::Other => {
                self.terminated = false;
                self.lines = 0;
            }
        }
    }

    /// A separator, ending the sentence if a terminator came before it.
    fn separate(&mut self) {
        if self.terminated {
            self.end(Boundary::Sentence);
        }
    }

    /// A line terminator, a separator that ends the paragraph at the second
    /// in a row.
    fn line(&mut self) {
        self.separate();
        self.lines = self.lines.saturating_add(1);
        if self.lines >= PARAGRAPH_LINES {
            self.end(Boundary::Paragraph);
        }
    }

    /// `boundary` seen.
    fn end(&mut self, boundary: Boundary) {
        self.pending = self.pending.max(boundary);
        self.terminated = false;
    }

    /// The word being read, ended, its elision taken off.
    fn end_word(&mut self, tables: &Tables) -> Option<Token> {
        let word = self.word.take()?;
        let cut = self.elided(tables, &word.text).unwrap_or(0);
        let text: String = word.text.chars().skip(cut).collect();
        Some(self.token(tables, &text, word.start.saturating_add(as_u32(cut)), word.length.saturating_sub(as_u32(cut))))
    }

    /// The characters of `text`'s elision with its apostrophe: an article
    /// or pronoun its language elides, before its first apostrophe.
    fn elided(&self, tables: &Tables, text: &str) -> Option<usize> {
        if self.profile.elisions.is_empty() {
            return None;
        }
        let at = text.chars().position(|c| tables.kind(c) == Kind::Apostrophe)?;
        let before: String = text.chars().take(at).collect();
        let normalized = tables.normalize_as(self.profile.fold, &before);
        self.profile.elisions.contains(&normalized.as_str()).then_some(at.saturating_add(1))
    }

    /// The next word, `text` as written at `offset`, of `length`
    /// characters, normalized and numbered.
    fn token(&mut self, tables: &Tables, text: &str, offset: u32, length: u32) -> Token {
        self.ordinal = self.ordinal.saturating_add(1);
        let normalized = tables.normalize_as(self.profile.fold, text);
        let stop = self.stop.contains(self.language, &normalized);
        Token { word: normalized, ordinal: self.ordinal, sentence: self.sentence, paragraph: self.paragraph, offset, length, stop }
    }
}

/// `n` as a `u32`, `u32::MAX` past it.
fn as_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Whether a character of `kind` continues a word that is not an
/// ideograph, or begins one.
fn continues(kind: Kind) -> bool {
    matches!(kind, Kind::Letter | Kind::Number | Kind::Mark)
}

/// Whether a character of `kind` after `word` may join it to what follows:
/// connector punctuation, or an apostrophe after a letter, but never after
/// an ideograph, which is a word by itself.
fn joiner(kind: Kind, word: &Word) -> bool {
    !word.ideograph && (kind == Kind::Connector || (kind == Kind::Apostrophe && word.letter))
}

/// Whether a joiner of `joiner` kind joins the word to a character of
/// `kind`: a connector to any word character, an apostrophe to a letter.
fn joins(joiner: Kind, kind: Kind) -> bool {
    if joiner == Kind::Apostrophe { kind == Kind::Letter } else { continues(kind) }
}
