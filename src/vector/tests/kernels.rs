//! Unit tests of the distance kernels and metrics, against references in
//! `f64` over the stored elements.

use super::*;
use crate::vector::metric::{
    COSINE, CallbackMetric, DOT, DistanceFn, EUCLIDEAN, EUCLIDEAN_SQUARED, HAMMING, JACCARD, MANHATTAN, MetricProperties, Metrics,
    VectorMetric, built_in_metrics, distance, metric_named, metrics_with, spot_check,
};
use std::sync::Arc;

/// The dimension counts tried: below, at and past a chunk of sixteen
/// lanes, and long ones with tails.
const DIMS: [usize; 7] = [1, 15, 16, 17, 64, 100, 333];
/// The reference's tolerance: a millionth of the sum of the terms'
/// magnitudes, the error an `f32` sum of them may have.
const TOLERANCE: f64 = 1e-6;

/// The metrics' references over the stored values of `a` and `b`: the
/// distance, and the sum of magnitudes it was built from.
fn reference(name: &str, a: &[f64], b: &[f64]) -> (Option<f64>, f64) {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let size: f64 = a.iter().zip(b).map(|(x, y)| (x * y).abs() + (x - y).powi(2) + (x - y).abs()).sum();
    let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let square: f64 = a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum();
    let d = match name {
        COSINE => (norm(a) * norm(b) > 0.0_f64).then(|| (1.0 - dot / (norm(a) * norm(b))).clamp(0.0, 2.0)),
        DOT => Some(-dot),
        EUCLIDEAN => Some(square.sqrt()),
        EUCLIDEAN_SQUARED => Some(square),
        MANHATTAN => Some(a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()),
        HAMMING => Some(f64::from(u32::try_from(a.iter().zip(b).filter(|(x, y)| x.to_bits() != y.to_bits()).count()).unwrap())),
        _ => None,
    };
    (d, size.max(1.0))
}

fn stored(v: &Value) -> Vec<f64> {
    Vector::of(vector_of(v).unwrap()).values()
}

fn check(metrics: &Metrics, name: &str, a: &Value, b: &Value) {
    let metric = metric_named(metrics, name).unwrap();
    let ours = distance(metric.as_ref(), vector_of(a).unwrap(), vector_of(b).unwrap()).unwrap();
    let (theirs, size) = reference(name, &stored(a), &stored(b));
    match (ours, theirs) {
        (Some(x), Some(y)) => {
            // A cosine divides by norms the image holds as f32s.
            let scale = if name == COSINE { 1.0_f64 } else { size };
            assert!((x - y).abs() <= TOLERANCE * scale, "{name}: {x} vs {y}");
        }
        (None, None) => {}
        _ => panic!("{name}: {ours:?} vs {theirs:?}"),
    }
}

/// Verifies: [EXTENSION] §6.2's COSINE, DOT (negated, R6.8), EUCLIDEAN,
/// EUCLIDEAN_SQUARED, MANHATTAN and HAMMING over FLOAT32, FLOAT16,
/// FLOAT64 and INT8 (R3.10 accumulators), for lengths around the
/// kernels' chunks, equal to a reference in `f64` to a millionth.
#[test]
fn metrics_match_f64_references() {
    let metrics = built_in_metrics();
    let mut random = Random::new(42);
    for element in [Element::Float32, Element::Float16, Element::Float64, Element::Int8] {
        for dims in DIMS {
            let mut values = || {
                let raw = noise(&mut random, dims);
                if element == Element::Int8 { raw.iter().map(|x| (x * 127.0).round()).collect() } else { raw }
            };
            let (a, b) = (dense(element, &values()), dense(element, &values()));
            for name in [COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, MANHATTAN, HAMMING] {
                check(&metrics, name, &a, &b);
                check(&metrics, name, &a, &a);
            }
        }
    }
}

