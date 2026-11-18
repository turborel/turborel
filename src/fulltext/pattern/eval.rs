//! A pattern evaluated over one document: the value of `T.Contains(P) = 1`
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.2 Description 3c, §5.3.1
//! Description 5], as the Contains methods of the structured patterns
//! define it [§6].
//!
//! The document is read once, streaming, as the tokenizer cuts it. Each
//! word, phrase and expansion of the pattern is a leaf (`leaf.rs`) that
//! reads each token and keeps whether it was found and, under a proximity
//! or a context, where (`window.rs`). Then the operators combine them:
//! `|` OR, `&` AND, NOT negation [§5.3.1 Description 5a-5c].

mod leaf;
mod window;

use std::cell::Cell;

use crate::error::{Code, Result, SqlError};
use crate::fulltext::pattern::expand::EXPANSION_DEPTH;
use crate::fulltext::pattern::{Argument, Expansion, MAX_DEPTH, Pattern, Unit};
use crate::fulltext::{Language, tokenize};
use leaf::{Leaf, Matcher, Seen, Span};

/// Whether the document of characters `chars`, in `language`, matches
/// `pattern`: `T.Contains(P) = 1` [§5.1.2 Description 3c, §5.3.1
/// Description 5]. A document of no characters matches no pattern, NOT
/// ones too [§5.1.2 Description 3c ii]. The caller maps a null document to
/// null.
///
/// The document is read once and never held whole. What is kept is the
/// pattern's leaves and, of the document, only the places of tokens that
/// match a leaf under a proximity or a context: memory grows with those
/// matches, not with the document. A phrase also keeps the matches it has
/// begun, at most its words squared.
///
/// IS ABOUT is implementation-defined [§6.17.2 Description 2]: a document
/// is about a word or phrase when it holds any word of it that is not a
/// stop word of its language.
///
/// # Errors
///
/// SQLCODE -910 if the pattern still holds a thesaurus expansion, which
/// `Pattern::expanded` must replace first, or nests deeper than
/// `MAX_DEPTH`; the errors of reading `chars`.
pub fn contains<I: Iterator<Item = Result<char>>>(pattern: &Pattern, chars: I, language: Language) -> Result<bool> {
    let mut leaves = Vec::new();
    let node = compile(pattern, &mut leaves, 0)?;
    let stems = leaves.iter().any(Leaf::stems);
    let read = Cell::new(false);
    for token in tokenize(chars.inspect(|_| read.set(true)), language) {
        let token = token?;
        let seen = Seen::new(&token, language, stems);
        for leaf in &mut leaves {
            leaf.step(&seen);
        }
    }
    Ok(read.get() && value(&node, &leaves))
}

/// A pattern as evaluated: its operators over its leaves, by index.
#[derive(Debug)]
enum Node {
    /// Either holds.
    Or(Box<Node>, Box<Node>),
    /// Both hold.
    And(Box<Node>, Box<Node>),
    /// It does not hold.
    Not(Box<Node>),
    /// The leaf was found.
    Leaf(usize),
    /// A match of one list near one of the other [§6.8.2].
    Near {
        /// The leaves of the first list.
        left: Vec<usize>,
        /// The leaves of the second.
        right: Vec<usize>,
        /// The distance.
        distance: u64,
        /// Its unit.
        unit: Unit,
        /// Whether the first list's match comes first.
        in_order: bool,
    },
    /// A match of each argument in one unit [§6.18.2].
    Context {
        /// The unit.
        unit: Unit,
        /// The leaves of each argument.
        arguments: Vec<Vec<usize>>,
    },
}

/// The error of a thesaurus expansion not yet expanded.
fn unexpanded() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thesaurus expansion is evaluated only once Pattern::expanded has replaced it")
}

