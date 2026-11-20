//! What each character of a document is to the tokenizer: part of a word,
//! an ideograph, a joiner, a sentence terminator, a closer, a blank or a
//! line terminator [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.1].
//!
//! The kinds come from the Unicode 13.0 General Category table the regular
//! expressions read (`regex/unicode.rs`) and the simple case folding and
//! canonical decompositions of the collation table
//! (`value/collation/table.rs`), each read once for the process.

use std::sync::OnceLock;

use super::language::Fold;
use crate::error::Result;
use crate::regex::class::CharSet;
use crate::regex::unicode::{CATEGORIES, Table};
use crate::value::collation::table::Table as Collation;
use crate::xquery::library::{Form, normalized};

// General Categories [Unicode 13.0 §4.5], by their names in `CATEGORIES`.

/// The letters.
const LETTERS: [&str; 5] = ["Lu", "Ll", "Lt", "Lm", "Lo"];
/// The other letters, among them the ideographs and kana.
const OTHER_LETTERS: [&str; 1] = ["Lo"];
/// The numbers.
const NUMBERS: [&str; 3] = ["Nd", "Nl", "No"];
/// The marks.
const MARKS: [&str; 3] = ["Mn", "Mc", "Me"];
/// The nonspacing marks, which normalization takes out.
const NONSPACING: [&str; 1] = ["Mn"];
/// The connector punctuation, which joins a word's parts.
const CONNECTORS: [&str; 1] = ["Pc"];
/// Closing brackets and closing quotes.
const CLOSERS: [&str; 2] = ["Pe", "Pf"];
/// The space separators.
const SPACES: [&str; 1] = ["Zs"];

/// The blocks whose other letters (Lo) are each a word of their own
/// [Unicode 13.0 Blocks.txt]: Hiragana, Katakana, Katakana Phonetic
/// Extensions, CJK Unified Ideographs Extension A, CJK Unified Ideographs,
/// CJK Compatibility Ideographs, the halfwidth katakana of Halfwidth and
/// Fullwidth Forms, Kana Supplement through Small Kana Extension, CJK
/// Unified Ideographs Extensions B to F, CJK Compatibility Ideographs
/// Supplement, and CJK Unified Ideographs Extension G.
const IDEOGRAPHS: [(u32, u32); 12] = [
    (0x3040, 0x309F),
    (0x30A0, 0x30FF),
    (0x31F0, 0x31FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0xFF66, 0xFF9D),
    (0x1_B000, 0x1_B16F),
    (0x2_0000, 0x2_A6DF),
    (0x2_A700, 0x2_EBEF),
    (0x2_F800, 0x2_FA1F),
    (0x3_0000, 0x3_134F),
];

/// The blocks of the scripts written without spaces between words, whose
/// letters a language read as bigrams takes for letters to pair
/// (RULES.md D323) [Unicode 13.0 Blocks.txt]: Thai, Lao, Myanmar, Hangul
/// Jamo, Khmer, Khmer Symbols, Bopomofo, Hangul Compatibility Jamo,
/// Bopomofo Extended, Hangul Jamo Extended-A, Myanmar Extended-B, Myanmar
/// Extended-A, Hangul Syllables, Hangul Jamo Extended-B, and the
/// halfwidth Hangul of Halfwidth and Fullwidth Forms.
const NO_SPACE_BLOCKS: [(u32, u32); 15] = [
    (0x0E00, 0x0E7F),
    (0x0E80, 0x0EFF),
    (0x1000, 0x109F),
    (0x1100, 0x11FF),
    (0x1780, 0x17FF),
    (0x19E0, 0x19FF),
    (0x3100, 0x312F),
    (0x3130, 0x318F),
    (0x31A0, 0x31BF),
    (0xA960, 0xA97F),
    (0xA9E0, 0xA9FF),
    (0xAA60, 0xAA7F),
    (0xAC00, 0xD7AF),
    (0xD7B0, 0xD7FF),
    (0xFFA0, 0xFFDC),
];

