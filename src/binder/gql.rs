//! \[EXTENSION\] GQL lowered to SQL (graph standard §9 R9.1, R9.2;
//! RULES.md D-GQL): each GQL query becomes a `<query expression>` the SQL
//! binder binds, so GQL shares SQL's optimizer and executor.
//!
//! The lowering is [P2]'s linear composition (Theorem 5.1). A binding
//! table starts as the one-row table I∅, and each clause maps it to the
//! next: MATCH joins it with the pattern's matches (`gql/matching.rs`),
//! FILTER restricts it, LET and FOR widen it, CALL joins it with a
//! procedure's table, and RETURN projects it (`gql/linear.rs`,
//! `gql/results.rs`). NEXT feeds one statement's table to the next, and
//! the set operators combine tables by column name.
//!
//! MATCH is GRAPH_TABLE's: its pattern is written again as a
//! GRAPH_TABLE's, which the shared binder binds and path search runs, and
//! its COLUMNS give each variable's columns (`gql/matching.rs`,
//! `gql/pattern.rs`).
//!
//! Each variable is held in named columns of the binding table (`Var`):
//! an element's identity, ELEMENT_ID's text, its label set and its
//! properties; a path's elements as JSON, its length, vertices and edges
//! and what it is; a group variable's identities, number and each
//! property's values as arrays; a value. Expressions are written again
//! from their GQL tokens over these columns (`gql/rewrite.rs`).
//!
//! The SQL text is built here, then read and bound as any query is: its
//! names are all delimited, and its literals are written from the
//! catalog's names and the request's own tokens, so no value of a row
//! is ever spliced into it.

mod linear;
mod matching;
mod pattern;
mod results;
mod rewrite;
pub(crate) mod sources;
mod table;

use std::cell::Cell;

pub(crate) use linear::Table;
pub(crate) use rewrite::Text;
pub(crate) use table::{derived_of, graph_of};

use crate::ast::{GqlProcedure, TableName};
use crate::catalog::GraphDesc;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Token, lex};

/// What separates the labels of a label set held as text, `|A|B|`.
pub(crate) const LABEL_BAR: &str = "|";
/// The type an element's identity is held as, ELEMENT_ID's.
pub(crate) const ID_TYPE: &str = "VARCHAR(8192)";
/// The type an element's label set is held as.
pub(crate) const LABELS_TYPE: &str = "VARCHAR(1000)";
/// What a path variable's columns hold past its elements: its length,
/// its vertices and edges, and whether it is acyclic, a trail, simple;
/// each the path function's of GRAPH_TABLE that gives it [GS §6.9].
pub(crate) const PATH_PARTS: [(&str, &str); 6] = [
    ("$LENGTH", "PATH_LENGTH"),
    ("$VERTICES", "VERTICES"),
    ("$EDGES", "EDGES"),
    ("$ACYCLIC", "IS_ACYCLIC"),
    ("$TRAIL", "IS_TRAIL"),
    ("$SIMPLE", "IS_SIMPLE"),
];

/// The type of an array of values of type `t`, written.
pub(crate) fn array_of(t: &str) -> String {
    format!("{t} ARRAY[{}]", crate::value::types::MAX_ARRAY_CARDINALITY)
}

/// What a variable of a binding table is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// An element, a vertex or an edge: its properties and their SQL
    /// types, each its column; none in an open graph, whose properties
    /// are read from the element's JSON map.
    Element {
        /// Whether an edge.
        edge: bool,
        /// Its properties, or none of an open graph.
        properties: Option<Vec<(String, String)>>,
    },
    /// A path: its elements, its length, vertices and edges.
    Path,
    /// A value: of LET, FOR, RETURN or CALL.
    Value,
    /// A group variable of a quantified pattern: its elements, their
    /// number, and the values of each property, as arrays; none of an
    /// open graph, whose elements' JSON maps are held as an array.
    Group {
        /// Its properties, or none of an open graph.
        properties: Option<Vec<(String, String)>>,
    },
}

