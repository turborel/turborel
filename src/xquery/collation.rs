//! Collations, by which a query compares strings [XQueryFO §7.3.1]: the
//! Unicode codepoint collation, the default [XQuery Appendix C.1], and
//! \[EXTENSION\] the database's collations (RULES.md D60), each reached by
//! an implementation-defined URI, `SQL_COLLATION` then its name, so
//! `urn:turborel:collation:UNICODE` compares as UNICODE does.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

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
}

#[cfg(test)]
mod tests;
