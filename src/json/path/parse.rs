//! An SQL/JSON path expression parsed [ISO/IEC 9075-2:2023 §9.46 F, SR3,
//! SR5, SR6], by recursive descent over its tokens (`json/path/lex.rs`):
//! the `<JSON path expression>`, its mode and its `<JSON path wff>`s.
//! Predicates are parsed in `json/path/parse/predicate.rs`.
//!
//! \[DESIGN\] Parentheses, filters, subscripts, signs and negations nest at
//! most `MAX_PATH_NESTING` deep around what is read, so the parser's recursion
//! is bounded; and each node of the tree is built only if no more than
//! `MAX_PATH_NESTING` nodes are on the way down from it, so that a chain of
//! operators or accessors, built by a loop, is bounded too (RULES.md
//! R-CF-6).

mod method;
mod predicate;

use super::ast::{Accessor, Expr, Mode, Operator, Path, Sign, Subscript};
use super::lex::{Lexeme, SYMBOLS, Symbol, Token, lex};
use super::{DOLLAR, FALSE, LAST, LAX, MAX_PATH_NESTING, NULL, STRICT, TO, TRUE, beyond, misplaced, syntax};
use crate::error::{Result, SqlError};
use crate::json::Item;
use crate::value::datetime::Clocked;

/// [§9.46 F] The operators of a `<JSON additive expression>`.
const ADDITIVE: [(Symbol, Operator); 2] = [(Symbol::Plus, Operator::Add), (Symbol::Minus, Operator::Subtract)];
/// [§9.46 F] The operators of a `<JSON multiplicative expression>`.
const MULTIPLICATIVE: [(Symbol, Operator); 3] =
    [(Symbol::Asterisk, Operator::Multiply), (Symbol::Solidus, Operator::Divide), (Symbol::Percent, Operator::Modulo)];
/// [§9.46 F] The signs of a `<JSON unary expression>`.
const SIGNS: [(Symbol, Sign); 2] = [(Symbol::Plus, Sign::Plus), (Symbol::Minus, Sign::Minus)];

/// What the text's end is shown as in an error.
const END: &str = "the end of the path expression";
/// What a `<JSON path primary>` is called in an error.
const PRIMARY: &str = "a <JSON path primary>";
/// What may follow the `<period>` of an accessor op, in an error.
const AFTER_PERIOD: &str = "a <JSON path key name>, a string literal, * or a <JSON method>";

/// `symbol` as written, quoted.
fn written(symbol: Symbol) -> String {
    // The period, the one symbol not in SYMBOLS, is lexed apart.
    let text = SYMBOLS.iter().find(|(_, s)| *s == symbol).map_or(".", |(text, _)| *text);
    format!("'{text}'")
}

/// `token` as an error shows what it found (RULES.md D24): a symbol as
/// written, an identifier as it is, a string literal's value quoted, a
/// number as its value; none, the text's end.
fn shown(token: Option<&Token>) -> String {
    match token {
        Some(Token::Symbol(symbol)) => written(*symbol),
        Some(Token::Identifier(name)) => name.clone(),
        Some(Token::String(string)) => format!("\"{string}\""),
        Some(Token::Number(item, _)) => item.as_scalar().map_or_else(String::new, |scalar| scalar.value().to_string()),
        None => END.to_owned(),
    }
}

/// A node parsed, and its height: how many nodes are on the longest way
/// down from it, itself counted.
struct Built<T> {
    node: T,
    height: usize,
}

/// `node` built over children the tallest of which is `below` high.
///
/// # Errors
///
/// SQLCODE -900 if it would be more than `MAX_PATH_NESTING` high.
fn built<T>(node: T, below: usize) -> Result<Built<T>> {
    let height = below.saturating_add(1);
    if height > MAX_PATH_NESTING {
        return Err(beyond("its syntax tree nests too deep"));
    }
    Ok(Built { node, height })
}

/// A path expression's tokens being parsed.
struct Parser<'a> {
    tokens: Vec<Lexeme>,
    at: usize,
    /// How many characters the text has: where its end is.
    end: usize,
    /// How many `<JSON path predicate>`s hold what is read \[SR5\].
    predicates: usize,
    /// How many `<JSON subscript>`s hold what is read \[SR6\].
    subscripts: usize,
    /// The SQL-session's clock, which a `<JSON datetime template>` reads
    /// by \[SR10h\].
    now: Clocked<'a>,
}

