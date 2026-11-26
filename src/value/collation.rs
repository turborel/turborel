//! \[EXTENSION\] [ISO/IEC 9075-2:2023 §4.3.6] Collations (RULES.md D60): how
//! character strings compare, each collation a way of turning a string into
//! a sort key, bytes that compare as the strings do.
//!
//! A collation of the default behaviour (`CollationRef::is_default`) makes
//! no sort key: its comparisons stay bare, and `value/comparison.rs` judges
//! them. Any other reaches comparison as `Value::Binary` of the sort key, so
//! ordering, grouping, hashing and spilling need not know of collations,
//! and an index on a collated column holds sort keys. One whose key is the
//! text itself (`CollationRef::is_octets`) is marked in a plan as any other
//! is, and the operators that read vectors compare, hash and order its
//! strings in place, unpadded (RULES.md D1183).
//!
//! A collation's name fixes its behaviour for good. `Collation::version`
//! is stored with each collation a database uses, and a database whose
//! version differs is refused, since its indexes are in the old order.

mod folded;
mod pad;
mod reorder;
mod rules;
mod streaming;
pub(crate) mod table;
mod tailored;
mod unicode;
mod units;

use std::fmt;
use std::sync::Arc;

use super::{Binary, DataType, MAX_BINARY_LENGTH, Value};

pub use streaming::lob_equal;
pub use unicode::Unicode;
pub use units::Unit;

/// [ISO/IEC 9075-2:2023 §4.3.6] Ordering by Unicode scalar value.
pub const UCS_BASIC: &str = "UCS_BASIC";
/// [ISO/IEC 9075-2:2023 §4.3.6] Implementation-defined (IA003): as
/// `UCS_BASIC`.
pub const SQL_TEXT: &str = "SQL_TEXT";
/// [ISO/IEC 9075-2:2023 §4.3.6] The collation of ISO/IEC 14651:2020 without
/// delta.
pub const UNICODE: &str = "UNICODE";
/// \[EXTENSION\] `UCS_BASIC` of each character's simple case folding.
pub const CASE_INSENSITIVE: &str = "CASE_INSENSITIVE";
/// \[EXTENSION\] What the name of `UNICODE` tailored for a language begins
/// with, the language's CLDR locale after it: `UNICODE_SV`.
pub const TAILORED: &str = "UNICODE_";

/// The type of a sort key: how an index on a collated column holds it.
pub const SORT_KEY: DataType = DataType::VarBinary { length: MAX_BINARY_LENGTH };

/// [ISO/IEC 9075-2:2023 §4.3.2] How a collation compares strings of unequal
/// length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pad {
    /// The shorter is effectively extended with spaces.
    Space,
    /// The shorter, a prefix of the longer, is the less.
    None,
}

