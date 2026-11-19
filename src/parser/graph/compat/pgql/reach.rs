//! \[EXTENSION\] [GS §10.3] PGQL's reachability and path macros:
//!
//! ```text
//! <path macro> ::= PATH <macro name> AS <path pattern> [ WHERE <search condition> ]
//! <reachability> ::= -/ <reach> /-> | <-/ <reach> /- | -/ <reach> /-
//! <reach> ::= : <label expression> [ <quantifier> ]
//! ```
//!
//! A reachability asks only whether its two nodes are joined by a path
//! whose edges are of the label expression, as many as the quantifier
//! says, one if none is written. It binds no variable, and two nodes it
//! joins match once however many such paths there are. So a path
//! pattern holding one is split there: what is on each side of it is a
//! path pattern of its own, and the reachability is a path pattern ANY
//! from its first node to its last, the edge pattern quantified between
//! them. The three are joined by their shared nodes, which are given a
//! variable where none is written.
//!
//! A label expression that is the name of a path macro the query defines
//! repeats the macro's pattern instead of an edge: its nodes the ends of
//! each repetition, its WHERE a condition of each. The macro's variables
//! are its own, each `v` renamed `v$n` at the use written at character
//! `n`, so they are neither the query's nor those of another use.

use crate::ast::{
    EdgePattern, ElementPattern, GraphVariable, LabelExpression, Orientation, ParenthesizedPath, PathExpression, PathFactor, PathPattern,
    PathPrefix, PathPrimary, PathTerm, Quantifier, Selector,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, Token, delim, kw};
use crate::parser::{Parser, exposure};

/// PATH, of a path macro.
const PATH: &str = "PATH";

/// A path macro: its name, the tokens of its pattern and WHERE clause,
/// and its variables.
pub(super) struct Macro {
    /// Its name.
    name: String,
    /// Its pattern and WHERE clause, as written, ended by `Tok::End`.
    toks: Vec<Token>,
    /// The names of the variables its pattern declares.
    vars: Vec<String>,
}

/// A reachability, read: its label expression, quantifier, orientation
/// and where it begins.
struct Reach {
    /// The label expression.
    label: LabelExpression,
    /// The quantifier, `{1}` if none is written.
    quantifier: Quantifier,
    /// The orientation of its edges.
    orientation: Orientation,
    /// Where it begins.
    at: usize,
}

impl Parser {
    /// Whether a path macro begins at offset `n`: PATH, a name and AS.
    fn at_path_macro(&self, n: usize) -> bool {
        let path = matches!(self.peek_at(n), Tok::Identifier(w) if w == PATH);
        path && matches!(self.peek_at(n.saturating_add(1)), Tok::Identifier(_))
            && *self.peek_at(n.saturating_add(2)) == Tok::KeyWord(kw::AS)
    }

    /// The number of tokens of the path macros beginning here, up to
    /// SELECT outside parentheses; 0 if none begins here.
    pub(super) fn macros_length(&self) -> usize {
        if !self.at_path_macro(0) {
            return 0;
        }
        let (mut depth, mut n) = (0_usize, 1_usize);
        while *self.peek_at(n) != Tok::End {
            if depth == 0 && *self.peek_at(n) == Tok::KeyWord(kw::SELECT) {
                return n;
            }
            depth = nesting(depth, self.peek_at(n));
            n = n.saturating_add(1);
        }
        0
    }

    /// The path macros beginning here, read.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if one does not conform to the Format; -103 if one
    /// has a path variable or a prefix, or two are named alike.
    pub(super) fn path_macros(&mut self) -> Result<Vec<Macro>> {
        let mut macros: Vec<Macro> = Vec::new();
        while self.at_path_macro(0) {
            self.advance();
            let at = self.here();
            let name = self.identifier()?;
            if macros.iter().any(|m| m.name == name) {
                return Err(SqlError::at(Code::SyntaxRule, at, format!("[GS §10.3] PATH {name} is defined twice")));
            }
            self.expect_kw(kw::AS)?;
            let start = self.pos;
            let path = self.in_pattern(Parser::path_pattern)?;
            if path.variable.is_some() || path.prefix.is_some() {
                return Err(SqlError::at(Code::SyntaxRule, at, format!("[GS §10.3] PATH {name} has no path variable or prefix")));
            }
            if self.accept_kw(kw::WHERE) {
                self.in_pattern(Parser::search_condition)?;
            }
            let mut toks: Vec<Token> = self.toks.get(start..self.pos).unwrap_or_default().to_vec();
            let end = toks.last().map_or(at, |t| t.end);
            toks.push(Token { tok: Tok::End, start: end, end });
            let pattern =
                crate::ast::GraphPattern { mode: None, paths: vec![path], keep: None, where_clause: None, segments: Vec::new(), at };
            let vars = exposure(&pattern)?.into_keys().collect();
            macros.push(Macro { name, toks, vars });
        }
        Ok(macros)
    }

