//! Reading a search pattern [ISO/IEC 13249-2:2002 (CD, SC32 N0640)
//! §5.3.1 Description 3]: the `<search expression>` grammar, its key words
//! in any case [§5.3.2, §5.1.2 Description 4], into a `Pattern`.
//!
//! The choices this standard leaves to the implementation, or where its
//! text disagrees with itself, are these.
//!
//! - A `<language specification>` is a name before a word's opening quote,
//!   `ENGLISH "word"`, in any case, a language the tokenizer knows
//!   (`Language::named`) [§5.3.1 Description 3c]. The default is the
//!   caller's.
//! - A `<token list>` of one word may be written without parentheses, as
//!   the example of §4.2.5.1 writes `NEAR "language"`, and either side of
//!   NEAR may be an `<expansion function invocation>`, as §5.3.1
//!   Description 5f reads it.
//! - Every document unit of §5.3.1 Description 3f is supported, singular
//!   or plural, after WITHIN and after IN SAME; CHARACTERS and WORDS make
//!   no context [§4.2.5.2].
//! - A stop word may stand anywhere a word may [§5.1.2 Description 3b]. A
//!   pattern every word of which is a stop word of its language is
//!   effectively empty, SQLSTATE 2FF03 [§6.4.2 Description 2a i, §6.6.2
//!   Description 5c].

use crate::error::{Code, Result, SqlError, SqlState};
use crate::fulltext::Language;
use crate::fulltext::pattern::lex::{
    ABOUT, AND, ANY, AS, BROADER, CHARACTERS, ESCAPE, EXPAND, FOR, FORM, FUZZY, IN, IS, Item, KEY_WORDS, LEVEL, LEVELS, LIKE, Lexeme,
    NARROWER, NEAR, NOT, OF, ORDER, PARAGRAPH, PARAGRAPHS, PREFERRED, RELATED, SAME, SENTENCE, SENTENCES, SOUNDS, STEMMED, SYNONYM, TERM,
    THESAURUS, TOP, WITHIN, WORDS, invalid, lex,
};
use crate::fulltext::pattern::literal::{Written, is_stop_word, literal};
use crate::fulltext::pattern::{Argument, Context, Expansion, Literal, MAX_DEPTH, Part, Pattern, Proximity, Relation, Thesaurus, Unit};

/// The rule a pattern not of the grammar breaks.
const GRAMMAR: &str = "[ISO/IEC 13249-2:2002 §5.3.1 Description 3]";

/// The pattern `text` writes, its words and phrases of language `default`
/// unless they name another [§5.3.1 Description 3c].
///
/// # Errors
///
/// SQLSTATE 2FF01 if it is not a `<search expression>` or breaks a rule of
/// §5.3.1 Description 3; 2FF02 if it names a language the tokenizer does
/// not know; 2FF03 if every word of it is a stop word of its language.
/// Each message names the character of the pattern it is about.
pub fn parse(text: &str, default: Language) -> Result<Pattern> {
    let end = text.chars().count().saturating_add(1);
    let mut parser = Parser { items: lex(text)?, next: 0, end, default, nesting: 0 };
    let (pattern, _) = parser.search_expression()?;
    if let Some(item) = parser.peek() {
        return Err(invalid(item.at, &format!("{GRAMMAR} the search expression ends before this")));
    }
    effectively_empty(&pattern)?;
    Ok(pattern)
}

/// Refuses a pattern every word of which is a stop word of its language.
///
/// # Errors
///
/// SQLSTATE 2FF03 [§6.4.2 Description 2a i, §6.6.2 Description 5c].
fn effectively_empty(pattern: &Pattern) -> Result<()> {
    let stop = |literal: &&Literal| {
        literal.parts.iter().all(|part| match part {
            Part::Word(mask) => is_stop_word(mask, literal.language),
            Part::Optional => true,
        })
    };
    if pattern.words().iter().all(stop) {
        let msg = "[ISO/IEC 13249-2:2002 §6.4.2 Description 2a i] every word of the search pattern is a stop word";
        return Err(SqlError::new(Code::Data, msg).with_state(SqlState::EFFECTIVELY_EMPTY_SEARCH_EXPRESSION));
    }
    Ok(())
}

/// A pattern read and the depth of its tree.
type Parsed = (Pattern, usize);

