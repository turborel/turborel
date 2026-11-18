//! \[EXTENSION\] The built-in metrics of the design document's §6.2
//! table, each over the formats the table gives it.

use std::sync::Arc;

use super::metric::{COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, HAMMING, JACCARD, MANHATTAN, MetricProperties, VectorMetric};
use super::terms::{Term, and_or, norm, sum};
use super::{Element, VectorRef};
use crate::error::Result;

/// The formats of every metric but HAMMING and JACCARD: all but BINARY.
const NUMBERS: [Element; 4] = [Element::Float32, Element::Float16, Element::Float64, Element::Int8];
/// HAMMING's formats: all.
const ALL: [Element; 5] = Element::ALL;
/// JACCARD's format: BINARY.
const BITS: [Element; 1] = [Element::Binary];

/// A built-in metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltIn {
    /// COSINE.
    Cosine,
    /// DOT.
    Dot,
    /// EUCLIDEAN.
    Euclidean,
    /// EUCLIDEAN_SQUARED.
    EuclideanSquared,
    /// MANHATTAN.
    Manhattan,
    /// HAMMING.
    Hamming,
    /// JACCARD.
    Jaccard,
}

impl BuiltIn {
    /// Every built-in metric.
    pub const ALL: [BuiltIn; 7] = [
        BuiltIn::Cosine,
        BuiltIn::Dot,
        BuiltIn::Euclidean,
        BuiltIn::EuclideanSquared,
        BuiltIn::Manhattan,
        BuiltIn::Hamming,
        BuiltIn::Jaccard,
    ];
}

/// The built-in metrics, as `Metrics` gives them.
pub(crate) fn metrics() -> Vec<Arc<dyn VectorMetric>> {
    BuiltIn::ALL.into_iter().map(|m| -> Arc<dyn VectorMetric> { Arc::new(m) }).collect()
}

/// Whether `name` is a built-in metric's.
pub(crate) fn is_built_in(name: &str) -> bool {
    BuiltIn::ALL.iter().any(|m| m.name() == name)
}

/// A true metric's properties.
const METRIC: MetricProperties = MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None };

fn cosine(a: VectorRef<'_>, b: VectorRef<'_>) -> Option<f64> {
    let scale = norm(a) * norm(b);
    if scale <= 0.0_f64 {
        // R6.7: COSINE with a zero vector is NULL.
        return None;
    }
    // Rounding can take a cosine just past 1 or -1; the distance stays in
    // [0, 2] so it is never negative.
    Some((1.0 - sum(Term::Dot, a, b) / scale).clamp(0.0, 2.0))
}

fn jaccard(a: VectorRef<'_>, b: VectorRef<'_>) -> f64 {
    let (both, either) = and_or(a, b);
    // Two empty sets are taken as the same set.
    if either <= 0.0 { 0.0 } else { 1.0 - both / either }
}

impl VectorMetric for BuiltIn {
    fn name(&self) -> &'static str {
        match self {
            BuiltIn::Cosine => COSINE,
            BuiltIn::Dot => DOT,
            BuiltIn::Euclidean => EUCLIDEAN,
            BuiltIn::EuclideanSquared => EUCLIDEAN_SQUARED,
            BuiltIn::Manhattan => MANHATTAN,
            BuiltIn::Hamming => HAMMING,
            BuiltIn::Jaccard => JACCARD,
        }
    }

    fn formats(&self) -> &[Element] {
        match self {
            BuiltIn::Cosine | BuiltIn::Dot | BuiltIn::Euclidean | BuiltIn::EuclideanSquared | BuiltIn::Manhattan => &NUMBERS,
            BuiltIn::Hamming => &ALL,
            BuiltIn::Jaccard => &BITS,
        }
    }

    fn properties(&self) -> MetricProperties {
        match self {
            BuiltIn::Cosine => MetricProperties { triangle: false, normalized_as: Some(EUCLIDEAN), ..METRIC },
            BuiltIn::Dot => MetricProperties { non_negative: false, triangle: false, normalized_as: Some(EUCLIDEAN), ..METRIC },
            BuiltIn::Euclidean | BuiltIn::Manhattan | BuiltIn::Hamming | BuiltIn::Jaccard => METRIC,
            BuiltIn::EuclideanSquared => MetricProperties { triangle: false, ..METRIC },
        }
    }

    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
        Ok(match self {
            BuiltIn::Cosine => cosine(a, b),
            BuiltIn::Dot => Some(-sum(Term::Dot, a, b)),
            BuiltIn::Euclidean => Some(sum(Term::Square, a, b).sqrt()),
            BuiltIn::EuclideanSquared => Some(sum(Term::Square, a, b)),
            BuiltIn::Manhattan => Some(sum(Term::Gap, a, b)),
            BuiltIn::Hamming => Some(sum(Term::Differ, a, b)),
            BuiltIn::Jaccard => Some(jaccard(a, b)),
        })
    }

    fn lower_bound(&self, query: VectorRef<'_>, low: &[f32], high: &[f32]) -> Option<f64> {
        let mut gaps = (0..query.dims).zip(low.iter().zip(high)).map(|(d, (lo, hi))| {
            let q = query.at(d).unwrap_or_default();
            (f64::from(*lo) - q).max(q - f64::from(*hi)).max(0.0)
        });
        match self {
            BuiltIn::Euclidean => Some(gaps.map(|g| g * g).sum::<f64>().sqrt()),
            BuiltIn::EuclideanSquared => Some(gaps.map(|g| g * g).sum()),
            BuiltIn::Manhattan => Some(gaps.by_ref().sum()),
            BuiltIn::Cosine | BuiltIn::Dot | BuiltIn::Hamming | BuiltIn::Jaccard => None,
        }
    }
}
