//! Decimals of any number of digits [Schema2 §3.2.3]: the value of a
//! decimal or integer literal, its order, its canonical representation,
//! and the digits the totalDigits and fractionDigits facets count.

use std::cmp::Ordering;

use super::number::parts;

/// A decimal: i × 10^-n, kept as its sign and its digits before and after
/// the period, none leading or trailing, so each value has one form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exact {
    /// Whether it is less than zero; never of zero.
    negative: bool,
    /// Its digits before the period, no zero leading; empty below one.
    whole: String,
    /// Its digits after the period, no zero trailing.
    fraction: String,
}

impl Exact {
    /// The value of `text`, if it is a lexical form of `xs:decimal`
    /// [Schema2 §3.2.3.1], which every lexical form of `xs:integer` is.
    #[must_use]
    pub fn parse(text: &str) -> Option<Exact> {
        let (negative, whole, fraction) = parts(text)?;
        let whole = whole.trim_start_matches('0').to_owned();
        let fraction = fraction.trim_end_matches('0').to_owned();
        let zero = whole.is_empty() && fraction.is_empty();
        Some(Exact { negative: negative && !zero, whole, fraction })
    }

    /// Whether it is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.whole.is_empty() && self.fraction.is_empty()
    }

    /// Whether it is less than zero.
    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// [Schema2 §3.2.3, §4.3.11.3] Its total digits: the least p such that
    /// it is i × 10^-n with n no greater than p and i of no more than p
    /// significant digits; 1 of zero, as the facet's value is at least 1.
    #[must_use]
    pub fn total_digits(&self) -> usize {
        self.whole.len().saturating_add(self.fraction.len()).max(1)
    }

    /// [Schema2 §3.2.3, §4.3.12.3] Its fraction digits: the least n such
    /// that it is i × 10^-n.
    #[must_use]
    pub fn fraction_digits(&self) -> usize {
        self.fraction.len()
    }

    /// The order of its magnitude and `other`'s.
    fn magnitude(&self, other: &Exact) -> Ordering {
        self.whole.len().cmp(&other.whole.len()).then_with(|| self.whole.cmp(&other.whole)).then_with(|| self.fraction.cmp(&other.fraction))
    }

    /// [Schema2 §3.2.3] Its order with `other`: x < y iff y - x is
    /// positive, a total order.
    #[must_use]
    pub fn compare(&self, other: &Exact) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.magnitude(other),
            (true, true) => other.magnitude(self),
        }
    }

    /// Its canonical representation [Schema2 §3.2.3.2]: no sign but a
    /// minus, a digit at least on either side of the period; or, as an
    /// integer, no period [Schema2 §3.3.13.2].
    #[must_use]
    pub fn canonical(&self, integer: bool) -> String {
        let sign = if self.negative { "-" } else { "" };
        let whole = if self.whole.is_empty() { "0" } else { &self.whole };
        if integer {
            return format!("{sign}{whole}");
        }
        let fraction = if self.fraction.is_empty() { "0" } else { &self.fraction };
        format!("{sign}{whole}.{fraction}")
    }
}