/// LATIN CAPITAL LETTER I, which Turkish folds to dotless `ı`.
const CAPITAL_I: char = 'I';
/// LATIN SMALL LETTER DOTLESS I.
const DOTLESS_I: char = '\u{131}';
/// LATIN CAPITAL LETTER I WITH DOT ABOVE, which Turkish folds to `i`.
const DOTTED_CAPITAL_I: char = '\u{130}';
/// LATIN SMALL LETTER I.
const SMALL_I: char = 'i';

// Characters the tokenizer treats apart from their category.

/// FULL STOP, a sentence terminator.
const FULL_STOP: char = '.';
/// EXCLAMATION MARK, a sentence terminator.
const EXCLAMATION: char = '!';
/// QUESTION MARK, a sentence terminator.
const QUESTION: char = '?';
/// IDEOGRAPHIC FULL STOP, a sentence terminator by itself.
const IDEOGRAPHIC_STOP: char = '\u{3002}';
/// FULLWIDTH EXCLAMATION MARK, a sentence terminator by itself.
const FULLWIDTH_EXCLAMATION: char = '\u{FF01}';
/// FULLWIDTH FULL STOP, a sentence terminator by itself.
const FULLWIDTH_STOP: char = '\u{FF0E}';
/// FULLWIDTH QUESTION MARK, a sentence terminator by itself.
const FULLWIDTH_QUESTION: char = '\u{FF1F}';
/// APOSTROPHE, which joins two letters.
pub(super) const APOSTROPHE: char = '\'';
/// RIGHT SINGLE QUOTATION MARK, written for an apostrophe.
pub(super) const RIGHT_QUOTE: char = '\u{2019}';
/// QUOTATION MARK, a closing quote after a terminator (it is Po).
const QUOTATION: char = '"';
/// CHARACTER TABULATION, a blank.
const TAB: char = '\t';
/// LINE FEED, a line terminator.
const LINE_FEED: char = '\n';
/// CARRIAGE RETURN, a line terminator; a LINE FEED right after it is part
/// of it.
const RETURN: char = '\r';
/// LINE TABULATION, a line terminator.
const LINE_TABULATION: char = '\u{B}';
/// FORM FEED, a line terminator.
const FORM_FEED: char = '\u{C}';
/// NEXT LINE, a line terminator.
const NEXT_LINE: char = '\u{85}';
/// LINE SEPARATOR, a line terminator.
const LINE_SEPARATOR: char = '\u{2028}';
/// PARAGRAPH SEPARATOR, a paragraph end by itself.
const PARAGRAPH_SEPARATOR: char = '\u{2029}';

/// What a character is to the tokenizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A letter (L*) but an ideograph.
    Letter,
    /// A number (N*).
    Number,
    /// A mark (M*).
    Mark,
    /// An ideograph or kana, a word by itself.
    Ideograph,
    /// Connector punctuation (Pc).
    Connector,
    /// An apostrophe, U+0027 or U+2019.
    Apostrophe,
    /// A sentence terminator, ending the sentence if a separator follows.
    Terminator,
    /// An ideographic or full-width sentence terminator, ending the
    /// sentence by itself, as text in ideographs puts no space after it.
    WideTerminator,
    /// A closing bracket or quote.
    Closer,
    /// A space separator (Zs) or a tab.
    Blank,
    /// LINE FEED.
    LineFeed,
    /// CARRIAGE RETURN.
    Return,
    /// Another line terminator.
    Line,
    /// PARAGRAPH SEPARATOR.
    Paragraph,
    /// Anything else.
    Other,
}

/// The character sets and the collation table the tokenizer reads.
pub(super) struct Tables {
    letters: CharSet,
    others: CharSet,
    numbers: CharSet,
    marks: CharSet,
    nonspacing: CharSet,
    connectors: CharSet,
    closers: CharSet,
    spaces: CharSet,
    collation: Collation,
}

/// The tables, read once for the process.
static TABLES: OnceLock<Result<Tables>> = OnceLock::new();

