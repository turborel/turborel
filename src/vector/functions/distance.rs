//! \[EXTENSION\] VECTOR_DISTANCE in both argument orders (design document
//! §6.1) and the shorthand distances (§6.3). A character string as the
//! first argument selects the order that names the metric first (R6.1);
//! the metric is a name in any case (R6.2), COSINE when not given (R6.3,
//! which also asks for the metric of an index on the first vector's
//! column, a choice the binder makes by giving it as an argument). The
//! result is a FLOAT64, smaller for more similar vectors (R6.5, R6.6).

use super::{Arg, text_at, vector_at};
use crate::error::Result;
use crate::value::Value;
use crate::vector::metric::{self, COSINE, DOT, EUCLIDEAN, HAMMING, JACCARD, MANHATTAN, Metrics, metric_named};
use crate::vector::mismatch;

/// A distance function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distance {
    /// VECTOR_DISTANCE(v1, v2 [, metric]) or VECTOR_DISTANCE(metric, v1, v2).
    VectorDistance,
    /// L1_DISTANCE(a, b), MANHATTAN.
    L1,
    /// L2_DISTANCE(a, b), EUCLIDEAN.
    L2,
    /// COSINE_DISTANCE(a, b).
    Cosine,
    /// HAMMING_DISTANCE(a, b).
    Hamming,
    /// JACCARD_DISTANCE(a, b).
    Jaccard,
    /// INNER_PRODUCT(a, b), `a·b`, not negated.
    InnerProduct,
}

impl Distance {
    /// Every distance function.
    pub const ALL: [Distance; 7] = [
        Distance::VectorDistance,
        Distance::L1,
        Distance::L2,
        Distance::Cosine,
        Distance::Hamming,
        Distance::Jaccard,
        Distance::InnerProduct,
    ];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Distance::VectorDistance => super::VECTOR_DISTANCE,
            Distance::L1 => "L1_DISTANCE",
            Distance::L2 => "L2_DISTANCE",
            Distance::Cosine => "COSINE_DISTANCE",
            Distance::Hamming => "HAMMING_DISTANCE",
            Distance::Jaccard => "JACCARD_DISTANCE",
            Distance::InnerProduct => "INNER_PRODUCT",
        }
    }

    /// The metric it measures by, if fixed. INNER_PRODUCT measures by DOT,
    /// its sign turned back.
    pub fn metric(self) -> Option<&'static str> {
        match self {
            Distance::VectorDistance => None,
            Distance::L1 => Some(MANHATTAN),
            Distance::L2 => Some(EUCLIDEAN),
            Distance::Cosine => Some(COSINE),
            Distance::Hamming => Some(HAMMING),
            Distance::Jaccard => Some(JACCARD),
            Distance::InnerProduct => Some(DOT),
        }
    }

    pub(super) fn arity(self) -> (usize, usize) {
        match self {
            Distance::VectorDistance => (2, 3),
            Distance::L1 | Distance::L2 | Distance::Cosine | Distance::Hamming | Distance::Jaccard | Distance::InnerProduct => (2, 2),
        }
    }

    pub(super) fn arg(self, at: usize, text_first: bool) -> Option<Arg> {
        let general = matches!(self, Distance::VectorDistance);
        match (at, general && text_first) {
            (0, true) => Some(Arg::Text),
            (0 | 1, _) | (2, true) => Some(Arg::Vector),
            (2, false) => general.then_some(Arg::Text),
            _ => None,
        }
    }

    /// The metric a call of `args` names, and the position of its first
    /// vector.
    pub(super) fn chosen(self, args: &[Value]) -> Result<(String, usize)> {
        let named_first = matches!(self, Distance::VectorDistance) && matches!(args.first(), Some(Value::Char(_)));
        Ok(match (self.metric(), named_first) {
            (Some(fixed), _) => (fixed.to_owned(), 0),
            (None, true) => (text_at(args, 0)?.unwrap_or(COSINE).to_owned(), 1),
            (None, false) => (text_at(args, 2)?.unwrap_or(COSINE).to_owned(), 0),
        })
    }

    pub(super) fn call(self, args: &[Value], metrics: &Metrics) -> Result<Value> {
        let (name, first) = self.chosen(args)?;
        let found = metric_named(metrics, &name).ok_or_else(|| mismatch(&format!("unknown metric or distance function {name}")))?;
        let (a, b) = (vector_at(args, first)?, vector_at(args, first.saturating_add(1))?);
        let d = metric::distance(found.as_ref(), a, b)?;
        Ok(match (d, self) {
            (None, _) => Value::Null,
            (Some(d), Distance::InnerProduct) => Value::Approx(-d),
            (
                Some(d),
                Distance::VectorDistance | Distance::L1 | Distance::L2 | Distance::Cosine | Distance::Hamming | Distance::Jaccard,
            ) => Value::Approx(d),
        })
    }
}