/// A variable of a binding table, held in its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Var {
    /// Its name, its first column's.
    pub(crate) name: String,
    /// What it is.
    pub(crate) kind: Kind,
}

impl Var {
    /// A value's variable.
    pub(crate) fn value(name: &str) -> Var {
        Var { name: name.to_owned(), kind: Kind::Value }
    }

    /// The column of an element's label set.
    pub(crate) fn labels_column(&self) -> String {
        format!("{}$LABELS", self.name)
    }

    /// The column of an open graph's element's JSON property map.
    pub(crate) fn props_column(&self) -> String {
        format!("{}$PROPS", self.name)
    }

    /// The column of a path's part `part`, of `PATH_PARTS`.
    pub(crate) fn part_column(&self, part: &str) -> String {
        format!("{}{part}", self.name)
    }

    /// The column of a group variable's number of elements.
    pub(crate) fn count_column(&self) -> String {
        format!("{}$COUNT", self.name)
    }

    /// The column of an element's property `p`.
    pub(crate) fn property_column(&self, p: &str) -> String {
        format!("{}.{p}", self.name)
    }

    /// Its columns, in order, each with its SQL type if known.
    pub(crate) fn columns(&self) -> Vec<(String, Option<String>)> {
        let typed = |c: String, t: &str| (c, Some(t.to_owned()));
        match &self.kind {
            Kind::Element { properties, .. } => {
                let mut all = vec![typed(self.name.clone(), ID_TYPE), typed(self.labels_column(), LABELS_TYPE)];
                match properties {
                    Some(props) => all.extend(props.iter().map(|(p, t)| typed(self.property_column(p), t))),
                    None => all.push(typed(self.props_column(), "JSON")),
                }
                all
            }
            Kind::Path => {
                let ids = array_of(ID_TYPE);
                let kinds = ["INTEGER", &ids, &ids, "BOOLEAN", "BOOLEAN", "BOOLEAN"];
                let parts = PATH_PARTS.iter().zip(kinds).map(|((part, _), t)| typed(self.part_column(part), t));
                std::iter::once(typed(self.name.clone(), "JSON")).chain(parts).collect()
            }
            Kind::Group { properties } => {
                let mut all = vec![typed(self.name.clone(), &array_of(ID_TYPE)), typed(self.count_column(), "BIGINT")];
                match properties {
                    Some(props) => all.extend(props.iter().map(|(p, t)| typed(self.property_column(p), &array_of(t)))),
                    None => all.push(typed(self.props_column(), &array_of("JSON"))),
                }
                all
            }
            Kind::Value => vec![(self.name.clone(), None)],
        }
    }

    /// The same variable named `name`.
    pub(crate) fn renamed(&self, name: &str) -> Var {
        Var { name: name.to_owned(), kind: self.kind.clone() }
    }
}

/// `name` as a delimited identifier, its quotes doubled.
pub(crate) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `s` as a character string literal, its quotes doubled.
pub(crate) fn string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A graph a GQL query reads: its property graph, and whether it is an
/// open GQL graph, whose elements' labels and properties its two tables
/// hold as text and JSON.
#[derive(Debug, Clone)]
pub(crate) struct Graph {
    /// Its descriptor.
    pub(crate) desc: GraphDesc,
    /// Whether open.
    pub(crate) open: bool,
    /// Whether a GQL graph of a graph type: closed.
    pub(crate) closed: bool,
}

/// How the lowering finds a graph: by its name, or the request's own if
/// none is named.
pub(crate) type Graphs<'a> = &'a dyn Fn(Option<&TableName>) -> Result<Graph>;

/// A GQL request's text and tokens, and the state of its lowering.
pub(crate) struct Lowering<'a> {
    /// The request's text.
    pub(crate) text: Text<'a>,
    /// How graphs are found.
    pub(crate) graphs: Graphs<'a>,
    /// The number of the next correlation name made.
    next: Cell<usize>,
}

