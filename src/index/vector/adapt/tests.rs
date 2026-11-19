//! Tests of the database's metrics and quantizers as the engines compute
//! with them.

use std::sync::Arc;

use super::*;
use crate::error::Code;
use crate::vector::metric::{CallbackMetric, MetricProperties, built_in_metrics, metric_named};
use crate::vector::quantizer::{PQ, SQ8, built_in_quantizers, quantizer_named};

/// The dimensions of the vectors quantized.
const DIMS: usize = 8;

/// The built-in metric named `name`.
fn built_in(name: &str) -> Arc<dyn VectorMetric> {
    metric_named(&built_in_metrics(), name).unwrap()
}

/// What a user-defined metric of the tests answers of a sum.
type Answer = fn(f64) -> Result<Option<f64>>;

/// A FLOAT32 column of `dims` dimensions.
fn float32(dims: u32) -> Column {
    Column { dims, element: Element::Float32 }
}

/// A user-defined metric whose distance is the sum of the first vector's
/// bytes, after `answer` is consulted for a null or a failure.
fn summing(answer: Answer) -> Arc<dyn VectorMetric> {
    let properties = MetricProperties { symmetric: false, non_negative: true, triangle: false, normalized_as: None };
    let call: crate::vector::metric::DistanceFn =
        Arc::new(move |a: VectorRef<'_>, _: VectorRef<'_>| answer(a.bytes.iter().map(|b| f64::from(*b)).sum()));
    Arc::new(CallbackMetric::new("SUMMED", vec![Element::Float32], properties, call).unwrap())
}

/// A sample of `count` vectors of `DIMS`, one after another.
fn sample(count: usize) -> Vec<f32> {
    (0..count.checked_mul(DIMS).unwrap()).map(|i| f32::from(u16::try_from(i.wrapping_mul(7).wrapping_rem(23)).unwrap())).collect()
}

/// Verifies: [EXTENSION] vector_standard §6.1 R6.1: HAMMING counts the
/// elements that differ; JACCARD of BINARY vectors is one less the share
/// of set bits set in both, 0 of vectors with none set; COSINE of a zero
/// vector is the middle of its range; its normed form is bit for bit the
/// plain one, and a metric not measured by norms is measured plainly.
#[test]
fn built_in_distances_as_the_engines_measure() {
    let column = Column { dims: 4, element: Element::Binary };
    let hamming = distance(&built_in(HAMMING), column);
    assert!((hamming(&[1.0, 0.0, 1.0, 0.0], &[1.0, 1.0, 0.0, 0.0]) - 2.0).abs() < f32::EPSILON);
    let jaccard = distance(&built_in(JACCARD), column);
    assert!((jaccard(&[1.0, 1.0, 0.0, 0.0], &[1.0, 0.0, 1.0, 0.0]) - 2.0 / 3.0).abs() < 1e-6);
    assert!(jaccard(&[0.0; 4], &[0.0; 4]).abs() < f32::EPSILON);
    let cosine_metric = built_in(COSINE);
    let cosine = distance(&cosine_metric, float32(2));
    assert!((cosine(&[0.0, 0.0], &[1.0, 2.0]) - 1.0).abs() < f32::EPSILON);
    let by_norms = normed(&cosine_metric).unwrap();
    let (a, b) = ([1.0_f32, 2.0], [3.0_f32, -1.0]);
    assert_eq!(by_norms((&a, 5.0), (&b, 10.0)).to_bits(), cosine(&a, &b).to_bits());
    assert!((by_norms((&a, 0.0), (&b, 10.0)) - 1.0).abs() < f32::EPSILON);
    assert!(normed(&built_in(EUCLIDEAN)).is_none());
    let disk = disk_metric(&built_in(MANHATTAN), float32(2));
    assert!((disk.normed((&a, 5.0), (&b, 10.0)) - 5.0).abs() < f32::EPSILON);
    assert!((disk.distance(&a, &b) - 5.0).abs() < f32::EPSILON);
}