    /// A `<path pattern>` of a PGQL match, of the query's `macros`: as
    /// GPML's, else split at its reachabilities, as this module says.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format, or a
    /// reachability is not between two node patterns.
    pub(super) fn pgql_path(&mut self, macros: &[Macro]) -> Result<Vec<PathPattern>> {
        self.graph = self.graph.saturating_add(1);
        let read = if self.reach_ahead() { self.reaching_path(macros) } else { self.path_pattern().map(|p| vec![p]) };
        self.graph = self.graph.saturating_sub(1);
        read
    }

    /// Whether the path pattern beginning here holds a reachability: `-/`
    /// outside parentheses, brackets and braces, before a comma or key
    /// word outside them.
    fn reach_ahead(&self) -> bool {
        let (mut depth, mut n) = (0_usize, 0_usize);
        while *self.peek_at(n) != Tok::End {
            let t = self.peek_at(n);
            if depth == 0 && (matches!(t, Tok::KeyWord(_)) || *t == Tok::Delimiter(delim::COMMA)) {
                return false;
            }
            if depth == 0 && *t == Tok::Delimiter(delim::MINUS) && *self.peek_at(n.saturating_add(1)) == Tok::Delimiter(delim::SOLIDUS) {
                return true;
            }
            depth = nesting(depth, t);
            n = n.saturating_add(1);
        }
        false
    }
}

