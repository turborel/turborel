//! Thesaurus expansions replaced by the terms they expand to
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1 Description 5f, 5g,
//! §6.11-§6.16]. A pattern is expanded before it is evaluated or answered
//! by an index, as the terms come from tables (`Terms`).
//!
//! - BROADER, NARROWER, SYNONYM and RELATED TERM OF a term give it and its
//!   terms [§6.11.5 DR6, §6.12.5 DR6, §6.13.5 DR5, §6.15.5 DR5]; PREFERRED
//!   and TOP give the term only when it has none [§6.14.5 DR5, §6.16.5
//!   DR4]. A thesaurus or a term the tables do not hold has no terms.
//! - As a primary, the expansion becomes the OR of its terms, built as a
//!   balanced tree so it nests at most `EXPANSION_DEPTH` more; in a
//!   proximity or a context condition, a list of them [§5.3.1 Description
//!   5f, 5g].
//! - A term is cut into words and normalized as a document is, each word
//!   matched as written, in the language of the term it expands.

use crate::error::{Code, Result, SqlError};
use crate::fulltext::pattern::literal::term_literal;
use crate::fulltext::pattern::{Argument, Context, Expansion, Literal, MAX_DEPTH, Part, Pattern, Proximity, Relation, Thesaurus};

/// The most a balanced OR of any count of terms adds to a pattern's depth:
/// one level for each bit of the count.
pub(super) const EXPANSION_DEPTH: usize = 64;

/// The terms of a thesaurus [§8]: what a thesaurus expansion expands to.
///
/// The contract: `terms` gives the terms related to `term` by `relation`
/// in the thesaurus named `thesaurus`, `term` not among them, each as a
/// word or a phrase separated by blanks, for `levels` levels of BROADER or
/// NARROWER, or all levels when none [§6.11.5 DR4, DR5, §6.12.5]. It gives
/// none for a thesaurus or a term it does not hold, and raises only for a
/// failure reading them.
pub trait Terms {
    /// The terms `term` expands to.
    ///
    /// # Errors
    ///
    /// As reading the thesaurus.
    fn terms(&self, thesaurus: &str, relation: Relation, term: &str, levels: Option<u64>) -> Result<Vec<String>>;
}

impl Pattern {
    /// The pattern with each thesaurus expansion replaced by the terms it
    /// expands to, from `terms`.
    ///
    /// # Errors
    ///
    /// As `terms`; SQLCODE -910 if the pattern nests deeper than
    /// `MAX_DEPTH`.
    pub fn expanded(&self, terms: &dyn Terms) -> Result<Pattern> {
        expand(self, terms, 0)
    }
}

/// `pattern`, at depth `depth`, expanded by `terms`.
fn expand(pattern: &Pattern, terms: &dyn Terms, depth: usize) -> Result<Pattern> {
    if depth > MAX_DEPTH {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] a search pattern may nest at most {MAX_DEPTH} deep")));
    }
    let deeper = depth.saturating_add(1);
    let both = |l: &Pattern, r: &Pattern| -> Result<(Box<Pattern>, Box<Pattern>)> {
        Ok((Box::new(expand(l, terms, deeper)?), Box::new(expand(r, terms, deeper)?)))
    };
    Ok(match pattern {
        Pattern::Or(l, r) => {
            let (l, r) = both(l, r)?;
            Pattern::Or(l, r)
        }
        Pattern::And(l, r) => {
            let (l, r) = both(l, r)?;
            Pattern::And(l, r)
        }
        Pattern::Not(inner) => Pattern::Not(Box::new(expand(inner, terms, deeper)?)),
        Pattern::Expansion(Expansion::Thesaurus(thesaurus)) => any_of(expansion(thesaurus, terms)?, &thesaurus.of),
        Pattern::Literal(_) | Pattern::About(_) | Pattern::Expansion(Expansion::Soundex(_) | Expansion::Fuzzy(_)) => pattern.clone(),
        Pattern::Near(near) => {
            Pattern::Near(Proximity { left: argument(&near.left, terms)?, right: argument(&near.right, terms)?, ..*near })
        }
        Pattern::Context(context) => Pattern::Context(Context {
            unit: context.unit,
            arguments: context.arguments.iter().map(|a| argument(a, terms)).collect::<Result<_>>()?,
        }),
    })
}

/// The argument `argument` expanded: a thesaurus expansion becomes the list
/// of its terms.
fn argument(argument: &Argument, terms: &dyn Terms) -> Result<Argument> {
    match argument {
        Argument::Expansion(Expansion::Thesaurus(thesaurus)) => Ok(Argument::List(expansion(thesaurus, terms)?)),
        Argument::Literal(_) | Argument::List(_) | Argument::Expansion(Expansion::Soundex(_) | Expansion::Fuzzy(_)) => Ok(argument.clone()),
    }
}

/// The literals `thesaurus` expands to, each once, the term it starts from
/// first when it is among them.
fn expansion(thesaurus: &Thesaurus, terms: &dyn Terms) -> Result<Vec<Literal>> {
    let start = &thesaurus.of;
    let found = terms.terms(&thesaurus.name, thesaurus.relation, &term_text(start), thesaurus.levels)?;
    let mut out: Vec<Literal> = Vec::with_capacity(found.len().saturating_add(1));
    for term in found {
        if let Some(literal) = term_literal(&term, start.language)?
            && !out.contains(&literal)
            && literal != *start
        {
            out.push(literal);
        }
    }
    let with_start = match thesaurus.relation {
        Relation::Broader | Relation::Narrower | Relation::Synonym | Relation::Related => true,
        Relation::Preferred | Relation::Top => out.is_empty(),
    };
    if with_start {
        out.insert(0, start.clone());
    }
    Ok(out)
}

/// The words of `literal` as a thesaurus holds a term: its words, as
/// written, separated by blanks. In a language read as bigrams the pairs
/// of a run are joined back into it, `東京 京都` into `東京都` (RULES.md
/// D323).
fn term_text(literal: &Literal) -> String {
    let words: Vec<String> = literal
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Word(mask) => mask.text(),
            Part::Optional => None,
        })
        .collect();
    if !literal.language.reads_bigrams() {
        return words.join(" ");
    }
    let mut out = String::new();
    let mut last: Option<String> = None;
    for word in words {
        if let Some(rest) = last.as_deref().and_then(|before| overlap(before, &word)) {
            out.push_str(rest);
        } else {
            if last.is_some() {
                out.push(' ');
            }
            out.push_str(&word);
        }
        last = Some(word);
    }
    out
}

/// What `word` adds after `before` if it is the next pair of its run: its
/// part past the longest start of it, short of the whole, that ends
/// `before`.
fn overlap<'w>(before: &str, word: &'w str) -> Option<&'w str> {
    let starts: Vec<usize> = word.char_indices().map(|(at, _)| at).skip(1).collect();
    starts.into_iter().rev().find(|at| word.get(..*at).is_some_and(|start| before.ends_with(start))).and_then(|at| word.get(at..))
}

/// The OR of `literals`, as a balanced tree, so it nests no deeper than
/// `EXPANSION_DEPTH`; `start` if there are none, though an expansion
/// always gives at least one.
fn any_of(mut literals: Vec<Literal>, start: &Literal) -> Pattern {
    if literals.len() <= 1 {
        return Pattern::Literal(literals.pop().unwrap_or_else(|| start.clone()));
    }
    let right = literals.split_off(literals.len().div_ceil(2));
    Pattern::Or(Box::new(any_of(literals, start)), Box::new(any_of(right, start)))
}
