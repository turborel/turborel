//! \[EXTENSION\] [ISO/IEC 9075-2:2023 §4.3.6] Collations (RULES.md D60): how
//! character strings compare, each collation a way of turning a string into
//! a sort key, bytes that compare as the strings do.
//!
//! A collation of the default behaviour (`CollationRef::is_default`) makes
//! no sort key: its comparisons stay bare, and `value/comparison.rs` judges
//! them. Any other reaches comparison as `Value::Bits` of the sort key, so
//! ordering, grouping, hashing and spilling need not know of collations,
//! and an index on a collated column holds sort keys.
//!
//! A collation's name fixes its behaviour for good. `Collation::version`
//! is stored with each collation a database uses, and a database whose
//! version differs is refused, since its indexes are in the old order.

mod folded;
mod pad;
mod table;
mod unicode;

use std::fmt;
use std::sync::Arc;

use super::{Bits, DataType, MAX_BIT_LENGTH, Value};

pub use unicode::Unicode;

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

/// The type of a sort key: how an index on a collated column holds it.
pub const SORT_KEY: DataType = DataType::BitVarying { length: MAX_BIT_LENGTH };

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
///   characters one by one agrees with `sort_key`, which LIKE relies on.
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
}

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
        self.pad == Pad::Space && matches!(self.collation.name(), UCS_BASIC | SQL_TEXT)
    }

    /// Whether it behaves as `other`: an index of the one serves the other.
    pub fn behaves_as(&self, other: &CollationRef) -> bool {
        let scalar = |c: &CollationRef| matches!(c.collation.name(), UCS_BASIC | SQL_TEXT);
        self.pad == other.pad && (self.collation.name() == other.collation.name() || (scalar(self) && scalar(other)))
    }

    /// `v`'s sort key, as bits; a null stays null, and anything but a
    /// character string stays as it is.
    pub fn sort_key(&self, v: &Value) -> Value {
        let Value::Char(text) = v else { return v.clone() };
        let mut key = Vec::with_capacity(text.len());
        self.collation.sort_key(text, self.pad, &mut key);
        let bits = u32::try_from(key.len()).ok().and_then(|n| n.checked_mul(super::BYTE_BITS));
        bits.and_then(|len| Bits::new(key, len).ok()).map_or_else(|| v.clone(), Value::Bits)
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

/// The collations every database has. A table that cannot be read leaves
/// out those that read it, so naming one is refused; the test of this module
/// holds the table to its digest.
pub fn builtin() -> Vec<Arc<dyn Collation>> {
    let mut all: Vec<Arc<dyn Collation>> = vec![Arc::new(folded::Scalar(UCS_BASIC)), Arc::new(folded::Scalar(SQL_TEXT))];
    if let Ok(table) = table::Table::read().map(Arc::new) {
        all.push(Arc::new(folded::Folded(Arc::clone(&table))));
        all.push(Arc::new(Unicode::new(table)));
    }
    all
}

/// [ISO/IEC 9075-2:2023 §4.3.6] A built-in's pad characteristic. UNICODE
/// conforms to ISO/IEC 14651 without delta, and padding would be one.
pub fn default_pad(name: &str) -> Pad {
    if name == UNICODE { Pad::None } else { Pad::Space }
}

#[cfg(test)]
mod tests;
