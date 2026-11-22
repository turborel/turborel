//! \[EXTENSION\] Distance metrics (design document §6.2, §8): the
//! `VectorMetric` extension point, its built-in implementations
//! (`builtin.rs`), a metric made of a callback for user-defined distance
//! functions, a lookup by name, and the checks every metric's results
//! pass.
//!
//! Contract (RULES.md R-SAFE-3), for every implementation:
//! * `name` is fixed, upper case and unique among the registered metrics.
//! * `distance` is called only with two vectors of one dimension count
//!   and one element format among `formats`; it is deterministic, bit for
//!   bit, and never panics. It answers a finite number, smaller for more
//!   similar vectors (R6.6), or none for SQL NULL, as COSINE does of a
//!   zero vector (R6.7).
//! * `properties` says truly whether it is symmetric, never negative, and
//!   meets the triangle inequality, since a graph index relies on the
//!   first two (R8.2) and builds spot-check them (R8.3).
//! * It calls back into no `Database` and holds no lock across a call
//!   (RULES.md R-ERR-4).

use std::sync::Arc;

use super::{Element, Layout, VectorRef, mismatch, out_of_range};
use crate::error::{Code, Result, SqlError};
use crate::kernels::{Kernels, Portable};

/// COSINE: `1 - a·b / (|a| |b|)`.
pub const COSINE: &str = "COSINE";
/// DOT: `-(a·b)`, negated so smaller is more similar (R6.8).
pub const DOT: &str = "DOT";
/// EUCLIDEAN: `sqrt(Σ (a_i - b_i)^2)`.
pub const EUCLIDEAN: &str = "EUCLIDEAN";
/// EUCLIDEAN_SQUARED: `Σ (a_i - b_i)^2`.
pub const EUCLIDEAN_SQUARED: &str = "EUCLIDEAN_SQUARED";
/// L2_SQUARED, EUCLIDEAN_SQUARED's alias.
pub const L2_SQUARED: &str = "L2_SQUARED";
/// MANHATTAN: `Σ |a_i - b_i|`.
pub const MANHATTAN: &str = "MANHATTAN";
/// HAMMING: the count of elements that differ, of bits for BINARY.
pub const HAMMING: &str = "HAMMING";
/// JACCARD: `1 - |a ∧ b| / |a ∨ b|`, of BINARY vectors.
pub const JACCARD: &str = "JACCARD";

/// What a metric promises of its distances (design document §8
/// PROPERTIES).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricProperties {
    /// `d(a, b) = d(b, a)`.
    pub symmetric: bool,
    /// `d(a, b) >= 0`.
    pub non_negative: bool,
    /// `d(a, c) <= d(a, b) + d(b, c)`.
    pub triangle: bool,
    /// The metric that orders as this one does once both vectors are
    /// normalized, if one does: COSINE and DOT order as EUCLIDEAN.
    pub normalized_as: Option<&'static str>,
}

/// A distance metric, an extension point (RULES.md R-SAFE-3): see the
/// module's contract.
pub trait VectorMetric: Send + Sync {
    /// Its name: fixed, upper case, unique.
    fn name(&self) -> &str;
    /// The element formats it measures.
    fn formats(&self) -> &[Element];
    /// What it promises.
    fn properties(&self) -> MetricProperties;
    /// The distance from `a` to `b`, of one dimension count and format;
    /// none for SQL NULL.
    ///
    /// # Errors
    ///
    /// A user-defined metric's failure; a built-in one fails never.
    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>>;
    /// A bound no distance from `query` to a vector inside the box `low`
    /// to `high` is below, if it can give one. Default: none.
    fn lower_bound(&self, _query: VectorRef<'_>, _low: &[f32], _high: &[f32]) -> Option<f64> {
        None
    }
    /// The distances from `query` to each of `candidates`, in order, each
    /// as `distance` gives it: one call for many, as a plug-in's metric
    /// measures them in one round trip (RULES.md D169). Default: `distance`
    /// of each.
    ///
    /// # Errors
    ///
    /// As `distance`.
    fn distances(&self, query: VectorRef<'_>, candidates: &[VectorRef<'_>]) -> Result<Vec<Option<f64>>> {
        candidates.iter().map(|c| self.distance(query, *c)).collect()
    }
    /// Whether `distances` is cheaper than `distance` of each, as a
    /// plug-in's is: the engines then gather a node's neighbours before
    /// they measure them. Default: no.
    fn batches(&self) -> bool {
        false
    }
    /// \[DESIGN\] The kernels it sums on. A built-in metric of a database
    /// sums on the database's (RULES.md D571), and so do the vector indexes
    /// and quantizers that measure by it. Default: the portable ones.
    fn kernels(&self) -> &dyn Kernels {
        &Portable
    }
}