/// The tables, read the first time they are asked for.
///
/// # Errors
///
/// SQLCODE -916 if a compiled-in table is damaged.
pub(super) fn tables() -> Result<&'static Tables> {
    TABLES.get_or_init(Tables::read).as_ref().map_err(Clone::clone)
}

/// The places in `CATEGORIES` of the categories `names`.
fn places(names: &[&str]) -> Vec<usize> {
    names.iter().filter_map(|name| CATEGORIES.iter().position(|c| c == name)).collect()
}

/// Whether `c` lies in one of the `IDEOGRAPHS` blocks.
fn in_ideograph_block(c: char) -> bool {
    in_blocks(c, &IDEOGRAPHS)
}

/// Whether `c` lies in one of `blocks`.
fn in_blocks(c: char, blocks: &[(u32, u32)]) -> bool {
    let code = u32::from(c);
    blocks.iter().any(|&(first, last)| first <= code && code <= last)
}

/// `c` as Turkish folds it before case folding: `I` to `ı`, `İ` to `i`.
fn turkish(c: char) -> char {
    match c {
        CAPITAL_I => DOTLESS_I,
        DOTTED_CAPITAL_I => SMALL_I,
        other => other,
    }
}

impl Tables {
    /// The tables, read from the compiled-in Unicode data.
    fn read() -> Result<Tables> {
        let table = Table::read()?;
        let set = |names: &[&str]| table.categories(&places(names));
        Ok(Tables {
            letters: set(&LETTERS),
            others: set(&OTHER_LETTERS),
            numbers: set(&NUMBERS),
            marks: set(&MARKS),
            nonspacing: set(&NONSPACING),
            connectors: set(&CONNECTORS),
            closers: set(&CLOSERS),
            spaces: set(&SPACES),
            collation: Collation::read()?,
        })
    }

    /// What `c` is.
    pub(super) fn kind(&self, c: char) -> Kind {
        match c {
            FULL_STOP | EXCLAMATION | QUESTION => Kind::Terminator,
            IDEOGRAPHIC_STOP | FULLWIDTH_EXCLAMATION | FULLWIDTH_STOP | FULLWIDTH_QUESTION => Kind::WideTerminator,
            APOSTROPHE | RIGHT_QUOTE => Kind::Apostrophe,
            QUOTATION => Kind::Closer,
            TAB => Kind::Blank,
            LINE_FEED => Kind::LineFeed,
            RETURN => Kind::Return,
            LINE_TABULATION | FORM_FEED | NEXT_LINE | LINE_SEPARATOR => Kind::Line,
            PARAGRAPH_SEPARATOR => Kind::Paragraph,
            _ => self.category_kind(c),
        }
    }

    /// What `c` is by its General Category, read from the tables or, for
    /// ASCII, known.
    pub(super) fn category_kind(&self, c: char) -> Kind {
        if c.is_ascii() { ascii_kind(c) } else { self.table_kind(c) }
    }

    /// What `c` is by its General Category in the tables.
    pub(super) fn table_kind(&self, c: char) -> Kind {
        if self.letters.contains(c) {
            return self.letter_kind(c);
        }
        if self.numbers.contains(c) {
            return Kind::Number;
        }
        if self.marks.contains(c) {
            return Kind::Mark;
        }
        if self.connectors.contains(c) {
            return Kind::Connector;
        }
        if self.closers.contains(c) {
            return Kind::Closer;
        }
        if self.spaces.contains(c) {
            return Kind::Blank;
        }
        Kind::Other
    }

    /// What the letter `c` is, an ideograph if it is an other letter of an
    /// ideograph or kana block.
    fn letter_kind(&self, c: char) -> Kind {
        if self.others.contains(c) && in_ideograph_block(c) {
            return Kind::Ideograph;
        }
        Kind::Letter
    }