/// `pattern`, at depth `depth`, as a node over `leaves`, its leaves added.
///
/// # Errors
///
/// SQLCODE -910 for a thesaurus expansion, or past the depth an expanded
/// pattern can reach.
fn compile(pattern: &Pattern, leaves: &mut Vec<Leaf>, depth: usize) -> Result<Node> {
    if depth > MAX_DEPTH.saturating_add(EXPANSION_DEPTH) {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] a search pattern may nest at most {MAX_DEPTH} deep")));
    }
    let deeper = depth.saturating_add(1);
    Ok(match pattern {
        Pattern::Or(l, r) => Node::Or(Box::new(compile(l, leaves, deeper)?), Box::new(compile(r, leaves, deeper)?)),
        Pattern::And(l, r) => Node::And(Box::new(compile(l, leaves, deeper)?), Box::new(compile(r, leaves, deeper)?)),
        Pattern::Not(inner) => Node::Not(Box::new(compile(inner, leaves, deeper)?)),
        Pattern::Literal(literal) => Node::Leaf(add(leaves, Matcher::literal(literal), false)),
        Pattern::About(literal) => Node::Leaf(add(leaves, Matcher::about(literal), false)),
        Pattern::Expansion(expansion) => Node::Leaf(add(leaves, expansion_matcher(expansion)?, false)),
        Pattern::Near(near) => Node::Near {
            left: argument(&near.left, leaves)?,
            right: argument(&near.right, leaves)?,
            distance: near.distance,
            unit: near.unit,
            in_order: near.in_order,
        },
        Pattern::Context(context) => {
            Node::Context { unit: context.unit, arguments: context.arguments.iter().map(|a| argument(a, leaves)).collect::<Result<_>>()? }
        }
    })
}

/// Adds a leaf of `matcher` to `leaves`, keeping its places if `keep`; its
/// index.
fn add(leaves: &mut Vec<Leaf>, matcher: Matcher, keep: bool) -> usize {
    leaves.push(Leaf::new(matcher, keep));
    leaves.len().saturating_sub(1)
}

/// The matcher of `expansion`.
///
/// # Errors
///
/// SQLCODE -910 for a thesaurus expansion.
fn expansion_matcher(expansion: &Expansion) -> Result<Matcher> {
    match expansion {
        Expansion::Soundex(literal) => Ok(Matcher::sounds(literal)),
        Expansion::Fuzzy(literal) => Ok(Matcher::spelled(literal)),
        Expansion::Thesaurus(_) => Err(unexpanded()),
    }
}

/// The leaves of the proximity list or context argument `argument`, which
/// keep their places, added to `leaves`.
fn argument(argument: &Argument, leaves: &mut Vec<Leaf>) -> Result<Vec<usize>> {
    Ok(match argument {
        Argument::Literal(literal) => vec![add(leaves, Matcher::literal(literal), true)],
        Argument::List(literals) => literals.iter().map(|l| add(leaves, Matcher::literal(l), true)).collect(),
        Argument::Expansion(expansion) => vec![add(leaves, expansion_matcher(expansion)?, true)],
    })
}

/// The places the leaves `indexes` were found at.
fn spans(indexes: &[usize], leaves: &[Leaf]) -> Vec<Span> {
    indexes.iter().filter_map(|i| leaves.get(*i)).flat_map(|leaf| leaf.spans.iter().copied()).collect()
}

/// Whether `node` holds, its leaves having read the document. Its depth
/// is that `compile` checked.
fn value(node: &Node, leaves: &[Leaf]) -> bool {
    match node {
        Node::Or(l, r) => value(l, leaves) || value(r, leaves),
        Node::And(l, r) => value(l, leaves) && value(r, leaves),
        Node::Not(inner) => !value(inner, leaves),
        Node::Leaf(i) => leaves.get(*i).is_some_and(|leaf| leaf.found),
        Node::Near { left, right, distance, unit, in_order } => {
            window::near(&spans(left, leaves), &spans(right, leaves), *distance, *unit, *in_order)
        }
        Node::Context { unit, arguments } => {
            let arguments: Vec<Vec<Span>> = arguments.iter().map(|a| spans(a, leaves)).collect();
            window::context(&arguments, *unit)
        }
    }
}
