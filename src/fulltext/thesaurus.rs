//! Thesauri: the relations between terms that a pattern's thesaurus
//! expansions follow [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1
//! Description 5k–5p, §6.11–§6.16] (RULES.md D87).
//!
//! A thesaurus is the rows of the tables of the schema FT_THESAURUS that
//! clause 8 models, made by a user with that clause's definitions:
//! TERM_DICTIONARY names each term, and TERM_HIERARCHY, TERM_SYNONYM and
//! TERM_RELATED relate them. The number of thesauri and their names are
//! implementation-defined [§8.3 Description 1]; here they are those whose
//! names those tables hold.
//!
//! A call of Contains or Rank whose pattern may expand by a thesaurus is
//! given the relations as its last argument, an ARRAY its statement reads
//! once in its snapshot (`relations_query`, `ScalarFunction::appended_query`),
//! so a thesaurus changed by a transaction is seen by its own statements
//! and by no other's until it commits. `Relations` then answers the
//! expansions of §6.11.5 to §6.16.5 from them.

use std::collections::{HashMap, HashSet};

use crate::error::{Code, Result, SqlError};
use crate::fulltext::pattern::{Relation, Terms};
use crate::fulltext::{Language, tokenize};
use crate::functions::AppendedQuery;
use crate::value::Value;

/// The schema of the thesaurus tables [§8.2].
pub const SCHEMA: &str = "FT_THESAURUS";

/// The tables the relations are read from [§8.3–§8.6].
pub const TABLES: [&str; 4] = ["TERM_DICTIONARY", "TERM_HIERARCHY", "TERM_SYNONYM", "TERM_RELATED"];

/// FT_ThesNameLength, the longest thesaurus name in characters [§8.3 DR1].
pub const NAME_LENGTH: u32 = 128;

/// FT_ThesTermLength, the longest thesaurus term in characters [§10.2
/// Description 5].
pub const TERM_LENGTH: u32 = 1024;

/// The kind of a relation row of a broader and a narrower term.
const HIERARCHY: &str = "H";

/// The kind of a relation row of a term, a synonym and its preferred term.
const SYNONYM: &str = "S";

/// The kind of a relation row of a term and a related term.
const RELATED: &str = "R";

/// The word that marks a thesaurus expansion in a pattern [§5.3.1].
const THESAURUS: &str = "THESAURUS";

/// The relations a call is given of a pattern `constant`, None if it is no
/// constant: none of a constant pattern that names no thesaurus, else the
/// query of every relation of every thesaurus.
pub fn relations_query(constant: Option<&Value>) -> Option<AppendedQuery> {
    if let Some(Value::Char(text)) = constant
        && !text.to_uppercase().contains(THESAURUS)
    {
        return None;
    }
    let tables = TABLES.iter().map(|t| (SCHEMA.to_owned(), (*t).to_owned())).collect();
    Some(AppendedQuery { query: query(), tables })
}