/// The metrics a database knows, given fresh on each call: the built-in
/// ones, then those registered.
pub type Metrics = Arc<dyn Fn() -> Vec<Arc<dyn VectorMetric>> + Send + Sync>;

/// The built-in metrics alone.
pub fn built_in_metrics() -> Metrics {
    Arc::new(super::builtin::metrics)
}

/// The built-in metrics, summing on `kernels`, a database's registered
/// set, or on the portable ones if none is given.
pub(crate) fn built_in_on(kernels: Option<&Arc<dyn Kernels>>) -> Vec<Arc<dyn VectorMetric>> {
    super::builtin::metrics_on(kernels)
}

/// Metrics of a fixed list: the built-in ones, then `extra`.
pub fn metrics_with(extra: Vec<Arc<dyn VectorMetric>>) -> Metrics {
    Arc::new(move || {
        let mut all = super::builtin::metrics();
        all.extend(extra.iter().cloned());
        all
    })
}

/// The metric named `name` in any case, or by an alias (L2_SQUARED).
pub fn metric_named(metrics: &Metrics, name: &str) -> Option<Arc<dyn VectorMetric>> {
    let name = name.trim();
    let wanted = if name.eq_ignore_ascii_case(L2_SQUARED) { EUCLIDEAN_SQUARED } else { name };
    metrics().into_iter().find(|m| m.name().eq_ignore_ascii_case(wanted))
}

/// A user-defined distance function's body (design document §8): given
/// two vectors as `VectorMetric::distance` is.
pub type DistanceFn = Arc<dyn Fn(VectorRef<'_>, VectorRef<'_>) -> Result<Option<f64>> + Send + Sync>;

/// A metric whose distance is a callback, as CREATE VECTOR DISTANCE
/// FUNCTION makes one, with the properties it declares.
pub struct CallbackMetric {
    name: String,
    formats: Vec<Element>,
    properties: MetricProperties,
    call: DistanceFn,
}

impl CallbackMetric {
    /// A metric named `name` measuring `formats` by `call`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `name` is empty or not upper case, or no format is
    /// given.
    pub fn new(name: &str, formats: Vec<Element>, properties: MetricProperties, call: DistanceFn) -> Result<CallbackMetric> {
        if name.is_empty() || name.to_ascii_uppercase() != name || formats.is_empty() {
            return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] a distance function needs an upper-case name and a format"));
        }
        Ok(CallbackMetric { name: name.to_owned(), formats, properties, call })
    }
}

impl VectorMetric for CallbackMetric {
    fn name(&self) -> &str {
        &self.name
    }

    fn formats(&self) -> &[Element] {
        &self.formats
    }

    fn properties(&self) -> MetricProperties {
        self.properties
    }

    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
        (self.call)(a, b)
    }
}

/// The distance from `a` to `b` by `metric`, its operands checked first
/// and its result after.
///
/// # Errors
///
/// SQLCODE -301 for operands of other dimension counts or formats
/// (VEC-0003, VEC-0004) or a format the metric does not measure
/// (VEC-0008); SQLSTATE 22003 for an overflow (VEC-0012); SQLCODE -910
/// for a negative distance from a metric declared never negative.
pub fn distance(metric: &dyn VectorMetric, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
    measurable(metric, a, b)?;
    let Some(d) = metric.distance(a, b)? else { return Ok(None) };
    kept(metric, d).map(Some)
}

