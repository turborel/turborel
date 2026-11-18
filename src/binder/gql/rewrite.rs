//! \[EXTENSION\] A GQL expression written again as SQL over the columns of
//! a binding table (RULES.md D-GQL): its tokens copied, but for those of
//! its variables.
//!
//! An element variable `x` is its identity's column, `x.p` its property's
//! column, or in an open graph the property read from its JSON map. `x : L`
//! and `x IS [NOT] LABELED L` test the label set column. ELEMENT_ID,
//! VERTEX_ID, EDGE_ID, LABELS, PATH_LENGTH and EDGES of a variable, and
//! COUNT of a group variable, are the columns that hold them. Every other
//! token is copied as written, so the expression keeps SQL's meaning.

use std::collections::BTreeMap;

use super::{Kind, Var, quote};
use crate::catalog::identifier;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, Token, delim, kw};

/// A GQL text and its tokens.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Text<'a> {
    /// The text's characters.
    pub(crate) chars: &'a [char],
    /// Its tokens, the last `Tok::End`.
    pub(crate) toks: &'a [Token],
}

impl Text<'_> {
    /// Token `i`'s kind; the end past the last.
    pub(super) fn tok(&self, i: usize) -> &Tok {
        self.toks.get(i).map_or(&Tok::End, |t| &t.tok)
    }

    /// Token `i` as written.
    pub(super) fn written(&self, i: usize) -> String {
        self.toks.get(i).map_or_else(String::new, |t| self.chars.get(t.start..t.end).unwrap_or_default().iter().collect())
    }

    /// Where token `i` begins.
    pub(super) fn at(&self, i: usize) -> usize {
        self.toks.get(i).map_or(0, |t| t.start)
    }

    /// The number of the token that begins at character `at`, if one does.
    pub(crate) fn token_at(&self, at: usize) -> Option<usize> {
        self.toks.iter().position(|t| t.start == at)
    }
}

/// The variables an expression sees: each, and the correlation name of
/// the table whose columns hold it.
pub(crate) type Scope<'a> = BTreeMap<String, (&'a Var, String)>;

/// [ISO/IEC 39075:2024 §20.9] The aggregate functions GQL and SQL share,
/// whose argument may be a group variable.
const COUNT: &str = "COUNT";
/// [Graph standard §6.9] The functions of elements and paths this lowering
/// reads from the binding table's columns.
const ELEMENT_FUNCTIONS: [&str; 6] = ["ELEMENT_ID", "VERTEX_ID", "EDGE_ID", "LABELS", "PATH_LENGTH", "EDGES"];
/// LABELED, of IS LABELED.
const LABELED: &str = "LABELED";

/// The tokens `from` up to `to` of `text`, written again as SQL in
/// `scope`.
///
/// # Errors
///
/// SQLCODE -206 of a property no element table of the variable has
/// (GRF-0012); -900 of a function of paths this lowering does not read.
pub(crate) fn rewrite(text: Text<'_>, (from, to): (usize, usize), scope: &Scope<'_>) -> Result<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = from;
    while i < to {
        let (written, next) = token(text, i, scope)?;
        out.push(written);
        i = next.max(i.saturating_add(1));
    }
    Ok(out.join(" "))
}

/// Token `i` and what follows it that it heads, written again: the text,
/// and the token after.
fn token(text: Text<'_>, i: usize, scope: &Scope<'_>) -> Result<(String, usize)> {
    let after_period = i > 0 && *text.tok(i.saturating_sub(1)) == Tok::Delimiter(delim::PERIOD);
    if let Some(function) = function_call(text, i, scope)? {
        return Ok(function);
    }
    let Tok::Identifier(name) = text.tok(i) else {
        return Ok((text.written(i), i.saturating_add(1)));
    };
    match scope.get(name).filter(|_| !after_period) {
        Some((var, q)) => variable(text, i, (var, q)),
        None => Ok((text.written(i), i.saturating_add(1))),
    }
}