/// The text of the query of the relations: one column, a row of each
/// relation of its thesaurus's name, its kind, its two terms, and of a
/// synonym its preferred term.
fn query() -> String {
    let name = format!("CAST(TRIM(BOTH ' ' FROM {{t}}) AS CHARACTER VARYING({NAME_LENGTH}))");
    let term = |d: &str| format!("CAST({d}.EXPR AS CHARACTER VARYING({TERM_LENGTH}))");
    let none = format!("CAST(NULL AS CHARACTER VARYING({TERM_LENGTH}))");
    let of = |alias: &str, id: &str, thesaurus: &str| {
        format!("JOIN {SCHEMA}.TERM_DICTIONARY {alias} ON {alias}.TERMID = {id} AND {alias}.THNAME_DIC = {thesaurus}")
    };
    let hierarchy = format!(
        "SELECT ROW({}, '{HIERARCHY}', {}, {}, {none}) FROM {SCHEMA}.TERM_HIERARCHY X {} {}",
        name.replace("{t}", "X.THNAME_HRR"),
        term("A"),
        term("B"),
        of("A", "X.TERMID", "X.THNAME_HRR"),
        of("B", "X.NARROWER_TERMID", "X.THNAME_HRR"),
    );
    let synonym = format!(
        "SELECT ROW({}, '{SYNONYM}', {}, {}, {}) FROM {SCHEMA}.TERM_SYNONYM X {} {} LEFT {}",
        name.replace("{t}", "X.THNAME_SYN"),
        term("A"),
        term("B"),
        term("C"),
        of("A", "X.TERMID", "X.THNAME_SYN"),
        of("B", "X.SYNONYM_TERMID", "X.THNAME_SYN"),
        of("C", "X.PREFERRED_TERMID", "X.THNAME_SYN"),
    );
    let related = format!(
        "SELECT ROW({}, '{RELATED}', {}, {}, {none}) FROM {SCHEMA}.TERM_RELATED X {} {}",
        name.replace("{t}", "X.THNAME_REL"),
        term("A"),
        term("B"),
        of("A", "X.TERMID", "X.THNAME_REL"),
        of("B", "X.RELATED_TERMID", "X.THNAME_REL"),
    );
    format!("{hierarchy} UNION ALL {synonym} UNION ALL {related}")
}

/// A term as the relations compare it: its words, normalized, one space
/// between each, as §6.11.5 compares terms by their word arrays.
fn key(term: &str) -> Result<String> {
    let mut words = Vec::new();
    for token in tokenize(term.chars().map(Ok), Language::None) {
        words.push(token?.word);
    }
    Ok(words.join(" "))
}

/// A thesaurus's name and a term of it, as the relations key them.
type Entry = (String, String);

/// The relations of the thesauri a statement reads, each way they are
/// followed.
#[derive(Debug, Default)]
pub struct Relations {
    /// Of each term its broader terms.
    broader: HashMap<Entry, Vec<String>>,
    /// Of each term its narrower terms.
    narrower: HashMap<Entry, Vec<String>>,
    /// Of each term its synonyms.
    synonyms: HashMap<Entry, Vec<String>>,
    /// Of each term its preferred terms.
    preferred: HashMap<Entry, Vec<String>>,
    /// Of each term its related terms.
    related: HashMap<Entry, Vec<String>>,
    /// Of each term's key, its text as the thesaurus writes it.
    text: HashMap<Entry, String>,
}

/// The relations of `rows`, the last argument of a call of Contains or Rank;
/// none if there is no such argument or it is null.
///
/// # Errors
///
/// SQLCODE -900 if `rows` is not the array the binder appends.
pub fn terms_of(rows: Option<&Value>) -> Result<Relations> {
    let mut relations = Relations::default();
    let Some(rows) = rows.filter(|r| !r.is_null()) else { return Ok(relations) };
    let Value::Array(rows) = rows else { return Err(malformed()) };
    for row in rows.iter() {
        relations.add(row)?;
    }
    Ok(relations)
}

/// The error of an argument that is not the relations the binder appends.
fn malformed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] the thesaurus relations of a call are an ARRAY of rows of five character strings")
}

/// A character string field's text, None of a null.
fn text(v: &Value) -> Result<Option<&str>> {
    if let Value::Char(s) = v {
        return Ok(Some(s));
    }
    if v.is_null() {
        return Ok(None);
    }
    Err(SqlError::new(Code::Api, format!("[DESIGN] a thesaurus relation's field is {v}, no character string")))
}

