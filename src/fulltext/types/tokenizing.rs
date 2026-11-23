//! The FullText methods that cut a value into its words or its segments
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.4-§5.1.8], as scalar
//! functions called `f(SELF [, unit])`: TOKENIZE(SELF),
//! TOKENIZEPOSITION(SELF, unit), SEGMENTIZE(SELF, unit),
//! TOKENIZEANDSTEM(SELF) and TOKENIZEPOSITIONANDSTEM(SELF).
//!
//! The words are the tokenizer's (`fulltext/text.rs`), in SELF's
//! language, normalized [§4.5], and stemmed by the methods that stem.
//!
//! The result types are those of the methods' own clauses. FullText_Token
//! is a character string [§7.1], so TOKENIZE and TOKENIZEANDSTEM give an
//! array of them [§5.1.4, §5.1.7 Definition]; FT_TokenPosition [§5.2.1] is
//! the row type (TOKEN, POSITION, CORRVAL) here, so TOKENIZEPOSITION and
//! TOKENIZEPOSITIONANDSTEM give an array of those rows [§5.1.5, §5.1.8
//! Definition]. The summary of §5.1.1 says FullText ARRAY for the last
//! two; the clauses' descriptions say tokens, which is followed.
//!
//! Implementation-defined choices [§5.1.4 Description 4-5, §5.1.5
//! Description 4c, 6, 10].
//!
//! - Stop words are left out of every result, so no two of them compare.
//!   Each word keeps its place among all the words, stop words counted,
//!   so distances stay right and the first word's place is one more than
//!   the leading stop words [§5.1.5 Description 7-9, §5.1.8 Description
//!   6-7].
//! - A unit is matched with its case not counting and trailing spaces
//!   dropped. TOKENIZEPOSITION takes the units of §5.1.5 Description 2
//!   alone, and gives null of any other [§5.1.5 Description 5b]: so WORD
//!   and CHARACTER give null. SEGMENTIZE takes SENTENCE and PARAGRAPH
//!   [§5.1.6 Description 2], and gives null of any other, as
//!   TOKENIZEPOSITION does, the draft naming no condition for it.
//! - Of CHARACTERS, a word's position is its first character's, from 1,
//!   and its CORRVAL its length in characters as written, so
//!   `t2.POSITION - t1.POSITION - t1.CORRVAL` counts the characters
//!   between them; of the other units CORRVAL is 0.
//! - A segment runs from the end of the one before it to the first white
//!   space after its last word, or to the next segment's first word, so
//!   its terminator and closing quotes stay with it; the last runs to the
//!   end. Each is trimmed of white space and is a FullText value in SELF's
//!   language. Only sentences and paragraphs holding a word are segments,
//!   as only they are numbered.

use std::sync::Arc;