/// \[EXTENSION\] A collation's behaviour, the extension point of RULES.md
/// D60. Its contract, checked for each built-in by the tests of this
/// module:
///
/// * `sort_key` is a pure function of its arguments, and `version` changes
///   whenever it would give any string another key;
/// * two strings' keys, compared as bytes with a prefix the less, order
///   them as the collation does, and are equal only if the strings compare
///   equal;
/// * `fold` is `Some` of every character only if comparing folded
///   characters one by one agrees with `sort_key`, which LIKE relies on;
/// * `weighs` is true of a character that, alone, has a first-level
///   weight its keys hold, at the first level or, if it is variable, at
///   the fourth, so a string of `n` such characters has at least
///   `n / MOST_CONTRACTED` elements its keys hold, which LIKE's search
///   relies on;
/// * two strings compare equal exactly when, at each of its `levels`, the
///   `level_weights` of each are the same; and the weights at a level of a
///   string cut at the places `safe_cut` gives are the weights of its
///   pieces, joined, so a large object is compared a piece at a time
///   (`streaming.rs`, RULES.md D79); and the weights of `units`, joined
///   at each level, are the string's, which XQuery's substring matching
///   relies on.
pub trait Collation: Send + Sync {
    /// Its name.
    fn name(&self) -> &str;
    /// What fixes its behaviour.
    fn version(&self) -> &str;
    /// The sort key of `text` under `pad`, added to `out`.
    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>);
    /// The character `c` compares as, if its strings compare one character
    /// at a time.
    fn fold(&self, c: char) -> Option<char>;
    /// Whether `c` alone has a first-level weight its keys hold.
    fn weighs(&self, c: char) -> bool;
    /// The levels its comparisons have: one of a collation whose strings
    /// compare one character at a time.
    fn levels(&self) -> usize {
        1
    }
    /// The weights of `text` at `level`, from 1, under PAD NONE, added to
    /// `out`, as bytes compared for equality alone: of a collation whose
    /// strings compare one character at a time, each character folded.
    fn level_weights(&self, text: &str, level: usize, out: &mut Vec<u8>) {
        if level == 1 {
            let mut buffer = [0; 4];
            for c in text.chars() {
                out.extend_from_slice(self.fold(c).unwrap_or(c).encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    /// The last octet of `text` it may be cut at, so the weights of the
    /// pieces are those of the whole however it goes on: anywhere, of a
    /// collation whose strings compare one character at a time; none if
    /// nowhere, as of one that does not say.
    fn safe_cut(&self, text: &str) -> Option<usize> {
        self.fold('a').map(|_| text.len())
    }
    /// The collation units of `text` [XQueryFO §7.5], in order, each its
    /// weights at each of its `levels`, which, joined, are the
    /// `level_weights` of `text`: of a collation whose strings compare one
    /// character at a time, each character, its fold its weight; none if
    /// it splits no string into units, as of one that does not say.
    fn units(&self, text: &str) -> Option<Vec<Unit>> {
        text.chars().enumerate().map(|(start, c)| Some(Unit { start, weights: vec![self.fold(c)?.to_string().into_bytes()] })).collect()
    }
    /// \[DESIGN\] The sort keys of `texts` under `pad`, together: one
    /// round trip of a plug-in's collation for a batch of strings (RULES.md
    /// D221). Default: `sort_key` of each.
    ///
    /// # Errors
    ///
    /// A plug-in's collation's call fails.
    fn sort_keys(&self, texts: &[&str], pad: Pad) -> crate::error::Result<Vec<Vec<u8>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut key = Vec::with_capacity(t.len());
                self.sort_key(t, pad, &mut key);
                key
            })
            .collect())
    }
    /// \[DESIGN\] Whether `sort_keys` is worth calling with many strings
    /// at once, as a plug-in's is. Default: no.
    fn batches(&self) -> bool {
        false
    }
}

/// \[EXTENSION\] That `c` compares strings as LIKE, TRIM, XQuery and an
/// index ask, a character or a key at a time, perhaps under a lock: not a
/// plug-in's, whose keys are calls made a batch at a time (RULES.md D221).
///
/// # Errors
///
/// SQLCODE -103 if it is a plug-in's.
pub fn character_wise(c: &CollationRef, what: &str) -> crate::error::Result<()> {
    if !c.collation.batches() {
        return Ok(());
    }
    let message = format!(
        "[EXTENSION] {what} compares no strings by the plug-in collation {}, whose keys are calls made a batch at a time",
        c.collation.name()
    );
    Err(crate::error::SqlError::new(crate::error::Code::SyntaxRule, message))
}

/// \[EXTENSION\] The most characters one element of a key is made of: the
/// longest string a tailoring names, an emoji sequence of eight.
pub const MOST_CONTRACTED: usize = 8;

/// A collation and its pad characteristic [ISO/IEC 9075-2:2023 §11.43 GR5].
#[derive(Clone)]
pub struct CollationRef {
    /// How it orders strings.
    pub collation: Arc<dyn Collation>,
    /// How it treats strings of unequal length.
    pub pad: Pad,
}

impl CollationRef {
    /// Whether it orders as a comparison with no collation does, so its
    /// comparisons stay bare.
    pub fn is_default(&self) -> bool {
        self.pad == Pad::Space && self.is_scalar()
    }

