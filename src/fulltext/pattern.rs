//! A search pattern, an FT_Pattern value read [ISO/IEC 13249-2:2002 (CD,
//! SC32 N0640) §5.3.1]: the words, phrases, expansions and conditions a
//! FullText value's Contains method looks for, and what its Rank method
//! scores. A pattern is read once (`parse.rs`) and evaluated over a
//! document's tokens (`eval.rs`) or answered by a full-text index
//! (`index/fulltext/plan.rs`). Thesaurus expansions are replaced by the
//! terms they expand to before either (`Pattern::expanded`).

pub mod eval;
pub mod expand;
pub(crate) mod fuzzy;
mod lex;
mod literal;
pub mod parse;
pub(crate) mod soundex;
#[cfg(test)]
mod tests;

pub use eval::contains;
pub use expand::Terms;
pub use parse::parse;

use crate::fulltext::Language;

/// The name of the FullText method Contains [§5.1.2], the function a
/// full-text index answers.
pub const CONTAINS: &str = "CONTAINS";

/// The name of the FullText method Rank [§5.1.3].
pub const RANK: &str = "RANK";

/// \[DESIGN\] The deepest a pattern may nest, in operators and
/// parentheses, so that reading, expanding and evaluating it recurse a
/// bounded depth (RULES.md R-CF-6). A deeper pattern is refused with
/// SQLSTATE 2FF01.
pub const MAX_DEPTH: usize = 100;

/// A `<search expression>` [§5.3.1 Description 3].
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `<search expression> | <search term>`: either holds.
    Or(Box<Pattern>, Box<Pattern>),
    /// `<search term> & <search factor>`: both hold.
    And(Box<Pattern>, Box<Pattern>),
    /// `NOT <search primary>`.
    Not(Box<Pattern>),
    /// A `<text literal>`: a word or a phrase, perhaps stemmed.
    Literal(Literal),
    /// A `<Proximity expansion>`.
    Near(Proximity),
    /// An `<about expansion>`, IS ABOUT a word or a phrase.
    About(Literal),
    /// An `<expansion function invocation>`.
    Expansion(Expansion),
    /// A `<context condition>`.
    Context(Context),
}

/// A `<word>`, `<phrase>`, `<stemmed word>` or `<stemmed phrase>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Literal {
    /// Its language: as specified, else the default given to the parser
    /// [§5.3.1 Description 3c].
    pub language: Language,
    /// Whether it is a FORM OF, matched by stems [§5.3.1 Description 5d].
    pub stemmed: bool,
    /// Its parts: one word for a `<word>`, two or more for a `<phrase>`.
    pub parts: Vec<Part>,
}

/// A part of a word or phrase pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// A word, its wild cards `_` and `%` marked (`Mask`), escapes taken
    /// out.
    Word(Mask),
    /// `%` alone in a phrase: an optional word [§5.3.1 NOTE 13].
    Optional,
}

/// A word as written in a pattern: its characters, each a wild card or
/// not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask(pub Vec<MaskChar>);

/// A character of a `Mask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskChar {
    /// A character to match, as written.
    Literal(char),
    /// `_`: any one character.
    One,
    /// `%`: any characters, none too.
    Any,
}

/// A `<Proximity expansion>`: a word of one list within `distance` units
/// of a word of the other, in their order or either.
///
/// Each side is an `Argument`: a `<token list>` of words or stemmed words
/// (`Argument::List`), one such word written without parentheses
/// (`Argument::Literal`, as the example of §4.2.5.1 writes it), or an
/// expansion, which §5.3.1 Description 5f replaces by the words it
/// expands to.
#[derive(Debug, Clone, PartialEq)]
pub struct Proximity {
    /// `<token list1>`.
    pub left: Argument,
    /// `<token list2>`.
    pub right: Argument,
    /// `<distance>`.
    pub distance: u64,
    /// `<unit>`.
    pub unit: Unit,
    /// IN ORDER, else ANY ORDER.
    pub in_order: bool,
}

/// A document unit [§5.3.1 Description 3f].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// CHARACTERS.
    Characters,
    /// WORDS.
    Words,
    /// SENTENCE or SENTENCES.
    Sentences,
    /// PARAGRAPH or PARAGRAPHS.
    Paragraphs,
}

