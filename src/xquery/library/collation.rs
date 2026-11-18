//! Collations [XQueryFO §7.3.1]: which one a function uses, and how it
//! compares strings and splits them into collation units.
//!
//! \[DESIGN\] RULES.md D60: the collations a function names are the code
//! point collation [XQueryFO §7.3.1] and each built-in collation of
//! turborel, named by its SQL name as a URI (`UNICODE`, `UNICODE_SV`,
//! `CASE_INSENSITIVE`), an implementation-defined choice of names; any
//! other is err:FOCH0002. A collation compares strings by their sort keys
//! with no padding, as XQuery pads no string. The collation units of
//! substring matching [XQueryFO §7.5] are characters, of the code point
//! collation and of each collation that compares strings one character
//! at a time, its folds; a collation that does not (`UNICODE` and its
//! tailorings) cannot split a string into units, err:FOCH0004.

use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};

use super::codes::{FOCH0002, FOCH0004, fail};
use super::{CODEPOINT_COLLATION, Focus};
use crate::error::Result;
use crate::value::collation::{Collation, Pad, builtin};

/// A collation a function compares strings by.
#[derive(Clone)]
pub enum Collator {
    /// The Unicode code point collation [XQueryFO §7.3.1].
    Codepoint,
    /// A collation of turborel (RULES.md D60).
    Named(Arc<dyn Collation>),
}

impl std::fmt::Debug for Collator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Collator::Codepoint => write!(f, "{CODEPOINT_COLLATION}"),
            Collator::Named(c) => write!(f, "{}", c.name()),
        }
    }
}

impl Collator {
    /// The collation `uri` names [XQueryFO §7.3.1].
    ///
    /// # Errors
    ///
    /// err:FOCH0002 if it names none turborel supports.
    pub fn named(uri: &str) -> Result<Collator> {
        if uri == CODEPOINT_COLLATION {
            return Ok(Collator::Codepoint);
        }
        collations()
            .iter()
            .find(|c| c.name() == uri)
            .map(|c| Collator::Named(Arc::clone(c)))
            .ok_or_else(|| fail(FOCH0002, &format!("the collation '{uri}' is not supported [XQueryFO §7.3.1]")))
    }

    /// The collation of `focus` and the argument `given`: the one it names
    /// if given, else the default collation [XQueryFO §7.3.1].
    ///
    /// # Errors
    ///
    /// err:FOCH0002 if that one is not supported.
    pub fn of(focus: &Focus, given: Option<&str>) -> Result<Collator> {
        Collator::named(given.unwrap_or(&focus.collation))
    }

    /// The order of `first` and `second` [XQueryFO §7.3.2].
    #[must_use]
    pub fn compare(&self, first: &str, second: &str) -> Ordering {
        match self {
            // Strings ordered by their UTF-8 octets are ordered by code point.
            Collator::Codepoint => first.cmp(second),
            Collator::Named(collation) => {
                let (mut left, mut right) = (Vec::new(), Vec::new());
                collation.sort_key(first, Pad::None, &mut left);
                collation.sort_key(second, Pad::None, &mut right);
                left.cmp(&right)
            }
        }
    }

    /// Whether `a` and `b` are equal by it.
    #[must_use]
    pub fn equal(&self, a: &str, b: &str) -> bool {
        self.compare(a, b) == Ordering::Equal
    }

    /// The collation units of `text` [XQueryFO §7.5]: its characters, each
    /// folded as the collation compares it.
    ///
    /// # Errors
    ///
    /// err:FOCH0004 if the collation does not compare one character at a
    /// time.
    pub fn units(&self, text: &str) -> Result<Vec<char>> {
        match self {
            Collator::Codepoint => Ok(text.chars().collect()),
            Collator::Named(c) => text.chars().map(|ch| c.fold(ch).ok_or_else(|| no_units(c.name()))).collect(),
        }
    }
}

/// \[DESIGN\] The built-in collations, made once and shared, as each
/// tailoring makes its weights once (RULES.md D60).
fn collations() -> &'static [Arc<dyn Collation>] {
    static BUILT: OnceLock<Vec<Arc<dyn Collation>>> = OnceLock::new();
    BUILT.get_or_init(builtin)
}

/// The error of a collation that splits no string into units.
fn no_units(name: &str) -> crate::error::SqlError {
    fail(FOCH0004, &format!("the collation '{name}' does not split strings into collation units [XQueryFO §7.5]"))
}

#[cfg(test)]
mod tests;