use super::fulltext::parts;
use super::{FullTextType, full_text, is_type};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::fulltext::text::MAX_WORD_CHARS;
use crate::fulltext::value::FULLTEXT;
use crate::fulltext::{Language, Token, stem, tokenize};
use crate::functions::ScalarFunction;
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY};
use crate::value::{DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

// ---------------------------------------------------------------------------
// Names [ISO/IEC 13249-2:2002 §5.1.1 Definition, §5.1.5, §5.1.6, §5.2.1]
// ---------------------------------------------------------------------------

/// The name of `Tokenizing::Tokenize`.
pub const TOKENIZE: &str = "TOKENIZE";
/// The name of `Tokenizing::TokenizePosition`.
pub const TOKENIZE_POSITION: &str = "TOKENIZEPOSITION";
/// The name of `Tokenizing::Segmentize`.
pub const SEGMENTIZE: &str = "SEGMENTIZE";
/// The name of `Tokenizing::TokenizeAndStem`.
pub const TOKENIZE_AND_STEM: &str = "TOKENIZEANDSTEM";
/// The name of `Tokenizing::TokenizePositionAndStem`.
pub const TOKENIZE_POSITION_AND_STEM: &str = "TOKENIZEPOSITIONANDSTEM";

/// The unit of characters [§5.1.5 Description 2].
const CHARACTERS: &str = "CHARACTERS";
/// The unit of words [§5.1.5 Description 2].
const WORDS: &str = "WORDS";
/// The unit of a sentence [§5.1.5 Description 2, §5.1.6 Description 2].
const SENTENCE: &str = "SENTENCE";
/// The unit of sentences [§5.1.5 Description 2].
const SENTENCES: &str = "SENTENCES";
/// The unit of a paragraph [§5.1.5 Description 2, §5.1.6 Description 2].
const PARAGRAPH: &str = "PARAGRAPH";
/// The unit of paragraphs [§5.1.5 Description 2].
const PARAGRAPHS: &str = "PARAGRAPHS";

/// The field of FT_TokenPosition holding the word [§5.2.1].
pub const TOKEN_FIELD: &str = "TOKEN";
/// The field of FT_TokenPosition holding its place [§5.2.1].
pub const POSITION_FIELD: &str = "POSITION";
/// The field of FT_TokenPosition holding the distance's correction
/// [§5.2.1].
pub const CORRVAL_FIELD: &str = "CORRVAL";

// ---------------------------------------------------------------------------
// Implementation-dependent lengths [§5.1.4 DR1, §7.1 DR1]
// ---------------------------------------------------------------------------

/// FT_MaxArrayLength [§5.1.4 DR1], implementation-dependent: the greatest
/// maximum cardinality of an array.
pub const FT_MAX_ARRAY_LENGTH: u32 = MAX_ARRAY_CARDINALITY;

/// The most characters canonical decomposition makes of one character
/// [Unicode 13.0 UAX #15 §9, NFD]; simple case folding makes one.
const DECOMPOSED_CHARS: u32 = 4;

/// FT_MaxTokenLength [§7.1 DR1], implementation-dependent: the longest
/// normalized word, of a word's kept characters each decomposed.
pub const FT_MAX_TOKEN_LENGTH: u32 = MAX_WORD_CHARS.saturating_mul(DECOMPOSED_CHARS);

/// The type of POSITION and CORRVAL [§5.2.1].
const INTEGER: DataType = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };

/// The routines of this file, each a scalar function.
pub const TOKENIZING: [Tokenizing; 5] = [
    Tokenizing::Tokenize,
    Tokenizing::TokenizePosition,
    Tokenizing::Segmentize,
    Tokenizing::TokenizeAndStem,
    Tokenizing::TokenizePositionAndStem,
];

/// A FullText method that tokenizes.
#[derive(Debug, Clone, Copy)]
pub enum Tokenizing {
    /// TOKENIZE(SELF), its normalized words [§5.1.4].
    Tokenize,
    /// TOKENIZEPOSITION(SELF, unit), its words and their places in `unit`
    /// [§5.1.5].
    TokenizePosition,
    /// SEGMENTIZE(SELF, unit), its sentences or paragraphs [§5.1.6].
    Segmentize,
    /// TOKENIZEANDSTEM(SELF), its words' stems [§5.1.7].
    TokenizeAndStem,
    /// TOKENIZEPOSITIONANDSTEM(SELF), its words' stems and their places in
    /// words [§5.1.8].
    TokenizePositionAndStem,
}

/// A unit of text [§5.1.5 Description 2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unit {
    /// CHARACTERS.
    Characters,
    /// WORDS.
    Words,
    /// SENTENCE or SENTENCES.
    Sentences,
    /// PARAGRAPH or PARAGRAPHS.
    Paragraphs,
}

/// The text of a unit `v`, trailing spaces dropped and in upper case.
fn unit_text(v: &Value) -> Option<String> {
    if let Value::Char(name) = v { Some(name.trim_end_matches(' ').to_ascii_uppercase()) } else { None }
}

/// The unit TOKENIZEPOSITION takes that `v` names [§5.1.5 Description 2].
pub(super) fn position_unit(v: &Value) -> Option<Unit> {
    match unit_text(v)?.as_str() {
        CHARACTERS => Some(Unit::Characters),
        WORDS => Some(Unit::Words),
        SENTENCE | SENTENCES => Some(Unit::Sentences),
        PARAGRAPH | PARAGRAPHS => Some(Unit::Paragraphs),
        _ => None,
    }
}

/// The unit SEGMENTIZE takes that `v` names [§5.1.6 Description 2].
pub(super) fn segment_unit(v: &Value) -> Option<Unit> {
    match unit_text(v)?.as_str() {
        SENTENCE => Some(Unit::Sentences),
        PARAGRAPH => Some(Unit::Paragraphs),
        _ => None,
    }
}