/// A variable at token `i`, `var` held in the table `q`: its property, its
/// label test, or itself.
fn variable(text: Text<'_>, i: usize, (var, q): (&Var, &str)) -> Result<(String, usize)> {
    let next = i.saturating_add(1);
    if *text.tok(next) == Tok::Delimiter(delim::PERIOD)
        && let Tok::Identifier(p) = text.tok(next.saturating_add(1))
    {
        let after = next.saturating_add(2);
        let operand = comparison(text, after).then(|| after.saturating_add(1));
        return property(text, (var, q), p, operand).map(|s| (s, after));
    }
    if *text.tok(next) == Tok::Delimiter(delim::COLON) {
        return labels(text, next.saturating_add(1), &column(q, &var.labels_column()));
    }
    let is = *text.tok(next) == Tok::KeyWord(kw::IS);
    let not = is && *text.tok(next.saturating_add(1)) == Tok::KeyWord(kw::NOT);
    let word = if not { next.saturating_add(2) } else { next.saturating_add(1) };
    if is && *text.tok(word) == Tok::Identifier(LABELED.to_owned()) {
        let (cond, after) = labels(text, word.saturating_add(1), &column(q, &var.labels_column()))?;
        return Ok((if not { format!("NOT ({cond})") } else { cond }, after));
    }
    Ok((column(q, &var.name), next))
}

/// `q`'s column `name`.
pub(crate) fn column(q: &str, name: &str) -> String {
    format!("{q}.{}", quote(name))
}

/// Property `p` of `var`, held in `q`, compared with the operand that
/// begins at token `operand` if one is.
///
/// # Errors
///
/// SQLCODE -103 of a property no element table of the variable has
/// (GRF-0012), or of a group variable's (GRF-0022).
pub(crate) fn property(text: Text<'_>, (var, q): (&Var, &str), p: &str, operand: Option<usize>) -> Result<String> {
    match &var.kind {
        Kind::Element { properties: Some(props), .. } => {
            if props.iter().any(|(name, _)| name == p) {
                return Ok(column(q, &format!("{}.{p}", var.name)));
            }
            Err(SqlError::new(
                Code::SyntaxRule,
                format!("[EXTENSION] GRF-0012 no element table {} may bind to has a property {p}", var.name),
            ))
        }
        Kind::Element { properties: None, .. } => {
            let path = format!("lax $.{}", json_key(p));
            let returning = operand.and_then(|o| literal_type(text, o)).map_or_else(String::new, |t| format!(" RETURNING {t}"));
            Ok(format!("JSON_VALUE({}, '{}'{returning})", column(q, &var.props_column()), path.replace('\'', "''")))
        }
        Kind::Group => {
            Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0022 group variable {} is referenced as a singleton", var.name)))
        }
        Kind::Path | Kind::Value => Ok(format!("{} . {}", column(q, &var.name), identifier(p))),
    }
}

