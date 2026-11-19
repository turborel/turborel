//! Collations, by which a query compares strings [XQueryFO §7.3.1]: the
//! Unicode codepoint collation, the default [XQuery Appendix C.1], and
//! \[EXTENSION\] the database's collations (RULES.md D60), each reached by
//! an implementation-defined URI, `SQL_COLLATION` then its name, so
//! `urn:turborel:collation:UNICODE` compares as UNICODE does. Each splits
//! a string into collation units for substring matching [XQueryFO §7.5].

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use super::error::{ErrorCode, error};
use crate::error::Result;
use crate::value::collation::{Collation, Pad};

/// [XQueryFO §7.3.1] The URI of the Unicode codepoint collation.
pub const CODEPOINT: &str = "http://www.w3.org/2005/xpath-functions/collation/codepoint";

/// \[EXTENSION\] Implementation-defined [XQuery Appendix C.1]: what the URI
/// of each collation of the database begins with, its name after it.
pub const SQL_COLLATION: &str = "urn:turborel:collation:";

/// The collations a database has, by name (RULES.md D60).
pub type Collations = BTreeMap<String, Arc<dyn Collation>>;

/// A collation of the statically known collations [XQuery §2.1.1]: the
/// codepoint collation, or one of the database's.
#[derive(Clone)]
pub struct Collator {
    /// The URI it is known by.
    uri: Box<str>,
    /// The database's collation it is, none for the codepoint collation.
    by: Option<Arc<dyn Collation>>,
}

impl fmt::Debug for Collator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Collator({})", self.uri)
    }
}

impl Collator {
    /// The Unicode codepoint collation [XQueryFO §7.3.1].
    #[must_use]
    pub fn codepoint() -> Collator {
        Collator { uri: CODEPOINT.into(), by: None }
    }

    /// The collation `uri` names among `collations`' and the codepoint
    /// collation, if one is named so [XQueryFO §7.3.1].
    #[must_use]
    pub fn named(uri: &str, collations: &Collations) -> Option<Collator> {
        if uri == CODEPOINT {
            return Some(Collator::codepoint());
        }
        let by = collations.get(uri.strip_prefix(SQL_COLLATION)?)?;
        Some(Collator { uri: uri.into(), by: Some(Arc::clone(by)) })
    }

    /// The URI it is known by.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// How `a` compares with `b` by it [XQueryFO §7.3.2]: by codepoints,
    /// or by the collation's sort keys of no padding, as XQuery pads no
    /// string.
    #[must_use]
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        let Some(by) = &self.by else { return a.cmp(b) };
        let (mut x, mut y) = (Vec::new(), Vec::new());
        by.sort_key(a, Pad::None, &mut x);
        by.sort_key(b, Pad::None, &mut y);
        x.cmp(&y)
    }

    /// Whether `a` and `b` are equal by it [XQueryFO §7.3.2].
    #[must_use]
    pub fn equal(&self, a: &str, b: &str) -> bool {
        self.compare(a, b) == Ordering::Equal
    }

    /// The collation units of `text` [XQueryFO §7.5]: of the codepoint
    /// collation its characters, each weighed as its UTF-8; of one of the
    /// database's, those it splits it into (`Collation::units`): of one
    /// that compares strings a character at a time each character, its
    /// fold its weight; of `UNICODE` and its tailorings a character and
    /// the marks after it, or a contraction, each weighed at every level
    /// as it is weighed in the string.
    ///
    /// # Errors
    ///
    /// err:FOCH0004 if the collation splits no string into units.
    pub fn units(&self, text: &str) -> Result<Units> {
        let length = text.chars().count();
        let Some(by) = &self.by else {
            let mut level = Level::default();
            for c in text.chars() {
                level.weights.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                level.ends.push(level.weights.len());
            }
            return Ok(Units { bounds: (0..=length).collect(), levels: vec![level] });
        };
        let units = by.units(text).ok_or_else(|| {
            let why = format!("the collation {} does not split strings into collation units [XQueryFO §7.5]", self.uri);
            error(ErrorCode::FOCH0004, &why)
        })?;
        let mut bounds: Vec<usize> = units.iter().map(|u| u.start).collect();
        bounds.push(length);
        let mut levels = vec![Level::default(); by.levels()];
        for unit in units {
            for (level, weights) in levels.iter_mut().zip(unit.weights) {
                level.weights.extend(weights);
                level.ends.push(level.weights.len());
            }
        }
        Ok(Units { bounds, levels })
    }
}

/// One level of the weights of a string's collation units: each unit's,
/// joined, and where each unit's end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Level {
    weights: Vec<u8>,
    ends: Vec<usize>,
}

/// A string split into collation units [XQueryFO §7.5] by a collation
/// (`Collator::units`): where each unit begins, and its weights at each of
/// the collation's levels, each as bytes compared for equality alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Units {
    /// The character each unit begins at, then the string's length.
    bounds: Vec<usize>,
    /// The units' weights, a level each.
    levels: Vec<Level>,
}

impl Units {
    /// How many units there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bounds.len().saturating_sub(1)
    }

    /// Whether there are none, of the zero-length string.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many levels the units are weighed at.
    #[must_use]
    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    /// The characters of unit `unit`: where it begins and where it ends.
    #[must_use]
    pub fn characters(&self, unit: usize) -> (usize, usize) {
        let at = |n: usize| self.bounds.get(n).copied().unwrap_or(0);
        (at(unit), at(unit.saturating_add(1)))
    }

    /// The weights of unit `unit` at level `level`, from 0.
    #[must_use]
    pub fn weights(&self, level: usize, unit: usize) -> &[u8] {
        let Some(l) = self.levels.get(level) else { return &[] };
        let from = unit.checked_sub(1).and_then(|before| l.ends.get(before)).copied().unwrap_or(0);
        let to = l.ends.get(unit).copied().unwrap_or(from);
        l.weights.get(from..to).unwrap_or_default()
    }

    /// The weights of every unit at level `level`, from 0, joined.
    #[must_use]
    pub fn joined(&self, level: usize) -> &[u8] {
        self.levels.get(level).map_or(&[], |l| l.weights.as_slice())
    }

    /// Whether unit `unit` is ignorable, of no weight at any level
    /// [XQueryFO §7.5.1].
    #[must_use]
    pub fn ignorable(&self, unit: usize) -> bool {
        (0..self.levels()).all(|level| self.weights(level, unit).is_empty())
    }
}

#[cfg(test)]
mod tests;