/// Verifies: [EXTENSION] BINARY HAMMING counts differing bits and JACCARD
/// is `1 - |a ∧ b| / |a ∨ b|`, over 64-bit words and tails; two empty
/// sets are at distance 0.
#[test]
fn bit_metrics_count_bits() {
    let metrics = built_in_metrics();
    let mut random = Random::new(9);
    for dims in [8_usize, 64, 72, 1024] {
        let mut bits = || (0..dims).map(|_| f64::from(u8::from(random.next() & 1 == 1))).collect::<Vec<_>>();
        let (x, y) = (bits(), bits());
        let (a, b) = (dense(Element::Binary, &x), dense(Element::Binary, &y));
        let count = |f: &dyn Fn(f64, f64) -> bool| f64::from(u32::try_from(x.iter().zip(&y).filter(|(p, q)| f(**p, **q)).count()).unwrap());
        let ham = distance(metric_named(&metrics, HAMMING).unwrap().as_ref(), vector_of(&a).unwrap(), vector_of(&b).unwrap()).unwrap();
        assert!((ham.unwrap() - count(&|p, q| p.to_bits() != q.to_bits())).abs() < 0.5_f64);
        let jac = distance(metric_named(&metrics, JACCARD).unwrap().as_ref(), vector_of(&a).unwrap(), vector_of(&b).unwrap()).unwrap();
        let expected = 1.0_f64 - count(&|p, q| p > 0.5_f64 && q > 0.5_f64) / count(&|p, q| p > 0.5_f64 || q > 0.5_f64);
        assert!((jac.unwrap() - expected).abs() < 1e-12_f64);
    }
    let zero = dense(Element::Binary, &[0.0_f64; 8]);
    let jaccard = metric_named(&metrics, JACCARD).unwrap();
    assert_eq!(distance(jaccard.as_ref(), vector_of(&zero).unwrap(), vector_of(&zero).unwrap()).unwrap(), Some(0.0_f64));
}

/// Verifies: [EXTENSION] R4.2, R6.4: a sparse vector beside a sparse or a
/// dense one measures as the dense equivalents do, by merging indexes.
#[test]
fn sparse_distances_merge() {
    let metrics = built_in_metrics();
    let a = sparse(Element::Float32, 40, &[(1, 0.5_f64), (7, -2.0_f64), (39, 1.0_f64)]);
    let b = sparse(Element::Float32, 40, &[(0, 3.0_f64), (7, 1.0_f64), (20, 0.25_f64)]);
    let dense_of = |v: &Value| dense(Element::Float32, &stored(v));
    for name in [COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, MANHATTAN, HAMMING] {
        let metric = metric_named(&metrics, name).unwrap();
        let both = distance(metric.as_ref(), vector_of(&a).unwrap(), vector_of(&b).unwrap()).unwrap().unwrap();
        let (da, db) = (dense_of(&a), dense_of(&b));
        let mixed = distance(metric.as_ref(), vector_of(&a).unwrap(), vector_of(&db).unwrap()).unwrap().unwrap();
        let plain = distance(metric.as_ref(), vector_of(&da).unwrap(), vector_of(&db).unwrap()).unwrap().unwrap();
        assert!((both - plain).abs() < 1e-6_f64 && (mixed - plain).abs() < 1e-6_f64, "{name}: {both} {mixed} {plain}");
    }
}

/// Verifies: [EXTENSION] R6.7 COSINE of a zero vector is NULL; VEC-0003,
/// VEC-0004 and VEC-0008: operands of other dimension counts or formats,
/// or a format the metric does not measure, are SQLCODE -301; names are
/// found in any case and L2_SQUARED is EUCLIDEAN_SQUARED's alias.
#[test]
fn metric_operands_are_checked() {
    let metrics = built_in_metrics();
    let cosine = metric_named(&metrics, "cosine").unwrap();
    let zero = dense(Element::Float32, &[0.0_f64, 0.0_f64]);
    let one = dense(Element::Float32, &[1.0_f64, 0.0_f64]);
    assert_eq!(distance(cosine.as_ref(), vector_of(&zero).unwrap(), vector_of(&one).unwrap()).unwrap(), None);
    assert_eq!(metric_named(&metrics, "l2_squared").unwrap().name(), EUCLIDEAN_SQUARED);
    assert!(metric_named(&metrics, "COSINE_X").is_none());
    let three = dense(Element::Float32, &[1.0_f64, 0.0_f64, 0.0_f64]);
    let wide = dense(Element::Float64, &[1.0_f64, 0.0_f64]);
    let bits = dense(Element::Binary, &[1.0_f64; 8]);
    for (a, b) in [(&one, &three), (&one, &wide), (&bits, &bits)] {
        let err = distance(cosine.as_ref(), vector_of(a).unwrap(), vector_of(b).unwrap()).unwrap_err();
        assert_eq!((err.sqlcode(), err.sqlstate().as_str()), (-301_i32, "22000"));
    }
    let jaccard = metric_named(&metrics, JACCARD).unwrap();
    assert!(distance(jaccard.as_ref(), vector_of(&one).unwrap(), vector_of(&one).unwrap()).is_err());
    let huge = dense(Element::Float64, &[1e300_f64, -1e300_f64]);
    let err = distance(
        metric_named(&metrics, EUCLIDEAN_SQUARED).unwrap().as_ref(),
        vector_of(&huge).unwrap(),
        vector_of(&dense(Element::Float64, &[-1e300_f64, 1e300_f64])).unwrap(),
    );
    assert_eq!(err.unwrap_err().sqlstate().as_str(), "22003");
    let properties = metric_named(&metrics, DOT).unwrap().properties();
    assert!(properties.symmetric && !properties.non_negative && properties.normalized_as == Some(EUCLIDEAN));
}

