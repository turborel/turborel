//! \[EXTENSION\] A GQL expression written again as SQL over the columns of
//! a binding table (RULES.md D-GQL): its tokens copied, but for those of
//! its variables.
//!
//! An element variable `x` is its identity's column, `x.p` its property's
//! column, or in an open graph the property read from its JSON map. `x : L`
//! and `x IS [NOT] LABELED L` test the label set column. ELEMENT_ID,
//! VERTEX_ID, EDGE_ID, LABELS, PATH_TO_JSON and each path function of a
//! variable are the columns that hold them. An aggregate of a group
//! variable, or of its property, is read across its elements in each row
//! from the arrays that hold them. Every other token is copied as
//! written, so the expression keeps SQL's meaning.
//!
//! In a MATCH's own pattern, written again as a GRAPH_TABLE's, the
//! pattern's variables are read by their own names (`NATIVE`), an open
//! graph's labels and properties from its LABELS and PROPS.

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

    /// Where token `i` ends.
    pub(super) fn end(&self, i: usize) -> usize {
        self.toks.get(i).map_or(0, |t| t.end)
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
/// whose argument may be a group variable, then read across its
/// elements in each row.
pub(super) const AGGREGATES: [&str; 9] = ["COUNT", "SUM", "AVG", "MIN", "MAX", "ARRAY_AGG", "COLLECT_LIST", "STDDEV_POP", "STDDEV_SAMP"];
/// COUNT, of a group variable its number of elements.
const COUNT: &str = "COUNT";
/// The aggregates of numbers, which read an open graph's property as one.
const NUMERIC: [&str; 4] = ["SUM", "AVG", "STDDEV_POP", "STDDEV_SAMP"];
/// The correlation name of a variable of a MATCH's own pattern, written
/// in its GRAPH_TABLE by its own name (`gql/pattern.rs`).
pub(crate) const NATIVE: &str = "";
/// LABELED, of IS LABELED.
const LABELED: &str = "LABELED";

/// The tokens `from` up to `to` of `text`, written again as SQL in
/// `scope`.
///
/// # Errors
///
/// SQLCODE -206 of a property no element table of the variable has
/// (GRF-0012); -103 of a path function of no path variable, or of a
/// group variable read as one element (GRF-0022).
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
pub(super) fn token(text: Text<'_>, i: usize, scope: &Scope<'_>) -> Result<(String, usize)> {
    let after_period = i > 0 && *text.tok(i.saturating_sub(1)) == Tok::Delimiter(delim::PERIOD);
    if let Some(function) = function_call(text, i, scope)? {
        return Ok(function);
    }
    let Tok::Identifier(name) = text.tok(i) else {
        return Ok((text.written(i), i.saturating_add(1)));
    };
    match scope.get(name).filter(|_| !after_period && !label_or_key(text, i)) {
        Some((var, q)) => variable(text, i, (var, q)),
        None => Ok((text.written(i), i.saturating_add(1))),
    }
}

/// Whether the identifier at token `i` names a label, after ":", "&",
/// "|", "!" or LABELED, or a property's key.
fn label_or_key(text: Text<'_>, i: usize) -> bool {
    let labelled = match i.checked_sub(1).map_or(&Tok::End, |j| text.tok(j)) {
        Tok::Delimiter(delim::AMPERSAND | delim::VERTICAL_BAR | delim::EXCLAMATION_MARK) => true,
        Tok::Delimiter(delim::COLON) => !i.checked_sub(2).is_some_and(|j| key(text, j)),
        Tok::Identifier(w) => w == LABELED,
        Tok::KeyWord(_)
        | Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::Delimiter(_)
        | Tok::End => false,
    };
    labelled || key(text, i)
}

/// Whether token `j` is a property's key: an identifier before ":", after
/// "{" or ",".
fn key(text: Text<'_>, j: usize) -> bool {
    let after = j.checked_sub(1).map_or(&Tok::End, |k| text.tok(k));
    matches!(text.tok(j), Tok::Identifier(_))
        && *text.tok(j.saturating_add(1)) == Tok::Delimiter(delim::COLON)
        && matches!(after, Tok::Delimiter(delim::LEFT_BRACE | delim::COMMA))
}

/// Whether `var` is of an open graph, its properties a JSON map's.
fn open(var: &Var) -> bool {
    matches!(var.kind, Kind::Element { properties: None, .. } | Kind::Group { properties: None })
}

/// A variable at token `i`, `var` held in the table `q`, or a pattern's
/// own if `q` is `NATIVE`: its property, its label test, or itself.
fn variable(text: Text<'_>, i: usize, (var, q): (&Var, &str)) -> Result<(String, usize)> {
    let next = i.saturating_add(1);
    if *text.tok(next) == Tok::Delimiter(delim::PERIOD)
        && let Tok::Identifier(p) = text.tok(next.saturating_add(1))
    {
        let after = next.saturating_add(2);
        let operand = comparison(text, after).then(|| after.saturating_add(1));
        return property(text, (var, q), p, operand).map(|s| (s, after));
    }
    let native = q == NATIVE;
    if native && !open(var) {
        return Ok((text.written(i), next));
    }
    let set = if native { format!("{}.\"LABELS\"", identifier(&var.name)) } else { column(q, &var.labels_column()) };
    if *text.tok(next) == Tok::Delimiter(delim::COLON) {
        return labels(text, next.saturating_add(1), &set);
    }
    let is = *text.tok(next) == Tok::KeyWord(kw::IS);
    let not = is && *text.tok(next.saturating_add(1)) == Tok::KeyWord(kw::NOT);
    let word = if not { next.saturating_add(2) } else { next.saturating_add(1) };
    if is && *text.tok(word) == Tok::Identifier(LABELED.to_owned()) {
        let (cond, after) = labels(text, word.saturating_add(1), &set)?;
        return Ok((if not { format!("NOT ({cond})") } else { cond }, after));
    }
    if native {
        return Ok((text.written(i), next));
    }
    Ok((column(q, &var.name), next))
}

/// `q`'s column `name`.
pub(crate) fn column(q: &str, name: &str) -> String {
    format!("{q}.{}", quote(name))
}

/// Property `p` of `var`, held in `q` or a pattern's own if `q` is
/// `NATIVE`, compared with the operand that begins at token `operand` if
/// one is.
///
/// # Errors
///
/// SQLCODE -103 of a property no element table of the variable has
/// (GRF-0012), or of a group variable's outside a pattern and an
/// aggregate (GRF-0022).
pub(crate) fn property(text: Text<'_>, (var, q): (&Var, &str), p: &str, operand: Option<usize>) -> Result<String> {
    let native = q == NATIVE;
    let own = || format!("{}.{}", identifier(&var.name), identifier(p));
    let map = || if native { format!("{}.\"PROPS\"", identifier(&var.name)) } else { column(q, &var.props_column()) };
    match &var.kind {
        Kind::Element { properties: Some(props), .. } => {
            known(var, props, p)?;
            Ok(if native { own() } else { column(q, &var.property_column(p)) })
        }
        Kind::Group { properties: Some(props) } => {
            known(var, props, p)?;
            if native { Ok(own()) } else { Err(singleton(var)) }
        }
        Kind::Element { properties: None, .. } => Ok(json_property(&map(), p, operand.and_then(|o| literal_type(text, o)))),
        Kind::Group { properties: None } => {
            if native {
                return Ok(json_property(&map(), p, operand.and_then(|o| literal_type(text, o))));
            }
            Err(singleton(var))
        }
        Kind::Path | Kind::Value => Ok(format!("{} . {}", column(q, &var.name), identifier(p))),
    }
}

/// That property `p` is one of `props`, `var`'s.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if it is not.
fn known(var: &Var, props: &[(String, String)], p: &str) -> Result<()> {
    if props.iter().any(|(name, _)| name == p) {
        return Ok(());
    }
    Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0012 no element table {} may bind to has a property {p}", var.name)))
}

/// The error of a group variable's property read as one value.
fn singleton(var: &Var) -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        format!("[EXTENSION] GRF-0022 group variable {} is referenced as a singleton, outside an aggregate of its elements", var.name),
    )
}

