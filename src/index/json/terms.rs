//! \[DESIGN\] What a json index posts of a document (RULES.md D34): a term
//! for each node, its path and ⊤, and one for each canonical value of each
//! scalar, its path and the value. A path is the chain of member names
//! from the root, array steps dropped, as lax mode unwraps arrays, so a
//! path a member chain reaches is one whatever arrays lie on the way.
//!
//! A term is two 64-bit hashes, of its path and of its value (MurmurHash3
//! x64 128-bit, `hash/murmur.rs`, its first half, fixed seeds): stable
//! across runs and platforms. Two terms may collide, which only finds a
//! document more than it should, and every row found is rechecked.
//!
//! A value is canonical so that values SQL/JSON finds equal post alike:
//! a number by the `f64` nearest its exact value, as numbers compare by
//! value; a string by its code points, trailing spaces dropped, so a
//! string a JSON_VALUE result equals with PAD SPACE posts alike too; a
//! truth value; the SQL/JSON null. A number or truth value also posts the
//! character string JSON_VALUE casts it to, and a string reading TRUE or
//! FALSE the truth value a cast of it to BOOLEAN gives, so JSON_VALUE of
//! either returned type finds it (`need.rs`).

use std::collections::BTreeSet;

use crate::hash::murmur::murmur3_x64_128;
use crate::json::{IMPLICIT_RETURNING, Item};
use crate::value::{Value, cast};

/// Bytes of a term: its path's hash, then its value's.
pub(super) const TERM_BYTES: usize = 16;

/// A term.
pub(super) type Term = [u8; TERM_BYTES];

/// \[DESIGN\] The seed of a path's hash; its bytes spell "JSP1".
const PATH_SEED: u32 = 0x4A53_5031;
/// \[DESIGN\] The seed of a value's hash; its bytes spell "JSV1".
const VALUE_SEED: u32 = 0x4A53_5631;
/// The value half of a node's term, ⊤: no value's hash is asked to avoid
/// it, as a collision only finds more.
const NODE: u64 = 0;
/// \[DESIGN\] The term of a document the index cannot read: not JSON text,
/// or past a limit of turborel's. Every probe finds it, so the WHERE check
/// raises of it as a scan would (RULES.md D15).
pub(super) const UNREAD: Term = [0xFF; TERM_BYTES];

// How each kind of canonical value begins, before its bytes.
/// The SQL/JSON null.
const NULL_TAG: u8 = b'n';
/// A truth value: then 1 or 0.
const BOOL_TAG: u8 = b'b';
/// A number: then its `f64`'s bits.
const NUMBER_TAG: u8 = b'd';
/// A string: then its UTF-8.
const TEXT_TAG: u8 = b's';

/// The words a string is cast to BOOLEAN of, in any case.
const TRUE_WORD: &str = "true";
const FALSE_WORD: &str = "false";

/// How an index reads documents: whether it posts paths alone, and how
/// many member names deep it goes (RULES.md D41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// JSON.PATHS_ONLY: post nodes, no values.
    pub paths_only: bool,
    /// JSON.MAX_DEPTH: the most member names of a path posted.
    pub max_depth: usize,
}

/// A canonical value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Canon {
    /// The SQL/JSON null.
    Null,
    /// A truth value.
    Bool(bool),
    /// A number, by its nearest `f64`'s bits, zero's sign dropped.
    Number(u64),
    /// A string, trailing spaces dropped.
    Text(String),
}

impl Canon {
    /// The hash of the value.
    fn hash(&self) -> u64 {
        let mut bytes = Vec::new();
        match self {
            Canon::Null => bytes.push(NULL_TAG),
            Canon::Bool(truth) => bytes.extend_from_slice(&[BOOL_TAG, u8::from(*truth)]),
            Canon::Number(bits) => {
                bytes.push(NUMBER_TAG);
                bytes.extend_from_slice(&bits.to_be_bytes());
            }
            Canon::Text(text) => {
                bytes.push(TEXT_TAG);
                bytes.extend_from_slice(text.as_bytes());
            }
        }
        murmur3_x64_128(&bytes, VALUE_SEED).0
    }
}