/// \[EXTENSION\] The distances from `query` to each of `candidates` by
/// `metric`, in one call of it (`VectorMetric::distances`), each checked as
/// `distance` checks one.
///
/// # Errors
///
/// As `distance`; SQLCODE -910 if the metric answers another count.
pub fn distances(metric: &dyn VectorMetric, query: VectorRef<'_>, candidates: &[VectorRef<'_>]) -> Result<Vec<Option<f64>>> {
    for c in candidates {
        measurable(metric, query, *c)?;
    }
    let got = metric.distances(query, candidates)?;
    if got.len() != candidates.len() {
        return Err(broken(metric.name(), "gave another count of distances than it was asked"));
    }
    got.into_iter().map(|d| d.map(|d| kept(metric, d)).transpose()).collect()
}

/// That `metric` measures `a` and `b`, of one shape.
fn measurable(metric: &dyn VectorMetric, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<()> {
    if a.dims != b.dims {
        return Err(mismatch(&format!("dimension mismatch: {} and {}", a.dims, b.dims)));
    }
    if a.element != b.element {
        return Err(mismatch(&format!("format mismatch: {} and {}", a.element.name(), b.element.name())));
    }
    if !takes_layout(metric, a.layout()) || !takes_layout(metric, b.layout()) {
        return Err(mismatch(&format!("the metric {} measures dense vectors only", metric.name())));
    }
    if !metric.formats().contains(&a.element) {
        return Err(mismatch(&format!("the metric {} does not measure {} vectors", metric.name(), a.element.name())));
    }
    Ok(())
}

/// `d`, a distance `metric` gave, if it keeps the metric's contract.
fn kept(metric: &dyn VectorMetric, d: f64) -> Result<f64> {
    if !d.is_finite() {
        return Err(out_of_range(&format!("the {} distance overflowed", metric.name())));
    }
    if d < 0.0_f64 && metric.properties().non_negative {
        return Err(broken(metric.name(), "gave a negative distance"));
    }
    Ok(d)
}

fn broken(name: &str, what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] the metric {name} broke its contract: it {what}"))
}

/// How far two distances that should be equal may differ, relatively, for
/// rounding.
const SPOT_TOLERANCE: f64 = 1e-9;

/// Checks the properties `metric` declares on `sample`, each pair and
/// each triple of its first vectors (R8.3, VEC-0021).
///
/// # Errors
///
/// SQLCODE -910 if a declared property fails; as `distance`.
pub fn spot_check(metric: &dyn VectorMetric, sample: &[VectorRef<'_>]) -> Result<()> {
    let p = metric.properties();
    let near = |x: f64, y: f64| (x - y).abs() <= SPOT_TOLERANCE * x.abs().max(y.abs()).max(1.0);
    for (i, a) in sample.iter().enumerate() {
        for b in sample.iter().skip(i) {
            let (ab, ba) = (distance(metric, *a, *b)?, distance(metric, *b, *a)?);
            let agree = match (ab, ba) {
                (Some(x), Some(y)) => near(x, y),
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            };
            if p.symmetric && !agree {
                return Err(broken(metric.name(), "was declared SYMMETRIC but is not"));
            }
            for c in sample {
                let (Some(x), Some(y), Some(z)) = (ab, distance(metric, *b, *c)?, distance(metric, *a, *c)?) else { continue };
                if p.triangle && z > x + y && !near(z, x + y) {
                    return Err(broken(metric.name(), "was declared TRIANGLE_INEQUALITY but breaks it"));
                }
            }
        }
    }
    Ok(())
}

/// Whether a metric measures a layout. User-defined metrics are given
/// dense vectors only, as their bodies index elements.
fn takes_layout(metric: &dyn VectorMetric, layout: Layout) -> bool {
    matches!(layout, Layout::Dense) || super::builtin::is_built_in(metric.name())
}