/// Property `p` of the JSON map `map`, read as SQL type `returning` if
/// given, else as a character string.
pub(super) fn json_property(map: &str, p: &str, returning: Option<&str>) -> String {
    let path = format!("lax $.{}", json_key(p));
    let returning = returning.map_or_else(String::new, |t| format!(" RETURNING {t}"));
    format!("JSON_VALUE({map}, '{}'{returning})", path.replace('\'', "''"))
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
pub(super) fn literal_type(text: Text<'_>, operand: usize) -> Option<&'static str> {
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

/// Token `t` as a word: an identifier, or a key word.
pub(super) fn word(t: &Tok) -> Option<String> {
    if let Tok::KeyWord(k) = t {
        return Some((*k).to_owned());
    }
    identifier_of(t).cloned()
}

/// A function of elements, paths or group variables at token `i`, if one
/// is: its column, or an aggregate across a group variable's elements,
/// and the token after its ")". A pattern's own variables' are written
/// as they are.
///
/// # Errors
///
/// SQLCODE -103 of a path function of no path variable (GRF-0022), or of
/// PREV or FIRST outside a pattern.
fn function_call(text: Text<'_>, i: usize, scope: &Scope<'_>) -> Result<Option<(String, usize)>> {
    let Some(name) = word(text.tok(i)) else { return Ok(None) };
    let open = i.saturating_add(1);
    if *text.tok(open) != Tok::Delimiter(delim::LEFT_PAREN) {
        return Ok(None);
    }
    let quantified = matches!(text.tok(open.saturating_add(1)), Tok::KeyWord(kw::DISTINCT | kw::ALL));
    let arg = if quantified { open.saturating_add(2) } else { open.saturating_add(1) };
    let Some((var, q)) = identifier_of(text.tok(arg)).and_then(|a| scope.get(a)) else { return Ok(None) };
    if q == NATIVE {
        return Ok(None);
    }
    if matches!(var.kind, Kind::Group { .. }) && AGGREGATES.contains(&name.as_str()) {
        let set = quantified.then(|| text.written(open.saturating_add(1)));
        return across(text, (&name, set), arg, (var, q)).map(Some);
    }
    let close = arg.saturating_add(1);
    if quantified || *text.tok(close) != Tok::Delimiter(delim::RIGHT_PAREN) {
        return Ok(None);
    }
    let held = match name.as_str() {
        "ELEMENT_ID" | "VERTEX_ID" | "EDGE_ID" | "PATH_TO_JSON" => var.name.clone(),
        "LABELS" => var.labels_column(),
        "PREV" | "FIRST" => {
            return Err(SqlError::at(Code::SyntaxRule, text.at(i), format!("[EXTENSION] {name} is read in a MATCH's pattern alone")));
        }
        other => match super::PATH_PARTS.iter().find(|(_, f)| *f == other) {
            Some((part, _)) => {
                if var.kind != Kind::Path {
                    return Err(super::misused(&var.name, "a path variable"));
                }
                var.part_column(part)
            }
            None => return Ok(None),
        },
    };
    Ok(Some((column(q, &held), close.saturating_add(1))))
}

/// The aggregate `name`, of set quantifier `quantifier` if written, of the
/// group variable `var` held in `q` at token `arg` or of its property:
/// read across its elements in each row (ISO/IEC 39075:2024 §20.9), and
/// the token after its ")". COUNT of the variable is its number.
///
/// # Errors
///
/// SQLCODE -101 if the argument does not end at ")"; -103 of a property
/// it has not (GRF-0012).
fn across(text: Text<'_>, (name, quantifier): (&str, Option<String>), arg: usize, (var, q): (&Var, &str)) -> Result<(String, usize)> {
    let element = "\"$u\".\"V\"".to_owned();
    let dotted = *text.tok(arg.saturating_add(1)) == Tok::Delimiter(delim::PERIOD);
    let property = identifier_of(text.tok(arg.saturating_add(2))).filter(|_| dotted);
    let (array, value, close) = if let Some(p) = property {
        let close = arg.saturating_add(3);
        if let Kind::Group { properties: Some(props) } = &var.kind {
            known(var, props, p)?;
            (column(q, &var.property_column(p)), element, close)
        } else {
            let returning = NUMERIC.contains(&name).then_some("DOUBLE PRECISION");
            (column(q, &var.props_column()), json_property(&element, p, returning), close)
        }
    } else {
        let close = arg.saturating_add(1);
        if name == COUNT && quantifier.is_none() && *text.tok(close) == Tok::Delimiter(delim::RIGHT_PAREN) {
            return Ok((column(q, &var.count_column()), close.saturating_add(1)));
        }
        (column(q, &var.name), element, close)
    };
    if *text.tok(close) != Tok::Delimiter(delim::RIGHT_PAREN) {
        return Err(SqlError::at(Code::Syntax, text.at(close), format!("[EXTENSION] expected \")\" closing {name}")));
    }
    let set = quantifier.map_or_else(String::new, |w| format!("{w} "));
    Ok((format!("(SELECT {name}({set}{value}) FROM UNNEST({array}) AS \"$u\"(\"V\"))"), close.saturating_add(1)))
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
