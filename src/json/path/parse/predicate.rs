//! `<JSON path predicate>`s parsed [ISO/IEC 9075-2:2023 §9.46 F, SR7]:
//! the predicates of a `<JSON filter expression>`.
//!
//! A `(` where a predicate may begin opens a `<JSON delimited predicate>`,
//! a `<JSON unknown predicate>`, or a `<JSON path primary>` that begins
//! the wff of a comparison, a like_regex or a starts with predicate:
//! `($.a) == 1`. Which is not known until its `)`, so what it holds is
//! parsed as a predicate or a wff, whichever it is (`Opened`); a wff
//! inside is the primary of a wff that goes on after the `)`. So every
//! token is read once, with no backtracking.

use super::{Built, Parser, built};
use crate::error::Result;
use crate::json::Item;
use crate::json::path::ast::{Comparison, Expr, Predicate};
use crate::json::path::lex::{Symbol, Token};
use crate::json::path::{DOLLAR, EXISTS, FLAG, IS, LIKE_REGEX, STARTS, UNKNOWN, WITH};
use crate::regex::Regex;

/// [§9.46 F] The `<JSON comp op>`s.
const COMPARISONS: [(Symbol, Comparison); 6] = [
    (Symbol::DoubleEquals, Comparison::Equal),
    (Symbol::NotEquals, Comparison::NotEqual),
    (Symbol::Less, Comparison::Less),
    (Symbol::Greater, Comparison::Greater),
    (Symbol::LessEquals, Comparison::LessOrEqual),
    (Symbol::GreaterEquals, Comparison::GreaterOrEqual),
];
/// Why a wff alone is no predicate.
const NOT_A_PREDICATE: &str = "a <JSON comp op>, like_regex or starts with follows the <JSON path wff> of a predicate";

/// What a `(` in a predicate opens: a predicate, or a wff.
enum Opened {
    /// A predicate.
    Predicate(Built<Predicate>),
    /// A wff that no comparison, like_regex or starts with follows.
    Wff(Built<Expr>),
}

impl Parser {
    /// `<JSON path predicate>`, `depth` deep.
    pub(super) fn predicate(&mut self, depth: usize) -> Result<Built<Predicate>> {
        match self.predicate_or_wff(depth)? {
            Opened::Predicate(predicate) => Ok(predicate),
            Opened::Wff(_) => Err(self.invalid(NOT_A_PREDICATE)),
        }
    }

    /// A `<JSON path predicate>`, or a `<JSON path wff>` alone.
    fn predicate_or_wff(&mut self, depth: usize) -> Result<Opened> {
        let first = match self.negation_or_wff(depth)? {
            Opened::Predicate(first) => first,
            Opened::Wff(wff) => return Ok(Opened::Wff(wff)),
        };
        let mut left = self.conjunction_rest(first, depth)?;
        while self.accept(Symbol::DoubleBar) {
            let next = self.negation(depth)?;
            let right = self.conjunction_rest(next, depth)?;
            left = built(Predicate::Or(Box::new(left.node), Box::new(right.node)), left.height.max(right.height))?;
        }
        Ok(Opened::Predicate(left))
    }

    /// The rest of a `<JSON boolean conjunction>` whose first operand,
    /// `left`, is read.
    fn conjunction_rest(&mut self, left: Built<Predicate>, depth: usize) -> Result<Built<Predicate>> {
        let mut left = left;
        while self.accept(Symbol::DoubleAmpersand) {
            let right = self.negation(depth)?;
            left = built(Predicate::And(Box::new(left.node), Box::new(right.node)), left.height.max(right.height))?;
        }
        Ok(left)
    }

    /// `<JSON boolean negation>`.
    fn negation(&mut self, depth: usize) -> Result<Built<Predicate>> {
        match self.negation_or_wff(depth)? {
            Opened::Predicate(predicate) => Ok(predicate),
            Opened::Wff(_) => Err(self.invalid(NOT_A_PREDICATE)),
        }
    }

    /// A `<JSON boolean negation>`, or a `<JSON path wff>` alone.
    fn negation_or_wff(&mut self, depth: usize) -> Result<Opened> {
        if self.accept(Symbol::Exclamation) {
            let inner = depth.saturating_add(1);
            Parser::enter(inner)?;
            let operand = self.delimited(inner)?;
            return built(Predicate::Not(Box::new(operand.node)), operand.height).map(Opened::Predicate);
        }
        if self.is_exists() {
            return self.exists(depth).map(Opened::Predicate);
        }
        if self.accept(Symbol::LeftParen) {
            return self.opened(depth);
        }
        let wff = self.wff(depth)?;
        self.after_wff(wff, depth)
    }

    /// `<JSON delimited predicate>`, after a `!`.
    fn delimited(&mut self, depth: usize) -> Result<Built<Predicate>> {
        if self.is_exists() {
            return self.exists(depth);
        }
        self.expect(Symbol::LeftParen, "a <JSON delimited predicate>, exists or '(', follows '!'")?;
        let predicate = self.predicate(depth)?;
        self.expect(Symbol::RightParen, "a ')' closes the '(' of a <JSON delimited predicate>")?;
        Ok(predicate)
    }