fn callback(name: &str, properties: MetricProperties, call: DistanceFn) -> Arc<dyn VectorMetric> {
    Arc::new(CallbackMetric::new(name, vec![Element::Float32], properties, call).unwrap())
}

/// Verifies: [EXTENSION] §8: a user-defined metric backed by a callback
/// is found beside the built-in ones; its negative distance under
/// NON_NEGATIVE breaks the contract (SQLCODE -910); R8.3's spot check
/// catches a false SYMMETRIC or TRIANGLE_INEQUALITY; sparse vectors are
/// not handed to it.
#[test]
fn callback_metrics_are_checked() {
    let declared = MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None };
    let first: DistanceFn = Arc::new(|a, b| Ok(Some(a.at(0).unwrap_or_default() - b.at(0).unwrap_or_default())));
    let metrics = metrics_with(vec![callback("APP.FIRST", declared, first)]);
    let found = metric_named(&metrics, "app.first").unwrap();
    let (one, two) = (dense(Element::Float32, &[1.0_f64]), dense(Element::Float32, &[2.0_f64]));
    assert_eq!(distance(found.as_ref(), vector_of(&two).unwrap(), vector_of(&one).unwrap()).unwrap(), Some(1.0_f64));
    assert_eq!(distance(found.as_ref(), vector_of(&one).unwrap(), vector_of(&two).unwrap()).unwrap_err().sqlcode(), -910_i32);
    let lax = MetricProperties { non_negative: false, ..declared };
    let asymmetric = callback("ASYM", lax, Arc::new(|a, b| Ok(Some(a.at(0).unwrap_or_default() - b.at(0).unwrap_or_default()))));
    let sample = [vector_of(&one).unwrap(), vector_of(&two).unwrap()];
    assert_eq!(spot_check(asymmetric.as_ref(), &sample).unwrap_err().sqlcode(), -910_i32);
    let squared = callback(
        "SQ",
        MetricProperties { symmetric: true, ..declared },
        Arc::new(|a, b| Ok(Some((a.at(0).unwrap_or_default() - b.at(0).unwrap_or_default()).powi(2)))),
    );
    let three = dense(Element::Float32, &[3.0_f64]);
    let line = [vector_of(&one).unwrap(), vector_of(&two).unwrap(), vector_of(&three).unwrap()];
    assert_eq!(spot_check(squared.as_ref(), &line).unwrap_err().sqlcode(), -910_i32);
    let euclid = metric_named(&built_in_metrics(), EUCLIDEAN).unwrap();
    assert!(spot_check(euclid.as_ref(), &line).is_ok());
    let s = sparse(Element::Float32, 1, &[(0, 1.0_f64)]);
    assert!(distance(found.as_ref(), vector_of(&s).unwrap(), vector_of(&one).unwrap()).is_err());
    assert!(CallbackMetric::new("lower", vec![Element::Float32], declared, Arc::new(|_, _| Ok(None))).is_err());
}

/// Verifies: [EXTENSION] the box bounds EUCLIDEAN, EUCLIDEAN_SQUARED and
/// MANHATTAN give are below the distance to any vector in the box.
#[test]
fn lower_bounds_stay_below() {
    let metrics = built_in_metrics();
    let q = dense(Element::Float32, &[0.0_f64, 5.0_f64]);
    let inside = dense(Element::Float32, &[2.0_f64, 1.0_f64]);
    for name in [EUCLIDEAN, EUCLIDEAN_SQUARED, MANHATTAN] {
        let m = metric_named(&metrics, name).unwrap();
        let bound = m.lower_bound(vector_of(&q).unwrap(), &[1.0, 0.0], &[3.0, 2.0]).unwrap();
        let d = m.distance(vector_of(&q).unwrap(), vector_of(&inside).unwrap()).unwrap().unwrap();
        assert!(bound > 0.0_f64 && bound <= d, "{name}");
    }
    assert!(metric_named(&metrics, COSINE).unwrap().lower_bound(vector_of(&q).unwrap(), &[0.0; 2], &[1.0; 2]).is_none());
}