/// A key of an SQL/JSON path: `p` in double quotes.
fn json_key(p: &str) -> String {
    format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Whether a comparison operator is token `i`.
fn comparison(text: Text<'_>, i: usize) -> bool {
    matches!(
        text.tok(i),
        Tok::Delimiter(delim::EQUALS | delim::NOT_EQUALS | delim::LESS | delim::GREATER | delim::LESS_OR_EQUAL | delim::GREATER_OR_EQUAL)
    )
}

/// \[EXTENSION\] The SQL type an open graph's property is read as when
/// compared with the literal that begins at token `operand`: the
/// literal's; else none, and it is read as a character string.
fn literal_type(text: Text<'_>, operand: usize) -> Option<&'static str> {
    let operand = if *text.tok(operand) == Tok::Delimiter(delim::MINUS) { operand.saturating_add(1) } else { operand };
    match text.tok(operand) {
        Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_) => Some("DOUBLE PRECISION"),
        Tok::KeyWord(kw::TRUE | kw::FALSE) => Some("BOOLEAN"),
        Tok::KeyWord(_)
        | Tok::Identifier(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => None,
    }
}

/// A function of elements or paths at token `i`, if one is: its column,
/// and the token after its ")".
fn function_call(text: Text<'_>, i: usize, scope: &Scope<'_>) -> Result<Option<(String, usize)>> {
    let name = match text.tok(i) {
        Tok::Identifier(n) => n.clone(),
        Tok::KeyWord(k) => (*k).to_owned(),
        Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => return Ok(None),
    };
    let open = i.saturating_add(1);
    let (arg, close) = (open.saturating_add(1), open.saturating_add(2));
    let shaped = *text.tok(open) == Tok::Delimiter(delim::LEFT_PAREN) && *text.tok(close) == Tok::Delimiter(delim::RIGHT_PAREN);
    let Some((var, q)) = (if let Tok::Identifier(a) = text.tok(arg) { scope.get(a) } else { None }) else { return Ok(None) };
    if !shaped {
        return Ok(None);
    }
    let after = close.saturating_add(1);
    if name == COUNT && var.kind == Kind::Group {
        return Ok(Some((column(q, &var.count_column()), after)));
    }
    let held = match name.as_str() {
        "ELEMENT_ID" | "VERTEX_ID" | "EDGE_ID" | "EDGES" => var.name.clone(),
        "LABELS" => var.labels_column(),
        "PATH_LENGTH" => var.length_column(),
        other => {
            if crate::ast::GraphFunctionName::named(other).is_some() && !ELEMENT_FUNCTIONS.contains(&other) {
                return Err(SqlError::new(
                    Code::Unsupported,
                    format!("[EXTENSION] {other} is read by GRAPH_TABLE's path search, not by a GQL query's lowering (RULES.md D-GQL)"),
                ));
            }
            return Ok(None);
        }
    };
    Ok(Some((column(q, &held), after)))
}

/// A label expression from token `i`, its test of the label set held in
/// `set`: the SQL, and the token after it.
fn labels(text: Text<'_>, i: usize, set: &str) -> Result<(String, usize)> {
    let (mut cond, mut next) = label_term(text, i, set)?;
    while *text.tok(next) == Tok::Delimiter(delim::VERTICAL_BAR) {
        let (right, after) = label_term(text, next.saturating_add(1), set)?;
        cond = format!("({cond} OR {right})");
        next = after;
    }
    Ok((cond, next))
}

/// A label term: label factors joined by "&".
fn label_term(text: Text<'_>, i: usize, set: &str) -> Result<(String, usize)> {
    let (mut cond, mut next) = label_factor(text, i, set)?;
    while *text.tok(next) == Tok::Delimiter(delim::AMPERSAND) {
        let (right, after) = label_factor(text, next.saturating_add(1), set)?;
        cond = format!("({cond} AND {right})");
        next = after;
    }
    Ok((cond, next))
}

/// A label factor: "!" and a factor, "%", a label expression in
/// parentheses, or a label.
fn label_factor(text: Text<'_>, i: usize, set: &str) -> Result<(String, usize)> {
    let next = i.saturating_add(1);
    match text.tok(i) {
        Tok::Delimiter(delim::EXCLAMATION_MARK) => {
            let (inner, after) = label_factor(text, next, set)?;
            Ok((format!("NOT ({inner})"), after))
        }
        Tok::Delimiter(delim::PERCENT) => Ok((format!("({set} <> '{}')", super::LABEL_BAR), next)),
        Tok::Delimiter(delim::LEFT_PAREN) => {
            let (inner, after) = labels(text, next, set)?;
            Ok((inner, after.saturating_add(1)))
        }
        Tok::Identifier(label) => Ok((label_test(set, label), next)),
        Tok::KeyWord(_)
        | Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => Err(SqlError::at(Code::Syntax, text.at(i), "[EXTENSION] expected a label, %, ! or (")),
    }
}

/// Whether the label set `set`, its labels each between bars, holds
/// `label`.
pub(crate) fn label_test(set: &str, label: &str) -> String {
    let bar = super::LABEL_BAR;
    format!("(POSITION('{bar}{}{bar}' IN {set}) > 0)", label.replace('\'', "''"))
}

/// The name of `t` if it is an identifier.
pub(crate) fn identifier_of(t: &Tok) -> Option<&String> {
    match t {
        Tok::Identifier(n) => Some(n),
        Tok::KeyWord(_)
        | Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => None,
    }
}