/// The canonical number of `value`, if it is one: the `f64` nearest its
/// exact value, read of its decimal digits, so equal numbers of any kind
/// give one.
pub(super) fn number_bits(value: &Value) -> Option<u64> {
    let x = match value {
        Value::Exact(d) => d.to_string().parse::<f64>().ok()?,
        Value::Approx(x) => *x,
        Value::DecFloat(d) => d.numeral().parse::<f64>().ok()?,
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => return None,
    };
    // Zero's sign compares equal, so it hashes alike.
    Some(if x == 0.0_f64 { 0.0_f64.to_bits() } else { x.to_bits() })
}

/// The canonical string of `text`.
pub(super) fn text(text: &str) -> Canon {
    Canon::Text(text.trim_end_matches(' ').to_owned())
}

/// The canonical value a probe compares with, of an SQL/JSON scalar's
/// value or the null: none of a datetime, which no document holds.
pub(super) fn probe_canon(item: &Item) -> Option<Canon> {
    let Item::Scalar(scalar) = item else {
        return matches!(item, Item::Null).then_some(Canon::Null);
    };
    let value = scalar.value();
    if let Value::Char(s) = value {
        return Some(text(s));
    }
    if let Value::Bool(truth) = value {
        return Some(Canon::Bool(*truth));
    }
    number_bits(value).map(Canon::Number)
}

/// The canonical values a document's scalar `value` posts.
fn posted(value: &Value) -> Vec<Canon> {
    let mut out = Vec::with_capacity(2);
    if let Value::Char(s) = value {
        out.push(text(s));
        let word = s.trim();
        if word.eq_ignore_ascii_case(TRUE_WORD) || word.eq_ignore_ascii_case(FALSE_WORD) {
            out.push(Canon::Bool(word.eq_ignore_ascii_case(TRUE_WORD)));
        }
        return out;
    }
    if let Value::Bool(truth) = value {
        out.push(Canon::Bool(*truth));
    } else {
        out.extend(number_bits(value).map(Canon::Number));
    }
    // As JSON_VALUE casts it to a character string.
    if let Ok(Value::Char(cast_text)) = cast(value.clone(), &IMPLICIT_RETURNING) {
        out.push(text(&cast_text));
    }
    out
}

/// A path being read: the member names so far, each its length then its
/// bytes, so no two chains read alike.
#[derive(Debug, Clone, Default)]
pub(super) struct Chain {
    bytes: Vec<u8>,
    names: usize,
}

impl Chain {
    /// The chain of `names`.
    pub(super) fn of<'a>(names: impl IntoIterator<Item = &'a str>) -> Chain {
        let mut chain = Chain::default();
        for name in names {
            chain.push(name);
        }
        chain
    }

    /// It with one more member name.
    pub(super) fn push(&mut self, name: &str) {
        self.bytes.extend_from_slice(&u32::try_from(name.len()).unwrap_or(u32::MAX).to_be_bytes());
        self.bytes.extend_from_slice(name.as_bytes());
        self.names = self.names.saturating_add(1);
    }

    /// How many member names it has.
    pub(super) fn depth(&self) -> usize {
        self.names
    }

    /// The hash of the path.
    fn hash(&self) -> u64 {
        murmur3_x64_128(&self.bytes, PATH_SEED).0
    }

    /// The term of its node.
    pub(super) fn node(&self) -> Term {
        term(self.hash(), NODE)
    }

    /// The term of `value` at it.
    pub(super) fn value(&self, value: &Canon) -> Term {
        term(self.hash(), value.hash())
    }
}

/// The term of a path's and a value's hashes.
fn term(path: u64, value: u64) -> Term {
    let mut out = [0_u8; TERM_BYTES];
    let (high, low) = out.split_at_mut(size_of::<u64>());
    high.copy_from_slice(&path.to_be_bytes());
    low.copy_from_slice(&value.to_be_bytes());
    out
}