/// An `<expansion function invocation>`.
#[derive(Debug, Clone, PartialEq)]
pub enum Expansion {
    /// SOUNDS LIKE a word.
    Soundex(Literal),
    /// FUZZY FORM OF a word.
    Fuzzy(Literal),
    /// THESAURUS name EXPAND relation TERM OF a literal, perhaps FOR a
    /// count of levels.
    Thesaurus(Thesaurus),
}

/// A thesaurus expansion, before `Pattern::expanded` replaces it.
#[derive(Debug, Clone, PartialEq)]
pub struct Thesaurus {
    /// The thesaurus's name, as written.
    pub name: String,
    /// The relation followed.
    pub relation: Relation,
    /// The term expanded.
    pub of: Literal,
    /// FOR n LEVELS, of BROADER and NARROWER.
    pub levels: Option<u64>,
}

/// A thesaurus relation [§8].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// BROADER TERM.
    Broader,
    /// NARROWER TERM.
    Narrower,
    /// SYNONYM TERM.
    Synonym,
    /// PREFERRED TERM.
    Preferred,
    /// RELATED TERM.
    Related,
    /// TOP TERM.
    Top,
}

/// A `<context condition>`: its arguments all in one sentence or one
/// paragraph.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    /// SENTENCE or PARAGRAPH (`Unit::Sentences`, `Unit::Paragraphs`).
    pub unit: Unit,
    /// Two or more arguments, each a literal, a list of literals any of
    /// which may stand for it, or an expansion.
    pub arguments: Vec<Argument>,
}

/// A `<context argument>`.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    /// A `<text literal>`.
    Literal(Literal),
    /// A `<text literal list>`.
    List(Vec<Literal>),
    /// An `<expansion function invocation>`.
    Expansion(Expansion),
}

impl Pattern {
    /// Every literal of the pattern, in the order the pattern writes them:
    /// its words and phrases, those of its lists, and those its expansions
    /// start from. A full-text index lists the words it looks up from them.
    pub fn words(&self) -> Vec<&Literal> {
        let mut out = Vec::new();
        // An explicit stack, not recursion: a pattern built in code may nest
        // deeper than `MAX_DEPTH`. Children are pushed last first, so they
        // come off in the order written.
        let mut stack = vec![self];
        while let Some(pattern) = stack.pop() {
            match pattern {
                Pattern::Or(left, right) | Pattern::And(left, right) => {
                    stack.push(right);
                    stack.push(left);
                }
                Pattern::Not(inner) => stack.push(inner),
                Pattern::Literal(literal) | Pattern::About(literal) => out.push(literal),
                Pattern::Near(near) => {
                    near.left.push_words(&mut out);
                    near.right.push_words(&mut out);
                }
                Pattern::Expansion(expansion) => out.push(expansion.literal()),
                Pattern::Context(context) => context.arguments.iter().for_each(|a| a.push_words(&mut out)),
            }
        }
        out
    }
}

impl Expansion {
    /// The literal it starts from.
    pub fn literal(&self) -> &Literal {
        match self {
            Expansion::Soundex(literal) | Expansion::Fuzzy(literal) => literal,
            Expansion::Thesaurus(thesaurus) => &thesaurus.of,
        }
    }
}

impl Argument {
    /// Adds its literals to `out`, in order.
    fn push_words<'p>(&'p self, out: &mut Vec<&'p Literal>) {
        match self {
            Argument::Literal(literal) => out.push(literal),
            Argument::List(literals) => out.extend(literals),
            Argument::Expansion(expansion) => out.push(expansion.literal()),
        }
    }
}

impl Literal {
    /// Whether it is one word, not a phrase.
    pub fn is_word(&self) -> bool {
        matches!(self.parts.as_slice(), [Part::Word(_)])
    }
}

impl Mask {
    /// Whether any of its characters is a wild card.
    pub fn has_wild_cards(&self) -> bool {
        self.0.iter().any(|c| !matches!(c, MaskChar::Literal(_)))
    }

    /// Its characters as written, if none is a wild card.
    pub fn text(&self) -> Option<String> {
        self.0
            .iter()
            .map(|c| match c {
                MaskChar::Literal(c) => Some(*c),
                MaskChar::One | MaskChar::Any => None,
            })
            .collect()
    }
}