/// The type of a FullText_Token value [§7.1].
fn token_type() -> DataType {
    DataType::varying(FT_MAX_TOKEN_LENGTH)
}

/// A field of FT_TokenPosition.
fn field(name: &str, data_type: DataType) -> Field {
    Field { name: name.to_owned(), data_type, unnamed: false }
}

/// The type of an FT_TokenPosition value [§5.2.1], a row type here.
pub(super) fn token_position_type() -> DataType {
    DataType::row(vec![field(TOKEN_FIELD, token_type()), field(POSITION_FIELD, INTEGER), field(CORRVAL_FIELD, INTEGER)])
}

/// The error of a call of `name` not of the arguments it takes.
fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-2:2002 §5.1.1] invalid arguments to {name}"))
}

/// An INTEGER value of `n`.
fn integer(n: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The array of `elements`.
///
/// # Errors
///
/// SQLSTATE 2202F if they are more than FT_MaxArrayLength.
fn array(elements: Vec<Value>) -> Result<Value> {
    if u32::try_from(elements.len()).is_ok_and(|n| n <= FT_MAX_ARRAY_LENGTH) {
        return Ok(Value::Array(elements.into()));
    }
    Err(SqlError::new(Code::Data, format!("[ISO/IEC 13249-2:2002 §5.1.4 DR1] more than {FT_MAX_ARRAY_LENGTH} elements"))
        .with_state(SqlState::ARRAY_DATA_RIGHT_TRUNCATION))
}

/// The words of `contents` in `language` that are no stop words, each
/// with its place [§5.1.5 Description 7-9].
fn words(language: Language, contents: &str) -> impl Iterator<Item = Result<Token>> + '_ {
    tokenize(contents.chars().map(Ok), language).filter(|t| t.as_ref().map_or(true, |t| !t.stop))
}

/// The word of `token`, stemmed if `stemmed`.
fn word_of(language: Language, token: &Token, stemmed: bool) -> Result<String> {
    if stemmed { stem(language, &token.word) } else { Ok(token.word.clone()) }
}

/// The words of `contents`, stemmed if `stemmed` [§5.1.4, §5.1.7].
fn tokens(language: Language, contents: &str, stemmed: bool) -> Result<Value> {
    let mut out = Vec::new();
    for token in words(language, contents) {
        out.push(Value::Char(word_of(language, &token?, stemmed)?.into()));
    }
    array(out)
}

/// The place of `token` in `unit` and its CORRVAL [§5.1.5 Description 4].
fn place(token: &Token, unit: Unit) -> (u32, u32) {
    match unit {
        Unit::Characters => (token.offset.saturating_add(1), token.length),
        Unit::Words => (token.ordinal, 0),
        Unit::Sentences => (token.sentence, 0),
        Unit::Paragraphs => (token.paragraph, 0),
    }
}

/// The FT_TokenPosition values of `contents` in `unit`, their words
/// stemmed if `stemmed` [§5.1.5, §5.1.8].
fn positions(language: Language, contents: &str, unit: Unit, stemmed: bool) -> Result<Value> {
    let mut out = Vec::new();
    for token in words(language, contents) {
        let token = token?;
        let (position, corrval) = place(&token, unit);
        let row = [Value::Char(word_of(language, &token, stemmed)?.into()), integer(position), integer(corrval)];
        out.push(Value::Row(Arc::from(row)));
    }
    array(out)
}

/// Where a sentence or paragraph's words are, in characters.
struct Span {
    /// Its ordinal.
    ordinal: u32,
    /// Its first word's first character.
    start: usize,
    /// Just after its last word's last character.
    end: usize,
}