/// The terms of the document `item`, each once, as `shape` reads it.
/// Parsed items nest no deeper than `json::MAX_NESTING`, so the walk is
/// bounded.
pub(super) fn terms_of(item: &Item, shape: Shape) -> BTreeSet<Term> {
    let mut out = BTreeSet::new();
    walk(item, &Chain::default(), shape, &mut out);
    out
}

/// Posts `item`, at `chain`, and what it holds.
fn walk(item: &Item, chain: &Chain, shape: Shape, out: &mut BTreeSet<Term>) {
    if chain.depth() > shape.max_depth {
        return;
    }
    out.insert(chain.node());
    match item {
        Item::Null => {
            if !shape.paths_only {
                out.insert(chain.value(&Canon::Null));
            }
        }
        Item::Scalar(scalar) => {
            if !shape.paths_only {
                out.extend(posted(scalar.value()).iter().map(|canon| chain.value(canon)));
            }
        }
        // Array steps are dropped: each element is at the array's path.
        Item::Array(elements) => elements.iter().for_each(|element| walk(element, chain, shape, out)),
        Item::Object(members) => {
            for member in members {
                let mut deeper = chain.clone();
                deeper.push(&member.key);
                walk(&member.value, &deeper, shape, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Keys, parse};

    const WHOLE: Shape = Shape { paths_only: false, max_depth: 128 };

    fn terms(text: &str, shape: Shape) -> BTreeSet<Term> {
        terms_of(&parse(text, Keys::Any).unwrap(), shape)
    }

    /// Verifies: [EXTENSION] a document posts each node and each scalar's
    /// canonical values at its member chain, array steps dropped; equal
    /// numbers of any kind alike, and strings without trailing spaces.
    #[test]
    fn a_document_posts_nodes_and_canonical_values() {
        let found = terms(r#"{"a":[{"b":1.50}],"c":"x  "}"#, WHOLE);
        let ab = Chain::of(["a", "b"]);
        assert!(found.contains(&Chain::default().node()));
        assert!(found.contains(&ab.node()));
        assert!(found.contains(&ab.value(&Canon::Number(1.5_f64.to_bits()))));
        assert!(found.contains(&Chain::of(["c"]).value(&text("x"))));
        let approx = terms(r#"{"a":{"b":15e-1}}"#, WHOLE);
        assert!(approx.contains(&ab.value(&Canon::Number(1.5_f64.to_bits()))), "1.5 and 15e-1 alike");
        assert!(!found.contains(&Chain::of(["b"]).node()), "paths begin at the root");
    }

    /// Verifies: [EXTENSION] JSON.PATHS_ONLY posts no values and
    /// JSON.MAX_DEPTH no path deeper than it.
    #[test]
    fn options_limit_what_is_posted() {
        let paths = terms(r#"{"a":{"b":1}}"#, Shape { paths_only: true, max_depth: 128 });
        assert_eq!(paths.len(), 3, "the root, a and a.b");
        let shallow = terms(r#"{"a":{"b":1}}"#, Shape { paths_only: false, max_depth: 1 });
        assert!(shallow.contains(&Chain::of(["a"]).node()));
        assert!(!shallow.contains(&Chain::of(["a", "b"]).node()));
    }

    /// Verifies: [EXTENSION] a number posts the string JSON_VALUE casts it
    /// to, and a string reading true the truth value.
    #[test]
    fn casts_post_their_results() {
        let found = terms(r#"{"n":5,"t":" TRUE"}"#, WHOLE);
        assert!(found.contains(&Chain::of(["n"]).value(&text("5"))));
        assert!(found.contains(&Chain::of(["t"]).value(&Canon::Bool(true))));
        assert_eq!(number_bits(&Value::Approx(-0.0_f64)), number_bits(&Value::Approx(0.0_f64)));
    }
}