/// Verifies: [EXTENSION] vector_standard §8: a user-defined metric is
/// given the vectors packed in the column's own format, FLOAT64 as
/// FLOAT32; a null, a failure or a distance not finite is taken as
/// farthest.
#[test]
fn user_metrics_see_the_columns_format() {
    let metric = summing(|sum| Ok(Some(sum)));
    let measured = |element: Element, v: &[f32]| {
        let dims = u32::try_from(v.len()).unwrap();
        distance(&metric, Column { dims, element })(v, v)
    };
    assert!((measured(Element::Float32, &[1.0]) - 191.0).abs() < f32::EPSILON);
    assert!((measured(Element::Float64, &[1.0]) - 191.0).abs() < f32::EPSILON);
    assert!((measured(Element::Float16, &[1.0]) - 60.0).abs() < f32::EPSILON);
    assert!((measured(Element::Int8, &[-1.0, 2.0]) - 257.0).abs() < f32::EPSILON);
    let bits = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0];
    assert!((measured(Element::Binary, &bits) - 257.0).abs() < f32::EPSILON);
    let answers: [Answer; 3] = [|_| Ok(None), |_| Err(SqlError::new(Code::Data, "broken")), |_| Ok(Some(f64::INFINITY))];
    for answer in answers {
        assert_eq!(distance(&summing(answer), float32(1))(&[1.0], &[1.0]).to_bits(), FARTHEST.to_bits());
    }
}

/// Verifies: [EXTENSION] vector_standard §7.2: a database quantizer as
/// DiskANN's trains codes on a sample, encodes a vector to codes of its
/// length, measures codes from a query, and reads back the codes it
/// writes.
#[test]
fn disk_quantizer_trains_measures_and_reads_back() {
    let pq = quantizer_named(&built_in_quantizers(), PQ).unwrap();
    let quantizer = disk_quantizer(pq, built_in(EUCLIDEAN_SQUARED), 0);
    let vectors = sample(64);
    let codes = quantizer.train(&vectors, DIMS, 7).unwrap();
    let mut code = vec![0_u8; codes.code_bytes()];
    assert_ne!(code.len(), 0);
    codes.encode(&vectors[..DIMS], &mut code);
    let table = codes.table(&vectors[..DIMS]);
    let far: Vec<f32> = vectors[..DIMS].iter().map(|x| x + 100.0).collect();
    let mut far_code = vec![0_u8; codes.code_bytes()];
    codes.encode(&far, &mut far_code);
    assert!(table.distance(&code) < table.distance(&far_code));
    let again = quantizer.read(&codes.write()).unwrap();
    assert_eq!(again.code_bytes(), codes.code_bytes());
    assert!(quantizer.read(&[0xFF]).is_err());
}

/// Verifies: [EXTENSION] vector_standard §7.2: HNSW and IVF keep SQ8
/// codes of their own; another database quantizer is theirs by its
/// codebooks, encoding and decoding near the vector, and a codebook read
/// back of other dimensions is refused as corrupt, SQLCODE -916.
#[test]
fn codec_quantizer_keeps_codebooks() {
    let quantizers = built_in_quantizers();
    assert_eq!(codec_quantizer(quantizer_named(&quantizers, SQ8).unwrap(), 0).name(), hnsw::SQ8);
    let quantizer = codec_quantizer(quantizer_named(&quantizers, PQ).unwrap(), 2);
    assert_eq!(quantizer.name(), PQ);
    let vectors = sample(64);
    let codec = quantizer.train(&vectors, DIMS, 7).unwrap();
    assert_eq!(codec.code_bytes(), 2);
    let mut code = vec![0_u8; 2];
    codec.encode(&vectors[..DIMS], &mut code);
    let mut back = vec![f32::NAN; DIMS];
    codec.decode(&code, &mut back);
    assert!(back.iter().all(|x| x.is_finite()));
    let mut bytes = Vec::new();
    codec.write(&mut bytes);
    assert_eq!(quantizer.read(&bytes, DIMS).unwrap().code_bytes(), 2);
    let Err(e) = quantizer.read(&bytes, DIMS + 1) else { panic!("read") };
    assert_eq!(e.sqlcode(), -916_i32);
}