impl Relations {
    /// Adds a relation row.
    fn add(&mut self, row: &Value) -> Result<()> {
        let Value::Row(fields) = row else { return Err(malformed()) };
        let [name, kind, a, b, c] = fields.as_ref() else { return Err(malformed()) };
        let (Some(name), Some(kind), Some(a), Some(b)) = (text(name)?, text(kind)?, text(a)?, text(b)?) else {
            return Ok(());
        };
        let a = self.entry(name, a)?;
        let b = self.entry(name, b)?;
        match kind {
            HIERARCHY => {
                push(&mut self.narrower, &a, &b);
                push(&mut self.broader, &b, &a);
            }
            SYNONYM => {
                push(&mut self.synonyms, &a, &b);
                if let Some(c) = text(c)? {
                    let c = self.entry(name, c)?;
                    push(&mut self.preferred, &a, &c);
                }
            }
            RELATED => push(&mut self.related, &a, &b),
            other => return Err(SqlError::new(Code::Api, format!("[DESIGN] no thesaurus relation is of kind {other}"))),
        }
        Ok(())
    }

    /// The entry of `term` of thesaurus `name`, its text kept.
    fn entry(&mut self, name: &str, term: &str) -> Result<Entry> {
        let entry = (name.to_owned(), key(term)?);
        self.text.entry(entry.clone()).or_insert_with(|| term.to_owned());
        Ok(entry)
    }

    /// The broader terms of `start` that have none broader [§6.16.5].
    fn top(&self, start: &Entry) -> Vec<Entry> {
        let above = closure(&self.broader, start, None);
        above.into_iter().filter(|e| !self.broader.contains_key(e)).collect()
    }
}

impl Terms for Relations {
    /// The terms `term` is related to in thesaurus `thesaurus` by
    /// `relation`, as the thesaurus writes them, `term` not among them;
    /// `Pattern::expanded` adds it as §6.11.5 to §6.16.5 do:
    ///
    /// * BROADER and NARROWER, those up or down the hierarchy from it,
    ///   `levels` steps or, of none, all [§5.3.1 NOTE 14].
    /// * SYNONYM, its synonyms, theirs, and so on.
    /// * RELATED, its related terms.
    /// * PREFERRED, its preferred terms.
    /// * TOP, the terms above it in the hierarchy that have none above
    ///   them.
    ///
    /// A thesaurus name is compared trimmed of spaces, and a term by its
    /// words. A term the thesaurus does not hold is related to none.
    ///
    /// # Errors
    ///
    /// None but a tokenizer's, which a term's characters never raise.
    fn terms(&self, thesaurus: &str, relation: Relation, term: &str, levels: Option<u64>) -> Result<Vec<String>> {
        let start = (thesaurus.trim_matches(' ').to_owned(), key(term)?);
        let found = match relation {
            Relation::Broader => closure(&self.broader, &start, levels),
            Relation::Narrower => closure(&self.narrower, &start, levels),
            Relation::Synonym => closure(&self.synonyms, &start, None),
            Relation::Related => closure(&self.related, &start, Some(1)),
            Relation::Preferred => closure(&self.preferred, &start, Some(1)),
            Relation::Top => self.top(&start),
        };
        Ok(found.iter().filter(|e| **e != start).filter_map(|e| self.text.get(e).cloned()).collect())
    }
}

/// Adds `to` to the terms of `from`.
fn push(map: &mut HashMap<Entry, Vec<String>>, from: &Entry, to: &Entry) {
    map.entry(from.clone()).or_default().push(to.1.clone());
}

/// `start` and the terms reached from it in at most `levels` steps, or
/// any number of none, each once, in the order reached.
fn closure(map: &HashMap<Entry, Vec<String>>, start: &Entry, levels: Option<u64>) -> Vec<Entry> {
    let mut seen = HashSet::from([start.clone()]);
    let mut out = vec![start.clone()];
    let mut frontier = vec![start.clone()];
    let mut level = 0_u64;
    while !frontier.is_empty() && levels.is_none_or(|n| level < n) {
        let mut next = Vec::new();
        for entry in &frontier {
            for term in map.get(entry).into_iter().flatten() {
                let reached = (entry.0.clone(), term.clone());
                if seen.insert(reached.clone()) {
                    next.push(reached.clone());
                    out.push(reached);
                }
            }
        }
        frontier = next;
        level = level.saturating_add(1);
    }
    out
}