/// A pattern being read.
struct Parser {
    /// Its items.
    items: Vec<Item>,
    /// The index of the next item to read.
    next: usize,
    /// The position just past its last character.
    end: usize,
    /// The language of a word or phrase that names none.
    default: Language,
    /// The parentheses open around the item being read.
    nesting: usize,
}

impl Parser {
    /// The next item, if any is left.
    fn peek(&self) -> Option<&Item> {
        self.items.get(self.next)
    }

    /// The position of the next item, or of the end.
    fn at(&self) -> usize {
        self.peek().map_or(self.end, |item| item.at)
    }

    /// Reads the next item.
    fn advance(&mut self) {
        self.next = self.next.saturating_add(1);
    }

    /// Whether the next item is `lexeme`.
    fn is(&self, lexeme: &Lexeme) -> bool {
        self.peek().is_some_and(|item| item.lexeme == *lexeme)
    }

    /// Reads the next item if it is `lexeme`.
    fn eat(&mut self, lexeme: &Lexeme) -> bool {
        let found = self.is(lexeme);
        if found {
            self.advance();
        }
        found
    }

    /// Whether the next item is the key word `key`, in any case.
    fn is_key(&self, key: &str) -> bool {
        let Some(Item { lexeme: Lexeme::Name(name), .. }) = self.peek() else { return false };
        name.eq_ignore_ascii_case(key)
    }

    /// Reads the next item if it is the key word `key`.
    fn eat_key(&mut self, key: &str) -> bool {
        let found = self.is_key(key);
        if found {
            self.advance();
        }
        found
    }

    /// The error of the next item not being `what`.
    fn expected(&self, what: &str) -> SqlError {
        invalid(self.at(), &format!("{GRAMMAR} expected {what}"))
    }

    /// Reads the key word `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF01 if the next item is not it.
    fn expect_key(&mut self, key: &str) -> Result<()> {
        if self.eat_key(key) { Ok(()) } else { Err(self.expected(key)) }
    }

    /// Reads `lexeme`, called `what`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF01 if the next item is not it.
    fn expect(&mut self, lexeme: &Lexeme, what: &str) -> Result<()> {
        if self.eat(lexeme) { Ok(()) } else { Err(self.expected(what)) }
    }

    /// Reads a quoted item, called `what`.
    fn quoted(&mut self, what: &str) -> Result<String> {
        let Some(Item { lexeme: Lexeme::Quoted(text), .. }) = self.peek() else { return Err(self.expected(what)) };
        let text = text.clone();
        self.advance();
        Ok(text)
    }

    /// Reads an `<unsigned integer>`.
    fn number(&mut self) -> Result<u64> {
        let Some(Item { lexeme: Lexeme::Number(n), .. }) = self.peek() else { return Err(self.expected("an unsigned integer")) };
        let n = *n;
        self.advance();
        Ok(n)
    }

    /// The depth of a node over a tree of depth `depth`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF01 past `MAX_DEPTH`.
    fn deeper(&self, depth: usize, at: usize) -> Result<usize> {
        let depth = depth.saturating_add(1);
        if depth > MAX_DEPTH || self.nesting > MAX_DEPTH {
            return Err(invalid(at, &format!("[DESIGN] a search pattern may nest at most {MAX_DEPTH} deep")));
        }
        Ok(depth)
    }

    /// `<search expression>`: terms joined by `|`.
    fn search_expression(&mut self) -> Result<Parsed> {
        let (mut left, mut depth) = self.search_term()?;
        while self.is(&Lexeme::Bar) {
            let at = self.at();
            self.advance();
            let (right, right_depth) = self.search_term()?;
            depth = self.deeper(depth.max(right_depth), at)?;
            left = Pattern::Or(Box::new(left), Box::new(right));
        }
        Ok((left, depth))
    }

    /// `<search term>`: factors joined by `&`.
    fn search_term(&mut self) -> Result<Parsed> {
        let (mut left, mut depth) = self.search_factor()?;
        while self.is(&Lexeme::Ampersand) {
            let at = self.at();
            self.advance();
            let (right, right_depth) = self.search_factor()?;
            depth = self.deeper(depth.max(right_depth), at)?;
            left = Pattern::And(Box::new(left), Box::new(right));
        }
        Ok((left, depth))
    }

    /// `<search factor>`: `[ NOT ] <search primary>`.
    fn search_factor(&mut self) -> Result<Parsed> {
        let at = self.at();
        if !self.eat_key(NOT) {
            return self.search_primary();
        }
        let (inner, depth) = self.search_primary()?;
        Ok((Pattern::Not(Box::new(inner)), self.deeper(depth, at)?))
    }