/// The depth in parentheses, brackets and braces after token `t`, of
/// `depth` before it.
fn nesting(depth: usize, t: &Tok) -> usize {
    let any = |ds: [&'static str; 3]| ds.iter().any(|d| *t == Tok::Delimiter(d));
    if any([delim::LEFT_PAREN, delim::LEFT_BRACKET, delim::LEFT_BRACE]) {
        depth.saturating_add(1)
    } else if any([delim::RIGHT_PAREN, delim::RIGHT_BRACKET, delim::RIGHT_BRACE]) {
        depth.saturating_sub(1)
    } else {
        depth
    }
}

impl Parser {
    /// A path pattern holding a reachability, split as this module says.
    fn reaching_path(&mut self, macros: &[Macro]) -> Result<Vec<PathPattern>> {
        let at = self.here();
        if !self.is_delim(delim::LEFT_PAREN) {
            return Err(
                self.syntax("[GS §10.3] a path pattern of a reachability begins with a node pattern, of no path variable or prefix")
            );
        }
        let mut pieces: Vec<Vec<PathFactor>> = vec![Vec::new()];
        let mut reaches = Vec::new();
        loop {
            if let Some(reach) = self.reachability()? {
                reaches.push(reach);
                pieces.push(Vec::new());
            } else if self.at_path_primary() {
                let factor = self.path_factor()?;
                if let Some(piece) = pieces.last_mut() {
                    piece.push(factor);
                }
            } else {
                break;
            }
        }
        let mut paths = Vec::new();
        for (n, reach) in reaches.into_iter().enumerate() {
            let ends = (end_of(&mut pieces, n, false, reach.at)?, end_of(&mut pieces, n.saturating_add(1), true, reach.at)?);
            paths.push(self.reach_path(reach, ends, macros)?);
        }
        let whole = pieces.into_iter().map(|factors| PathPattern {
            variable: None,
            prefix: None,
            expression: PathExpression::Term(PathTerm { factors }),
            at,
        });
        Ok(whole.chain(paths).collect())
    }

    /// A `<reachability>`, if one begins here.
    fn reachability(&mut self) -> Result<Option<Reach>> {
        let at = self.here();
        let solidus = |n: usize| *self.peek_at(n) == Tok::Delimiter(delim::SOLIDUS);
        let left = self.is_delim(delim::LESS) && *self.peek_at(1) == Tok::Delimiter(delim::MINUS) && solidus(2);
        let plain = self.is_delim(delim::MINUS) && solidus(1);
        if !left && !plain {
            return Ok(None);
        }
        for _ in 0..if left { 3_usize } else { 2_usize } {
            self.advance();
        }
        if !self.is_delim(delim::COLON) {
            return Err(self.syntax("[GS §10.3] a reachability is of a label expression after \":\", and binds no variable"));
        }
        self.advance();
        let label = self.label_expression()?;
        let quantifier = self.quantifier()?.unwrap_or(Quantifier { lower: 1, upper: Some(1), sugar: None, at });
        self.expect_delim(delim::SOLIDUS)?;
        let right = self.accept_delim(delim::RIGHT_ARROW);
        if !right {
            self.expect_delim(delim::MINUS)?;
        }
        let orientation = match (left, right) {
            (true, false) => Orientation::Left,
            (false, true) => Orientation::Right,
            (false, false) => Orientation::Any,
            (true, true) => return Err(SqlError::at(Code::Syntax, at, "[GS §10.3] a reachability points one way or neither")),
        };
        Ok(Some(Reach { label, quantifier, orientation, at }))
    }

    /// The path pattern ANY of reachability `reach` from node `from` to
    /// node `to`, of the query's `macros`.
    fn reach_path(&mut self, reach: Reach, (from, to): (PathFactor, PathFactor), macros: &[Macro]) -> Result<PathPattern> {
        let at = reach.at;
        let named = if let LabelExpression::Label(name, _) = &reach.label { macros.iter().find(|m| m.name == *name) } else { None };
        let primary = if let Some(m) = named {
            PathPrimary::Parenthesized(Box::new(self.expanded(m, at)?))
        } else {
            let filler = ElementPattern { variable: None, label: Some(reach.label), where_clause: None, properties: Vec::new(), at };
            PathPrimary::Edge(EdgePattern { orientation: reach.orientation, abbreviated: false, filler })
        };
        let repeated = PathFactor { primary, quantifier: Some(reach.quantifier) };
        let prefix = Some(PathPrefix { selector: Some(Selector::Any), restrictor: None, at });
        Ok(PathPattern { variable: None, prefix, expression: PathExpression::Term(PathTerm { factors: vec![from, repeated, to] }), at })
    }

    /// Macro `m`'s pattern and WHERE clause in brackets, read again of
    /// its tokens, each of its variables `v` renamed `v$n` for its use
    /// written at `n`.
    fn expanded(&mut self, m: &Macro, n: usize) -> Result<ParenthesizedPath> {
        let mut toks = m.toks.clone();
        for k in 0..toks.len() {
            let after = k.checked_sub(1).and_then(|p| m.toks.get(p)).map(|t| &t.tok);
            let renamed = matches!(after, Some(Tok::Delimiter(delim::PERIOD | delim::COLON)) | Some(Tok::KeyWord(kw::IS)));
            if let Some(Token { tok: Tok::Identifier(v), .. }) = toks.get_mut(k)
                && !renamed
                && m.vars.contains(v)
            {
                *v = format!("{v}${n}");
            }
        }
        let saved = (std::mem::replace(&mut self.toks, toks), self.pos);
        self.pos = 0;
        let read = self.macro_body(n);
        (self.toks, self.pos) = saved;
        read
    }

    /// A macro's pattern and WHERE clause, read from its own tokens.
    fn macro_body(&mut self, at: usize) -> Result<ParenthesizedPath> {
        let path = self.path_pattern()?;
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.search_condition()?) } else { None };
        Ok(ParenthesizedPath { variable: None, prefix: None, expression: path.expression, where_clause, bracketed: true, at })
    }
}

/// The node pattern ending piece `n` of `pieces`, or beginning it if
/// `first`, as a node pattern of its variable only; one given a variable
/// if it has none, named after `at`, where the reachability is written,
/// as no regular identifier is: no `$n`, which would be anonymous.
///
/// # Errors
///
/// SQLCODE -102 if that is no node pattern: a reachability written at
/// `at` is not between two.
fn end_of(pieces: &mut [Vec<PathFactor>], n: usize, first: bool, at: usize) -> Result<PathFactor> {
    let piece = pieces.get_mut(n);
    let factor = piece.and_then(|p| if first { p.first_mut() } else { p.last_mut() });
    let Some(PathFactor { primary: PathPrimary::Node(node), .. }) = factor else {
        return Err(SqlError::at(Code::Syntax, at, "[GS §10.3] a reachability is between two node patterns"));
    };
    let name = format!("R${at}{}", if first { "B" } else { "A" });
    let variable = node.variable.get_or_insert(GraphVariable { name, at: node.at, anonymous: false }).clone();
    let bare = ElementPattern { variable: Some(variable), label: None, where_clause: None, properties: Vec::new(), at: node.at };
    Ok(PathFactor { primary: PathPrimary::Node(bare), quantifier: None })
}