impl<'a> Lowering<'a> {
    /// The lowering of the request `text`, its tokens `toks`.
    pub(crate) fn new(chars: &'a [char], toks: &'a [Token], graphs: Graphs<'a>) -> Lowering<'a> {
        Lowering { text: Text { chars, toks }, graphs, next: Cell::new(0) }
    }

    /// A correlation name of its own, delimited.
    pub(crate) fn alias(&self) -> String {
        let n = self.next.get();
        self.next.set(n.saturating_add(1));
        quote(&format!("$T{n}"))
    }

    /// A name of its own beginning `prefix`.
    pub(crate) fn fresh(&self, prefix: &str) -> String {
        let n = self.next.get();
        self.next.set(n.saturating_add(1));
        format!("{prefix}{n}")
    }

    /// A name of its own for an anonymous variable.
    pub(crate) fn anonymous(&self) -> String {
        let n = self.next.get();
        self.next.set(n.saturating_add(1));
        format!("$A{n}")
    }
}

/// A lowered query: its SQL text, and the variables its columns hold.
#[derive(Debug, Clone)]
pub(crate) struct Lowered {
    /// A `<query expression>`.
    pub(crate) sql: String,
    /// The variables of its columns, in order.
    pub(crate) vars: Vec<Var>,
}

impl Lowered {
    /// Its columns' names, in order.
    pub(crate) fn columns(&self) -> Vec<String> {
        self.vars.iter().flat_map(Var::columns).map(|(c, _)| c).collect()
    }
}

/// \[EXTENSION\] The GQL text `text` with each list literal `[...]`
/// written as SQL's array, `ARRAY[...]` (ISO/IEC 39075:2024 §21.2): a
/// "[" where an operand begins, after IN, "=", "(", ",", "||", a
/// comparison or RETURN. An edge's "[" follows "-", "<-" or "~".
pub(crate) fn gql_source(text: &str) -> Result<String> {
    let toks = lex(text)?;
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut from = 0;
    for (i, t) in toks.iter().enumerate() {
        let before = i.checked_sub(1).and_then(|p| toks.get(p)).map(|p| &p.tok);
        let operand = before.is_some_and(opens_operand);
        if t.tok == crate::lexer::Tok::Delimiter(crate::lexer::delim::LEFT_BRACKET) && operand {
            out.extend(chars.get(from..t.start).unwrap_or_default());
            out.push_str("ARRAY");
            from = t.start;
        }
    }
    out.extend(chars.get(from..).unwrap_or_default());
    Ok(out)
}

/// Whether an operand may begin after the token `t`.
fn opens_operand(t: &crate::lexer::Tok) -> bool {
    use crate::lexer::{Tok, delim, kw};
    match t {
        Tok::KeyWord(k) => *k == kw::IN,
        Tok::Identifier(w) => w == "RETURN",
        Tok::Delimiter(d) => [delim::EQUALS, delim::LEFT_PAREN, delim::COMMA, delim::CONCATENATION, delim::NOT_EQUALS].contains(d),
        Tok::ExactNumeric(_)
        | Tok::ApproximateNumeric(_)
        | Tok::CharacterString(_)
        | Tok::BinaryString(_)
        | Tok::Introducer(_)
        | Tok::LargeObjectLength(_)
        | Tok::End => false,
    }
}

/// \[EXTENSION\] The query of a GQL procedure, lowered to SQL: the text
/// of a `<query expression>` and the variables of its columns.
///
/// # Errors
///
/// SQLCODE -103 if a variable is not in scope or is of the wrong kind,
/// or of a label or property the graph has not (GRF-0012); -900 if the
/// procedure modifies data, which only EXECUTE GQL and a GQL session run.
pub(crate) fn lower(text: &str, procedure: &GqlProcedure, graphs: Graphs<'_>) -> Result<Lowered> {
    let chars: Vec<char> = text.chars().collect();
    let toks = lex(text)?;
    let l = Lowering::new(&chars, &toks, graphs);
    l.procedure(procedure, None)
}

/// The error of a variable of the wrong kind where it is used.
pub(crate) fn misused(name: &str, what: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0022 variable {name} is not {what} where it is used"))
}