    /// `word` normalized [§4.5], ASCII lowered without a lookup, as the
    /// tables would: each character simply case folded,
    /// canonically decomposed and folded again, nonspacing marks taken out,
    /// and U+2019 written as an apostrophe.
    pub(super) fn normalize(&self, word: &str) -> String {
        let mut out = String::with_capacity(word.len());
        let mut parts = Vec::new();
        for c in word.chars() {
            if c.is_ascii() {
                out.push(c.to_ascii_lowercase());
            } else {
                self.normalize_char(c, &mut parts, &mut out);
            }
        }
        out
    }

    /// `c` normalized by the tables, added to `out`; `parts` is room for
    /// its decomposition.
    pub(super) fn normalize_char(&self, c: char, parts: &mut Vec<char>, out: &mut String) {
        parts.clear();
        self.collation.decompose(self.collation.fold(c), parts);
        for &part in parts.iter() {
            let folded = self.collation.fold(part);
            if !self.nonspacing.contains(folded) {
                out.push(if folded == RIGHT_QUOTE { APOSTROPHE } else { folded });
            }
        }
    }
}

impl Tables {
    /// Whether `c` is a letter a language read as bigrams pairs: a letter
    /// of an ideograph or kana block, among them the prolonged sound mark,
    /// or of a script written without spaces (D323).
    pub(super) fn is_unit(&self, c: char) -> bool {
        self.letters.contains(c) && (in_ideograph_block(c) || in_blocks(c, &NO_SPACE_BLOCKS))
    }

    /// Whether `c` is a mark (M*).
    pub(super) fn is_mark(&self, c: char) -> bool {
        self.marks.contains(c)
    }

    /// `word` normalized as `fold` says [§4.5].
    pub(super) fn normalize_as(&self, fold: Fold, word: &str) -> String {
        match fold {
            Fold::Strip => self.normalize(word),
            Fold::Keep => self.composed(word, false, Form::Nfc),
            Fold::Turkish => self.composed(word, true, Form::Nfc),
            Fold::Width => self.composed(word, false, Form::Nfkc),
        }
    }

    /// `word` simply case folded, `I` and `İ` first as Turkish folds them
    /// if `dotted`, U+2019 written U+0027, then in the normalization
    /// form `form`. Turkish folds the word composed, so `I` and a combining
    /// dot above are `İ`.
    fn composed(&self, word: &str, dotted: bool, form: Form) -> String {
        let source = if dotted { normalized(word, Form::Nfc).unwrap_or_else(|_| word.to_owned()) } else { word.to_owned() };
        let folded: String = source
            .chars()
            .map(|c| {
                let c = self.collation.fold(if dotted { turkish(c) } else { c });
                if c == RIGHT_QUOTE { APOSTROPHE } else { c }
            })
            .collect();
        normalized(&folded, form).unwrap_or(folded)
    }
}

/// What the ASCII character `c`, not one `Tables::kind` matches by itself,
/// is by its General Category [Unicode 13.0 UnicodeData.txt], known so the
/// common case needs no lookup: letters (Lu, Ll), digits (Nd), LOW LINE
/// (Pc), the closing brackets (Pe), SPACE (Zs).
fn ascii_kind(c: char) -> Kind {
    match c {
        'a'..='z' | 'A'..='Z' => Kind::Letter,
        '0'..='9' => Kind::Number,
        LOW_LINE => Kind::Connector,
        RIGHT_PARENTHESIS | RIGHT_SQUARE_BRACKET | RIGHT_CURLY_BRACKET => Kind::Closer,
        SPACE => Kind::Blank,
        _ => Kind::Other,
    }
}

/// LOW LINE, connector punctuation.
const LOW_LINE: char = '_';
/// RIGHT PARENTHESIS, a closing bracket.
const RIGHT_PARENTHESIS: char = ')';
/// RIGHT SQUARE BRACKET, a closing bracket.
const RIGHT_SQUARE_BRACKET: char = ']';
/// RIGHT CURLY BRACKET, a closing bracket.
const RIGHT_CURLY_BRACKET: char = '}';
/// SPACE, a space separator.
const SPACE: char = ' ';
