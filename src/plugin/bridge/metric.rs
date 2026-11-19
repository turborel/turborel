//! \[EXTENSION\] A plug-in's vector metric (RULES.md D169), a
//! `VectorMetric` whose distances from one query to many candidates are
//! one CALL (`distances`), so VECTOR_DISTANCE over a batch of rows and a
//! graph index's expansion of a node each make one round trip. A metric's
//! failure in an index is taken as farthest, as any user-defined metric's
//! is (`index/vector/adapt.rs`).

use crate::bytes::Put;
use crate::error::Result;
use crate::plugin::manifest::{Declared, NON_NEGATIVE, SYMMETRIC, TRIANGLE};
use crate::plugin::payload::METRIC_DISTANCES;
use crate::plugin::values::{DENSE_VECTOR, SPARSE_VECTOR, values};
use crate::value::Value;
use crate::vector::metric::{MetricProperties, VectorMetric};
use crate::vector::{Element, Layout, VectorRef};

use super::{Link, answer, counted};

/// The candidates a CALL carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 4096;

/// A plug-in's metric.
#[derive(Debug)]
pub struct PluginMetric {
    link: Link,
    formats: Vec<Element>,
    properties: MetricProperties,
    batch: usize,
}

impl PluginMetric {
    /// The metric `d` declares.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 of a format no element's.
    pub fn new(d: &Declared, link: Link) -> Result<PluginMetric> {
        let properties = MetricProperties {
            symmetric: d.has_property(SYMMETRIC),
            non_negative: d.has_property(NON_NEGATIVE),
            triangle: d.has_property(TRIANGLE),
            normalized_as: None,
        };
        let batch = d.batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH);
        Ok(PluginMetric { link, formats: d.metric_formats()?, properties, batch: batch.max(1) })
    }
}

/// `v`, as protocol 1 carries a vector: dense or sparse, its values as
/// DOUBLE.
fn put_vector(out: &mut Vec<u8>, v: VectorRef<'_>) {
    match v.layout() {
        Layout::Dense => {
            out.put_u8(DENSE_VECTOR);
            out.put_u32(v.dims);
            (0..v.dims).for_each(|i| out.put_u64(v.at(i).unwrap_or(0.0_f64).to_bits()));
        }
        Layout::Sparse => {
            out.put_u8(SPARSE_VECTOR);
            out.put_u32(v.dims);
            let stored = v.stored();
            out.put_u32(stored);
            for i in 0..stored {
                out.put_u32(v.index(i).unwrap_or(0));
                out.put_u64(v.value(i).unwrap_or(0.0_f64).to_bits());
            }
        }
    }
}

impl VectorMetric for PluginMetric {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn formats(&self) -> &[Element] {
        &self.formats
    }
    fn properties(&self) -> MetricProperties {
        self.properties
    }
    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
        Ok(self.distances(a, &[b])?.pop().flatten())
    }
    fn distances(&self, query: VectorRef<'_>, candidates: &[VectorRef<'_>]) -> Result<Vec<Option<f64>>> {
        let mut all = Vec::with_capacity(candidates.len());
        for chunk in candidates.chunks(self.batch) {
            let mut payload = Vec::new();
            put_vector(&mut payload, query);
            payload.put_u32(u32::try_from(chunk.len()).unwrap_or(u32::MAX));
            for c in chunk {
                put_vector(&mut payload, *c);
            }
            let got = self.link.plain(METRIC_DISTANCES, payload)?;
            for v in counted(answer(&got, values)?, chunk.len())? {
                all.push(distance_of(&v)?);
            }
        }
        Ok(all)
    }
    fn batches(&self) -> bool {
        true
    }
}

/// A distance answered: a DOUBLE, or null.
fn distance_of(v: &Value) -> Result<Option<f64>> {
    if let Value::Approx(d) = v {
        return Ok(Some(*d));
    }
    if v.is_null() { Ok(None) } else { Err(crate::plugin::errors::violation(&format!("a distance is a DOUBLE or null, not {v:?}"))) }
}