/// [§9.46 SR3] The `<JSON path expression>` that `text` is, by the
/// lexical rules of §9.45, its `<JSON datetime template>`s to read
/// character strings in the SQL-session `now` gives: its instant read only
/// if one takes the current year or month [§9.51 GR5].
///
/// # Errors
///
/// SQLCODE -101 if `text` is not of the path language's tokens
/// [§9.45 SR2]; SQLCODE -102 if it does not conform to `<JSON path
/// expression>` [§9.46 F]; SQLCODE -103 if `@` is not within a predicate
/// \[SR5\] or `last` not within a subscript \[SR6\], or if an item
/// method's precision is 0 or its scale above its precision [§6.1 SR23,
/// SR25]; SQLSTATE 2201S or 2201T if a `like_regex` pattern is no XQuery
/// regular expression or its flags no XQuery option flag \[SR7\]; SQLCODE
/// -103 if a `<JSON datetime template>` breaks SR10 or a Syntax Rule of
/// §9.52; SQLCODE -900 for a template of more digits of a second's
/// fraction than 6, an item method's precision past turborel's maximum, a
/// number out of every number's range, or a path nested deeper than
/// `MAX_PATH_NESTING`.
pub fn parse_path(text: &str, now: Clocked<'_>) -> Result<Path> {
    let mut parser = Parser { tokens: lex(text)?, at: 0, end: text.chars().count(), predicates: 0, subscripts: 0, now };
    let mode = parser.mode()?;
    let expr = parser.wff(0)?;
    if parser.peek().is_some() {
        return Err(parser.expected(END));
    }
    Ok(Path { mode, expr: expr.node })
}

