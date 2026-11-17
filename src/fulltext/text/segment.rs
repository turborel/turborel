//! The segmenter: one character at a time, where words begin and end and
//! where sentences and paragraphs end [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §4.1, §5.1.5]. It holds at most the word being read and one
//! joiner, never the document.

use super::kind::{Kind, Tables};
use super::{Language, StopWords, Token};

/// The characters of a word kept in `Token::word` \[DESIGN\]. A longer run
/// keeps its first characters, so the memory a document takes is bounded,
/// while `Token::length` counts all of them.
pub const MAX_WORD_CHARS: u32 = 256;

/// The line terminators in a row, with only blanks between, that end a
/// paragraph.
const PARAGRAPH_LINES: u8 = 2;

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

/// The segmenter's state between characters.
#[derive(Debug)]
pub(super) struct Segmenter {
    language: Language,
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
        let seen = Seen { c, at, kind: tables.kind(c), after_return: std::mem::take(&mut self.after_return) };
        let Some((joiner, joiner_kind)) = self.joiner.take() else {
            return self.place(tables, seen);
        };
        if let Some(word) = self.word.as_mut().filter(|_| joins(joiner_kind, seen.kind)) {
            word.push(joiner);
            word.push(c);
            word.letter = seen.kind == Kind::Letter;
            return None;
        }
        let ended = self.end_word(tables);
        self.boundary(joiner_kind, false);
        self.open(seen);
        ended
    }

    /// The end of the document; the word it ends, if any.
    pub(super) fn finish(&mut self, tables: &Tables) -> Option<Token> {
        self.joiner = None;
        self.end_word(tables)
    }

    /// The character `seen`, no joiner pending.
    fn place(&mut self, tables: &Tables, seen: Seen) -> Option<Token> {
        let Seen { c, kind, .. } = seen;
        let Some(word) = self.word.as_mut() else {
            self.open(seen);
            return None;
        };
        let joined = if word.ideograph { kind == Kind::Mark } else { continues(kind) };
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
        self.open(seen);
        ended
    }

    /// The character `seen`, no word being read, beginning a word or taken
    /// as a boundary character.
    fn open(&mut self, seen: Seen) {
        let Seen { c, at, kind, after_return } = seen;
        let ideograph = kind == Kind::Ideograph;
        if !continues(kind) && !ideograph {
            self.boundary(kind, after_return);
            return;
        }
        if self.ordinal > 0 {
            self.apply();
        }
        self.pending = Boundary::None;
        self.terminated = false;
        self.lines = 0;
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

    /// The word being read, ended and normalized.
    fn end_word(&mut self, tables: &Tables) -> Option<Token> {
        let word = self.word.take()?;
        self.ordinal = self.ordinal.saturating_add(1);
        let normalized = tables.normalize(&word.text);
        let stop = self.stop.contains(self.language, &normalized);
        Some(Token {
            word: normalized,
            ordinal: self.ordinal,
            sentence: self.sentence,
            paragraph: self.paragraph,
            offset: word.start,
            length: word.length,
            stop,
        })
    }
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