    /// [ISO/IEC 9075-2:2023 §4.3.2] Whether it orders strings as their
    /// octets order, a prefix the less: `UCS_BASIC` or `SQL_TEXT` with NO
    /// PAD, whose sort key is the text itself. Its comparisons are made of
    /// the strings in place, no key built (RULES.md D1183).
    pub fn is_octets(&self) -> bool {
        self.pad == Pad::None && self.is_scalar()
    }

    /// Whether it compares each character as itself, by its Unicode scalar
    /// value: `UCS_BASIC` or `SQL_TEXT`, of either pad characteristic. LIKE
    /// under it, which never pads [ISO/IEC 9075-2:2023 §8.5 SR3d], is LIKE
    /// of the strings as they are.
    pub fn is_scalar(&self) -> bool {
        matches!(self.collation.name(), UCS_BASIC | SQL_TEXT)
    }

    /// Whether it behaves as `other`: an index of the one serves the other.
    pub fn behaves_as(&self, other: &CollationRef) -> bool {
        let scalar = |c: &CollationRef| matches!(c.collation.name(), UCS_BASIC | SQL_TEXT);
        self.pad == other.pad && (self.collation.name() == other.collation.name() || (scalar(self) && scalar(other)))
    }

    /// `v`'s sort key, as a binary string; a null stays null, and anything
    /// but a character string stays as it is.
    ///
    /// # Errors
    ///
    /// A plug-in's collation's call fails (RULES.md D221).
    pub fn sort_key(&self, v: &Value) -> crate::error::Result<Value> {
        self.sort_keys(std::slice::from_ref(v)).map(|mut keys| keys.pop().unwrap_or(Value::Null))
    }

    /// The sort key of each of `values`, as `sort_key` gives it, the
    /// character strings' asked of the collation together.
    ///
    /// # Errors
    ///
    /// As `sort_key`.
    pub fn sort_keys(&self, values: &[Value]) -> crate::error::Result<Vec<Value>> {
        let texts: Vec<&str> = values.iter().filter_map(|v| if let Value::Char(t) = v { Some(t.as_str()) } else { None }).collect();
        let mut keys = self.collation.sort_keys(&texts, self.pad)?.into_iter();
        Ok(values
            .iter()
            .map(|v| if let Value::Char(_) = v { Value::Binary(Binary::new(keys.next().unwrap_or_default())) } else { v.clone() })
            .collect())
    }
}

impl fmt::Debug for CollationRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}", self.collation.name(), self.pad)
    }
}

impl PartialEq for CollationRef {
    fn eq(&self, other: &CollationRef) -> bool {
        self.behaves_as(other)
    }
}

/// `UCS_BASIC` with NO PAD, whose key is the text itself, for tests.
#[cfg(test)]
pub(crate) fn unpadded_scalar() -> CollationRef {
    CollationRef { collation: Arc::new(folded::Scalar(UCS_BASIC)), pad: Pad::None }
}

/// The collations every database has. A table that cannot be read leaves
/// out those that read it, so naming one is refused; the test of this module
/// holds the table to its digest.
pub fn builtin() -> Vec<Arc<dyn Collation>> {
    let mut all: Vec<Arc<dyn Collation>> = vec![Arc::new(folded::Scalar(UCS_BASIC)), Arc::new(folded::Scalar(SQL_TEXT))];
    if let Ok(table) = table::Table::read().map(Arc::new) {
        all.push(Arc::new(folded::Folded(Arc::clone(&table))));
        all.extend(tailored::all(&table).into_iter().map(|t| -> Arc<dyn Collation> { Arc::new(t) }));
        all.push(Arc::new(Unicode::new(table)));
    }
    all
}

/// [ISO/IEC 9075-2:2023 §4.3.6] A built-in's pad characteristic. UNICODE
/// conforms to ISO/IEC 14651 without delta, and padding would be one; its
/// tailorings pad as it does.
pub fn default_pad(name: &str) -> Pad {
    if name == UNICODE || name.starts_with(TAILORED) { Pad::None } else { Pad::Space }
}

#[cfg(test)]
mod tests;