    /// Whether a `<JSON exists path predicate>` is next: `exists` and `(`.
    fn is_exists(&self) -> bool {
        self.is_word(EXISTS) && matches!(self.peek_second(), Some(Token::Symbol(Symbol::LeftParen)))
    }

    /// `<JSON exists path predicate>`, next.
    fn exists(&mut self, depth: usize) -> Result<Built<Predicate>> {
        self.bump();
        self.bump();
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        let wff = self.wff(inner)?;
        self.expect(Symbol::RightParen, "a ')' closes the <JSON path wff> of a <JSON exists path predicate>")?;
        built(Predicate::Exists(wff.node), wff.height)
    }

    /// What a `(` read opens: a `<JSON delimited predicate>` or `<JSON
    /// unknown predicate>`; or a `<JSON path primary>`, and the wff it
    /// begins, and the predicate that wff begins if one follows it.
    fn opened(&mut self, depth: usize) -> Result<Opened> {
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        let held = self.predicate_or_wff(inner)?;
        self.expect(Symbol::RightParen, "a ')' closes a '('")?;
        let predicate = match held {
            Opened::Predicate(predicate) => predicate,
            Opened::Wff(primary) => {
                let wff = self.accessor_rest(primary, depth)?;
                let wff = self.multiplicative_rest(wff, depth)?;
                let wff = self.additive_rest(wff, depth)?;
                return self.after_wff(wff, depth);
            }
        };
        if !self.accept_word(IS) {
            return Ok(Opened::Predicate(predicate));
        }
        if !self.accept_word(UNKNOWN) {
            return Err(self.invalid("unknown follows the is of a <JSON unknown predicate>"));
        }
        built(Predicate::IsUnknown(Box::new(predicate.node)), predicate.height).map(Opened::Predicate)
    }

    /// The `<JSON non-delimited predicate>` the wff `wff` begins, but a
    /// `<JSON unknown predicate>`; or the wff alone, if none follows.
    fn after_wff(&mut self, wff: Built<Expr>, depth: usize) -> Result<Opened> {
        if let Some(comparison) = self.operator(&COMPARISONS) {
            self.bump();
            let right = self.wff(depth)?;
            let height = wff.height.max(right.height);
            return built(Predicate::Compare(wff.node, comparison, right.node), height).map(Opened::Predicate);
        }
        if self.accept_word(LIKE_REGEX) {
            return self.like_regex(wff).map(Opened::Predicate);
        }
        if self.accept_word(STARTS) {
            return self.starts_with(wff).map(Opened::Predicate);
        }
        Ok(Opened::Wff(wff))
    }

    /// The rest of a `<JSON like_regex predicate>` of the wff `wff`, its
    /// `like_regex` read. \[SR7\] Its pattern is to be an XQuery regular
    /// expression, its flags an XQuery option flag: so it is compiled.
    fn like_regex(&mut self, wff: Built<Expr>) -> Result<Built<Predicate>> {
        let pattern = self.string_literal("a <JSON like_regex pattern>, a string literal, follows like_regex")?;
        // Without `flag`, the `<XQuery option flag>` is none: no letters.
        let flags = if self.accept_word(FLAG) {
            self.string_literal("<JSON like_regex flags>, a string literal, follow flag")?
        } else {
            String::new()
        };
        let regex = Regex::new(&pattern, &flags)?;
        built(Predicate::LikeRegex(wff.node, regex), wff.height)
    }

    /// The `<JSON path string literal>` next, taken; else the error `why`.
    fn string_literal(&mut self, why: &str) -> Result<String> {
        let Some(Token::String(string)) = self.peek().cloned() else { return Err(self.invalid(why)) };
        self.bump();
        Ok(string)
    }

    /// The rest of a `<JSON starts with predicate>` of the `<JSON starts
    /// with whole>` `wff`, its `starts` read.
    fn starts_with(&mut self, wff: Built<Expr>) -> Result<Built<Predicate>> {
        const INITIAL: &str = "a <JSON starts with initial>, a string literal or a named variable, follows starts with";
        if !self.accept_word(WITH) {
            return Err(self.invalid("with follows the starts of a <JSON starts with predicate>"));
        }
        let initial = match self.peek().cloned() {
            Some(Token::String(string)) => Expr::Literal(Item::Scalar(string.into())),
            Some(Token::Identifier(name)) => Expr::Variable(named_variable(&name).ok_or_else(|| self.invalid(INITIAL))?.to_owned()),
            Some(Token::Symbol(_) | Token::Number(_)) | None => return Err(self.invalid(INITIAL)),
        };
        self.bump();
        built(Predicate::StartsWith(wff.node, initial), wff.height.max(1))
    }
}

/// [§9.45 SR4b] The name of the `<JSON path named variable>` `identifier`
/// is, if it is one.
fn named_variable(identifier: &str) -> Option<&str> {
    identifier.strip_prefix(DOLLAR).filter(|name| !name.is_empty())
}