impl Parser<'_> {
    /// The token next, if one is.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|lexeme| &lexeme.token)
    }

    /// The token after the one next, if one is.
    fn peek_second(&self) -> Option<&Token> {
        self.tokens.get(self.at.saturating_add(1)).map(|lexeme| &lexeme.token)
    }

    /// Where the token next begins, or the text's end.
    fn here(&self) -> usize {
        self.tokens.get(self.at).map_or(self.end, |lexeme| lexeme.at)
    }

    /// Past the token next.
    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// The token next, taken.
    fn next(&mut self) -> Option<Token> {
        let token = self.peek().cloned();
        self.bump();
        token
    }

    /// Whether the special symbol `symbol` is next.
    fn is(&self, symbol: Symbol) -> bool {
        matches!(self.peek(), Some(Token::Symbol(s)) if *s == symbol)
    }

    /// Whether `symbol` is next, taken if it is.
    fn accept(&mut self, symbol: Symbol) -> bool {
        let found = self.is(symbol);
        if found {
            self.bump();
        }
        found
    }

    /// Past `symbol`, which is to be next, else the error that it was
    /// expected.
    fn expect(&mut self, symbol: Symbol) -> Result<()> {
        if self.accept(symbol) { Ok(()) } else { Err(self.expected(&written(symbol))) }
    }

    /// Whether the identifier `word` is next.
    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Identifier(name)) if name == word)
    }

    /// Whether the identifier `word` is next, taken if it is.
    fn accept_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        if found {
            self.bump();
        }
        found
    }

    /// What of `table` the special symbol next is, if it is any.
    fn operator<T: Copy>(&self, table: &[(Symbol, T)]) -> Option<T> {
        table.iter().find(|(symbol, _)| self.is(*symbol)).map(|(_, value)| *value)
    }

    /// The error of a path that does not conform to the Format: at the
    /// token next, `expected` was to be.
    fn expected(&self, expected: &str) -> SqlError {
        syntax(self.here(), expected, &shown(self.peek()))
    }

    /// Into a construct `depth` deep around what follows.
    fn enter(depth: usize) -> Result<()> {
        if depth > MAX_PATH_NESTING {
            return Err(beyond("parentheses, filters, subscripts, signs and negations nest too deep"));
        }
        Ok(())
    }

    /// `<JSON path mode>`.
    fn mode(&mut self) -> Result<Mode> {
        if self.accept_word(STRICT) {
            return Ok(Mode::Strict);
        }
        if self.accept_word(LAX) {
            return Ok(Mode::Lax);
        }
        Err(self.expected("a <JSON path mode>, strict or lax"))
    }

    /// `<JSON path wff>`, `depth` deep: a `<JSON additive expression>`.
    fn wff(&mut self, depth: usize) -> Result<Built<Expr>> {
        let first = self.multiplicative(depth)?;
        self.additive_rest(first, depth)
    }

    /// The rest of a `<JSON additive expression>` whose first operand,
    /// `left`, is read.
    fn additive_rest(&mut self, left: Built<Expr>, depth: usize) -> Result<Built<Expr>> {
        let mut left = left;
        while let Some(op) = self.operator(&ADDITIVE) {
            self.bump();
            let right = self.multiplicative(depth)?;
            left = built(Expr::Arithmetic(Box::new(left.node), op, Box::new(right.node)), left.height.max(right.height))?;
        }
        Ok(left)
    }

    /// `<JSON multiplicative expression>`.
    fn multiplicative(&mut self, depth: usize) -> Result<Built<Expr>> {
        let first = self.unary(depth)?;
        self.multiplicative_rest(first, depth)
    }

    /// The rest of a `<JSON multiplicative expression>` whose first
    /// operand, `left`, is read.
    fn multiplicative_rest(&mut self, left: Built<Expr>, depth: usize) -> Result<Built<Expr>> {
        let mut left = left;
        while let Some(op) = self.operator(&MULTIPLICATIVE) {
            self.bump();
            let right = self.unary(depth)?;
            left = built(Expr::Arithmetic(Box::new(left.node), op, Box::new(right.node)), left.height.max(right.height))?;
        }
        Ok(left)
    }

    /// `<JSON unary expression>`: a sign binds less tightly than an
    /// accessor op, so it signs the whole `<JSON accessor expression>`.
    fn unary(&mut self, depth: usize) -> Result<Built<Expr>> {
        let Some(sign) = self.operator(&SIGNS) else { return self.accessor(depth) };
        self.bump();
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        let operand = self.unary(inner)?;
        built(Expr::Signed(sign, Box::new(operand.node)), operand.height)
    }

    /// `<JSON accessor expression>`.
    fn accessor(&mut self, depth: usize) -> Result<Built<Expr>> {
        let base = self.primary(depth)?;
        self.accessor_rest(base, depth)
    }

    /// The `<JSON accessor op>`s that follow `base`, applied to it in turn.
    fn accessor_rest(&mut self, base: Built<Expr>, depth: usize) -> Result<Built<Expr>> {
        let mut base = base;
        loop {
            let (op, height) = if self.accept(Symbol::Period) {
                (self.after_period()?, 0)
            } else if self.accept(Symbol::LeftBracket) {
                self.array_accessor(depth)?
            } else if self.accept(Symbol::Question) {
                self.filter(depth)?
            } else {
                return Ok(base);
            };
            base = built(Expr::Access(Box::new(base.node), op), base.height.max(height))?;
        }
    }

    /// `<JSON path primary>`.
    fn primary(&mut self, depth: usize) -> Result<Built<Expr>> {
        let at = self.here();
        let Some(token) = self.next() else { return Err(syntax(at, PRIMARY, END)) };
        match token {
            Token::Number(item, _) => built(Expr::Literal(item), 0),
            Token::String(string) => built(Expr::Literal(Item::Scalar(string.into())), 0),
            Token::Identifier(name) => self.variable(&name, at),
            Token::Symbol(symbol) => self.primary_symbol(symbol, depth, at),
        }
    }

    /// The `<JSON path primary>` that the special symbol `symbol`, at
    /// character `at` and read, begins: `@`, or a parenthesized wff.
    fn primary_symbol(&mut self, symbol: Symbol, depth: usize, at: usize) -> Result<Built<Expr>> {
        if symbol == Symbol::At {
            if self.predicates == 0 {
                return Err(misplaced(at, "[ISO/IEC 9075-2:2023 §9.46 SR5] @ is not within a <JSON path predicate>"));
            }
            return built(Expr::Current, 0);
        }
        if symbol != Symbol::LeftParen {
            return Err(syntax(at, PRIMARY, &written(symbol)));
        }
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        let wff = self.wff(inner)?;
        self.expect(Symbol::RightParen)?;
        Ok(wff)
    }

    /// The `<JSON path primary>` the identifier `name`, at character `at`
    /// and read, is [§9.45 SR4]: a variable, a literal, or `last`.
    fn variable(&self, name: &str, at: usize) -> Result<Built<Expr>> {
        if let Some(rest) = name.strip_prefix(DOLLAR) {
            let variable = if rest.is_empty() { Expr::Context } else { Expr::Variable(rest.to_owned()) };
            return built(variable, 0);
        }
        let literal = match name {
            TRUE => Item::Scalar(true.into()),
            FALSE => Item::Scalar(false.into()),
            NULL => Item::Null,
            LAST => return self.last(at),
            _ => return Err(syntax(at, PRIMARY, name)),
        };
        built(Expr::Literal(literal), 0)
    }

    /// `<JSON last subscript>`, at character `at`.
    fn last(&self, at: usize) -> Result<Built<Expr>> {
        if self.subscripts == 0 {
            return Err(misplaced(at, "[ISO/IEC 9075-2:2023 §9.46 SR6] last is not within a <JSON subscript>"));
        }
        built(Expr::Last, 0)
    }

    /// What follows the `<period>` of a `<JSON member accessor>`, a `<JSON
    /// wildcard member accessor>` or a `<JSON item method>`.
    fn after_period(&mut self) -> Result<Accessor> {
        let at = self.here();
        match self.next() {
            Some(Token::Identifier(name)) => self.key_or_method(name, at),
            Some(Token::String(key)) => Ok(Accessor::Member(key)),
            Some(Token::Symbol(Symbol::Asterisk)) => Ok(Accessor::AnyMember),
            other @ (Some(Token::Symbol(_) | Token::Number(..)) | None) => Err(syntax(at, AFTER_PERIOD, &shown(other.as_ref()))),
        }
    }

    /// The `<JSON path key name>` or `<JSON method>` that the identifier
    /// `name`, at character `at`, begins: a method if `(` follows it
    /// [NOTE 483].
    fn key_or_method(&mut self, name: String, at: usize) -> Result<Accessor> {
        if name.starts_with(DOLLAR) {
            return Err(syntax(at, AFTER_PERIOD, &name));
        }
        if !self.accept(Symbol::LeftParen) {
            return Ok(Accessor::Member(name));
        }
        self.method(&name, at).map(Accessor::Method)
    }

    /// `<JSON array accessor>` or `<JSON wildcard array accessor>`, its
    /// `[` read: the accessor, and the height of its tallest subscript.
    fn array_accessor(&mut self, depth: usize) -> Result<(Accessor, usize)> {
        if self.accept(Symbol::Asterisk) {
            self.expect(Symbol::RightBracket)?;
            return Ok((Accessor::AnyElement, 0));
        }
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        self.subscripts = self.subscripts.saturating_add(1);
        let (mut list, mut height) = (Vec::new(), 0);
        loop {
            let from = self.wff(inner)?;
            height = height.max(from.height);
            let to = if self.accept_word(TO) { Some(self.wff(inner)?) } else { None };
            height = height.max(to.as_ref().map_or(0, |last| last.height));
            list.push(Subscript { from: from.node, to: to.map(|last| last.node) });
            if !self.accept(Symbol::Comma) {
                break;
            }
        }
        if !self.accept(Symbol::RightBracket) {
            return Err(self.expected("',' or ']'"));
        }
        self.subscripts = self.subscripts.saturating_sub(1);
        Ok((Accessor::Subscripts(list), height))
    }

    /// `<JSON filter expression>`, its `?` read: the accessor, and the
    /// height of its predicate.
    fn filter(&mut self, depth: usize) -> Result<(Accessor, usize)> {
        self.expect(Symbol::LeftParen)?;
        let inner = depth.saturating_add(1);
        Parser::enter(inner)?;
        self.predicates = self.predicates.saturating_add(1);
        let predicate = self.predicate(inner)?;
        self.expect(Symbol::RightParen)?;
        self.predicates = self.predicates.saturating_sub(1);
        Ok((Accessor::Filter(Box::new(predicate.node)), predicate.height))
    }
}

#[cfg(test)]
mod tests;