    /// `<search primary>`.
    fn search_primary(&mut self) -> Result<Parsed> {
        let at = self.at();
        if self.eat(&Lexeme::LeftParen) {
            return self.parenthesized(at);
        }
        if self.eat_key(IS) {
            self.expect_key(ABOUT)?;
            return Ok((Pattern::About(self.word_or_phrase()?), 1));
        }
        let argument =
            if self.starts_expansion() { Argument::Expansion(self.expansion()?) } else { Argument::Literal(self.text_literal()?) };
        self.after_argument(argument, at)
    }

    /// What follows `(` at `at`: a `<search expression>` and `)`, or a
    /// list of literals before NEAR or IN SAME.
    fn parenthesized(&mut self, at: usize) -> Result<Parsed> {
        self.nesting = self.nesting.saturating_add(1);
        self.deeper(0, at)?;
        let (first, depth) = self.search_expression()?;
        // NEAR and IN SAME inside were read with the argument before them,
        // so only a comma begins a list here.
        let parsed = if self.is(&Lexeme::Comma) {
            let Pattern::Literal(first) = first else {
                return Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] a list holds only words and phrases"));
            };
            let mut literals = vec![first];
            while self.eat(&Lexeme::Comma) {
                literals.push(self.text_literal()?);
            }
            self.expect(&Lexeme::RightParen, ")")?;
            self.after_argument(Argument::List(literals), at)?
        } else {
            self.expect(&Lexeme::RightParen, ")")?;
            self.parenthesized_argument(first, depth, at)?
        };
        self.nesting = self.nesting.saturating_sub(1);
        Ok(parsed)
    }

    /// `( pattern )`, of depth `depth`, at `at`: a list of one literal
    /// before NEAR or IN SAME, else the pattern.
    fn parenthesized_argument(&mut self, pattern: Pattern, depth: usize, at: usize) -> Result<Parsed> {
        if !self.is_key(NEAR) && !self.is_key(IN) {
            return Ok((pattern, depth));
        }
        let Pattern::Literal(literal) = pattern else {
            return Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] a list holds only words and phrases"));
        };
        self.after_argument(Argument::List(vec![literal]), at)
    }

    /// What follows the argument `argument`, at `at`: NEAR, IN SAME, or
    /// nothing, when it is a primary by itself.
    fn after_argument(&mut self, argument: Argument, at: usize) -> Result<Parsed> {
        if self.eat_key(NEAR) {
            return self.proximity(argument, at);
        }
        if self.eat_key(IN) {
            self.expect_key(SAME)?;
            return self.context(argument);
        }
        match argument {
            Argument::Literal(literal) => Ok((Pattern::Literal(literal), 1)),
            Argument::Expansion(expansion) => Ok((Pattern::Expansion(expansion), 1)),
            Argument::List(_) => Err(invalid(at, "[ISO/IEC 13249-2:2002 §4.2.3.3] a list stands only before NEAR or IN SAME")),
        }
    }

    /// The rest of a `<Proximity expansion>` after NEAR, its first token
    /// list `left` at `at`.
    fn proximity(&mut self, left: Argument, at: usize) -> Result<Parsed> {
        token_list(&left, at)?;
        let right_at = self.at();
        let right = if self.eat(&Lexeme::LeftParen) {
            let mut literals = vec![self.text_literal()?];
            while self.eat(&Lexeme::Comma) {
                literals.push(self.text_literal()?);
            }
            self.expect(&Lexeme::RightParen, ")")?;
            Argument::List(literals)
        } else if self.starts_expansion() {
            Argument::Expansion(self.expansion()?)
        } else {
            Argument::Literal(self.text_literal()?)
        };
        token_list(&right, right_at)?;
        self.expect_key(WITHIN)?;
        let distance = self.number()?;
        let unit = self.unit()?;
        let in_order = if self.eat_key(IN) {
            true
        } else {
            self.expect_key(ANY)?;
            false
        };
        self.expect_key(ORDER)?;
        Ok((Pattern::Near(Proximity { left, right, distance, unit, in_order }), 1))
    }

    /// A document unit [§5.3.1 Description 3f].
    fn unit(&mut self) -> Result<Unit> {
        let units = [
            (CHARACTERS, Unit::Characters),
            (WORDS, Unit::Words),
            (SENTENCE, Unit::Sentences),
            (SENTENCES, Unit::Sentences),
            (PARAGRAPH, Unit::Paragraphs),
            (PARAGRAPHS, Unit::Paragraphs),
        ];
        let found = units.iter().find(|(key, _)| self.is_key(key)).map(|(_, unit)| *unit);
        let Some(unit) = found else { return Err(self.expected("CHARACTERS, WORDS, SENTENCES or PARAGRAPHS")) };
        self.advance();
        Ok(unit)
    }

    /// The rest of a `<context condition>` after IN SAME, its first
    /// argument `first`.
    fn context(&mut self, first: Argument) -> Result<Parsed> {
        let at = self.at();
        let unit = self.unit()?;
        if matches!(unit, Unit::Characters | Unit::Words) {
            return Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] a context is a SENTENCE or a PARAGRAPH"));
        }
        self.expect_key(AS)?;
        let mut arguments = vec![first, self.context_argument()?];
        while self.eat_key(AND) {
            arguments.push(self.context_argument()?);
        }
        Ok((Pattern::Context(Context { unit, arguments }), 1))
    }

    /// A `<context argument>`.
    fn context_argument(&mut self) -> Result<Argument> {
        if self.starts_expansion() {
            return Ok(Argument::Expansion(self.expansion()?));
        }
        if !self.eat(&Lexeme::LeftParen) {
            return Ok(Argument::Literal(self.text_literal()?));
        }
        let mut literals = vec![self.text_literal()?];
        while self.eat(&Lexeme::Comma) {
            literals.push(self.text_literal()?);
        }
        self.expect(&Lexeme::RightParen, ")")?;
        Ok(Argument::List(literals))
    }

    /// Whether an `<expansion function invocation>` begins next.
    fn starts_expansion(&self) -> bool {
        self.is_key(SOUNDS) || self.is_key(FUZZY) || self.is_key(THESAURUS)
    }

    /// An `<expansion function invocation>`.
    fn expansion(&mut self) -> Result<Expansion> {
        if self.eat_key(SOUNDS) {
            self.expect_key(LIKE)?;
            return Ok(Expansion::Soundex(self.plain_word()?));
        }
        if self.eat_key(FUZZY) {
            self.expect_key(FORM)?;
            self.expect_key(OF)?;
            return Ok(Expansion::Fuzzy(self.plain_word()?));
        }
        self.expect_key(THESAURUS)?;
        self.thesaurus().map(Expansion::Thesaurus)
    }

    /// The rest of a thesaurus expansion after THESAURUS.
    fn thesaurus(&mut self) -> Result<Thesaurus> {
        let name_at = self.at();
        let name = self.quoted("a thesaurus name in double quotes")?;
        if name.trim().is_empty() {
            return Err(invalid(name_at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 4] a thesaurus name must hold a character"));
        }
        self.expect_key(EXPAND)?;
        let relation = self.relation()?;
        self.expect_key(TERM)?;
        self.expect_key(OF)?;
        let at = self.at();
        let (of, escaped) = self.literal_written()?;
        if of.stemmed {
            return Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3a] a thesaurus expands no stemmed word or phrase"));
        }
        plain(&of, escaped, at)?;
        let counted = matches!(relation, Relation::Broader | Relation::Narrower);
        let levels = if counted && self.eat_key(FOR) { Some(self.levels()?) } else { None };
        Ok(Thesaurus { name, relation, of, levels })
    }

    /// The rest of `FOR n LEVELS` after FOR.
    fn levels(&mut self) -> Result<u64> {
        let n = self.number()?;
        if self.eat_key(LEVEL) || self.eat_key(LEVELS) { Ok(n) } else { Err(self.expected("LEVEL or LEVELS")) }
    }

    /// A thesaurus relation.
    fn relation(&mut self) -> Result<Relation> {
        let relations = [
            (BROADER, Relation::Broader),
            (NARROWER, Relation::Narrower),
            (SYNONYM, Relation::Synonym),
            (PREFERRED, Relation::Preferred),
            (RELATED, Relation::Related),
            (TOP, Relation::Top),
        ];
        let found = relations.iter().find(|(key, _)| self.is_key(key)).map(|(_, relation)| *relation);
        let Some(relation) = found else { return Err(self.expected("BROADER, NARROWER, SYNONYM, PREFERRED, RELATED or TOP")) };
        self.advance();
        Ok(relation)
    }

    /// The `<word>` of SOUNDS LIKE or FUZZY FORM OF: one word, no wild
    /// card, no escape [§5.3.1 Description 3d].
    fn plain_word(&mut self) -> Result<Literal> {
        let at = self.at();
        let word = self.word_or_phrase_written()?;
        if !word.0.is_word() {
            return Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] SOUNDS LIKE and FUZZY FORM OF take one word"));
        }
        plain(&word.0, word.1, at)?;
        Ok(word.0)
    }

    /// The `<word or phrase>` of IS ABOUT.
    fn word_or_phrase(&mut self) -> Result<Literal> {
        self.word_or_phrase_written().map(|(literal, _)| literal)
    }

    /// A `<word>` or `<phrase>`, not stemmed, and whether it has an escape.
    fn word_or_phrase_written(&mut self) -> Result<(Literal, bool)> {
        if self.is_key(STEMMED) || self.is_key(FORM) {
            return Err(self.expected("a word or phrase, not a stemmed one"));
        }
        self.literal_written()
    }

    /// A `<text literal>`.
    fn text_literal(&mut self) -> Result<Literal> {
        self.literal_written().map(|(literal, _)| literal)
    }

    /// A `<text literal>`, and whether it has an escape.
    fn literal_written(&mut self) -> Result<(Literal, bool)> {
        let stemmed = if self.eat_key(STEMMED) {
            self.expect_key(FORM)?;
            self.expect_key(OF)?;
            true
        } else if self.eat_key(FORM) {
            self.expect_key(OF)?;
            true
        } else {
            false
        };
        let language = self.language()?;
        let at = self.at();
        let text = self.quoted("a word or phrase in double quotes")?;
        let escape = if self.eat_key(ESCAPE) { Some(self.escape()?) } else { None };
        let literal = literal(Written { text: &text, at, escape }, language, stemmed)?;
        if stemmed {
            plain(&literal, escape.is_some(), at)?;
        }
        Ok((literal, escape.is_some()))
    }

    /// The `<escape representation character>` after ESCAPE.
    fn escape(&mut self) -> Result<char> {
        let at = self.at();
        let text = self.quoted("an escape character in double quotes")?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            (None, _) | (Some(_), Some(_)) => {
                Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3a] an escape specification is one character"))
            }
        }
    }

    /// A `<language specification>`, if one is next: a name, not a key
    /// word, before a quote. Else the default.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 if the tokenizer knows no language of that name.
    fn language(&mut self) -> Result<Language> {
        let Some(Item { lexeme: Lexeme::Name(name), at }) = self.peek() else { return Ok(self.default) };
        let quoted = matches!(self.items.get(self.next.saturating_add(1)), Some(Item { lexeme: Lexeme::Quoted(_), .. }));
        if !quoted || KEY_WORDS.iter().any(|key| key.eq_ignore_ascii_case(name)) {
            return Ok(self.default);
        }
        let language = Language::named(name).map_err(|_| {
            let msg = format!(
                "[ISO/IEC 13249-2:2002 §5.3.1 Description 3c] no language is named {name}, at character {at} of the search pattern"
            );
            SqlError::new(Code::Data, msg).with_state(SqlState::INVALID_LANGUAGE_SPECIFICATION)
        })?;
        self.advance();
        Ok(language)
    }
}

/// Refuses `list`, at `at`, as a `<token list>` unless each of its literals
/// is a word or a stemmed word [§5.3.1 Description 3, `<word
/// specification>`].
fn token_list(list: &Argument, at: usize) -> Result<()> {
    let words = match list {
        Argument::Literal(literal) => literal.is_word(),
        Argument::List(literals) => literals.iter().all(Literal::is_word),
        Argument::Expansion(_) => true,
    };
    if words { Ok(()) } else { Err(invalid(at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3] a token list holds words, not phrases")) }
}

/// Refuses `literal`, at `at`, if it has a wild card or, by `escaped`, an
/// escape [§5.3.1 Description 3d, 3e].
fn plain(literal: &Literal, escaped: bool, at: usize) -> Result<()> {
    let wild = literal.parts.iter().any(|part| match part {
        Part::Word(mask) => mask.has_wild_cards(),
        Part::Optional => true,
    });
    if wild || escaped {
        return Err(invalid(
            at,
            "[ISO/IEC 13249-2:2002 §5.3.1 Description 3d, 3e] a stemmed or expanded word takes no wild card or escape",
        ));
    }
    Ok(())
}