/// The spans of the sentences or paragraphs of `contents` holding a word,
/// stop words too, in order.
fn spans(language: Language, contents: &str, unit: Unit) -> Result<Vec<Span>> {
    let mut out: Vec<Span> = Vec::new();
    for token in tokenize(contents.chars().map(Ok), language) {
        let token = token?;
        let ordinal = place(&token, unit).0;
        let start = usize::try_from(token.offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(usize::try_from(token.length).unwrap_or(usize::MAX));
        if let Some(last) = out.last_mut().filter(|s| s.ordinal == ordinal) {
            last.end = end;
        } else {
            out.push(Span { ordinal, start, end });
        }
    }
    Ok(out)
}

/// Where a segment whose last word ends at `end` ends, the next's first
/// word beginning at `next`: at the first white space between, else at
/// `next`.
fn boundary(chars: &[char], end: usize, next: usize) -> usize {
    chars.get(end..next).and_then(|gap| gap.iter().position(|c| c.is_whitespace())).map_or(next, |n| end.saturating_add(n))
}

/// The sentences or paragraphs of `contents`, as FullText values in
/// `language` [§5.1.6 Description 4].
fn segments(language: Language, contents: &str, unit: Unit) -> Result<Value> {
    let chars: Vec<char> = contents.chars().collect();
    let spans = spans(language, contents, unit)?;
    let mut out = Vec::with_capacity(spans.len());
    let mut from = 0;
    for (n, span) in spans.iter().enumerate() {
        let to = spans.get(n.saturating_add(1)).map_or(chars.len(), |next| boundary(&chars, span.end, next.start));
        let piece: String = chars.get(from..to).unwrap_or_default().iter().collect();
        out.push(full_text(language, piece.trim())?);
        from = to;
    }
    array(out)
}

impl Tokenizing {
    /// Whether it takes a unit after SELF.
    fn has_unit(self) -> bool {
        match self {
            Self::TokenizePosition | Self::Segmentize => true,
            Self::Tokenize | Self::TokenizeAndStem | Self::TokenizePositionAndStem => false,
        }
    }

    /// The result's type [§5.1.4-§5.1.8 Definition].
    fn output(self) -> DataType {
        let element = match self {
            Self::Tokenize | Self::TokenizeAndStem => token_type(),
            Self::TokenizePosition | Self::TokenizePositionAndStem => token_position_type(),
            Self::Segmentize => FullTextType::data_type(),
        };
        DataType::array(element, FT_MAX_ARRAY_LENGTH)
    }

    /// Of a value `contents` in `language`, the unit given if one is
    /// taken: null if that unit is no unit it takes [§5.1.5 Description
    /// 5b].
    fn apply(self, language: Language, contents: &str, unit: Option<&Value>) -> Result<Value> {
        match self {
            Self::Tokenize => tokens(language, contents, false),
            Self::TokenizeAndStem => tokens(language, contents, true),
            Self::TokenizePositionAndStem => positions(language, contents, Unit::Words, true),
            Self::TokenizePosition => unit.and_then(position_unit).map_or(Ok(Value::Null), |u| positions(language, contents, u, false)),
            Self::Segmentize => unit.and_then(segment_unit).map_or(Ok(Value::Null), |u| segments(language, contents, u)),
        }
    }
}

impl ScalarFunction for Tokenizing {
    fn name(&self) -> &'static str {
        match self {
            Self::Tokenize => TOKENIZE,
            Self::TokenizePosition => TOKENIZE_POSITION,
            Self::Segmentize => SEGMENTIZE,
            Self::TokenizeAndStem => TOKENIZE_AND_STEM,
            Self::TokenizePositionAndStem => TOKENIZE_POSITION_AND_STEM,
        }
    }

    /// SELF is a FullText value; the unit, if taken, a character string,
    /// a FullText_Token value [§7.1].
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fits = match (self.has_unit(), args) {
            (false, [text]) => is_type(text, FULLTEXT),
            (true, [text, unit]) => is_type(text, FULLTEXT) && unit.is_character(),
            _ => false,
        };
        if fits { Ok(self.output()) } else { Err(signature(self.name())) }
    }

    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        Some(if position == 0 { FullTextType::data_type() } else { token_type() })
    }

    /// Null of a null SELF or unit [§5.1.4 Description 2, §5.1.5
    /// Description 5]; an empty array of a value of no characters, as it
    /// has no words [§5.1.4 Description 3].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (text, unit) = match (self.has_unit(), args) {
            (false, [text]) => (text, None),
            (true, [text, unit]) => (text, Some(unit)),
            _ => return Err(signature(self.name())),
        };
        if text.is_null() || unit.is_some_and(Value::is_null) {
            return Ok(Value::Null);
        }
        let (language, contents) = parts(text)?;
        self.apply(language, contents, unit)
    }

    fn deterministic(&self) -> bool {
        true
    }
}
