//! \[EXTENSION\] The options every vector index method declares alike,
//! each under its own group (RULES.md D41; the vector design document
//! §10.2, §10.4, R10.2, R13.1): TARGET_ACCURACY, the share of the true
//! nearest rows a search aims at; PQ_BYTES, the bytes of a quantizer's
//! code; and NORMALIZED, which declares every vector of the column of
//! norm 1.
//!
//! An index's TARGET_ACCURACY maps to its search list, or IVF's probes,
//! as a query's does (`ask.rs`), when the query sets neither; but an index
//! whose own list or probes option is set searches by that, which is
//! what setting it asks.
//!
//! NORMALIZED ON holds the column to vectors of norm 1, to the tolerance
//! the image's normalized flag has: the index refuses another as it is
//! written (SQLCODE -301). On such vectors COSINE, DOT, EUCLIDEAN and
//! EUCLIDEAN_SQUARED order every row alike for one query, as
//! `|a − q|² = 1 + |q|² − 2 a·q`. So an index of one serves a call of any
//! of them, as R13.1 has it ("COSINE may use a DOT index when the column
//! is flagged normalized"). A graph measures COSINE and DOT by EUCLIDEAN,
//! which is SYMMETRIC and NON_NEGATIVE as a graph needs (R8.2).

use crate::catalog::IndexDesc;
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};
use crate::vector::metric::{COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, L2_SQUARED};

/// The suffix of each group's target accuracy.
pub(crate) const TARGET_ACCURACY: &str = "TARGET_ACCURACY";
/// The suffix of each group's code bytes.
pub(crate) const PQ_BYTES: &str = "PQ_BYTES";
/// The suffix of each group's declaration of normalized vectors.
pub(crate) const NORMALIZED: &str = "NORMALIZED";
/// A switch on.
pub(crate) const ON: &str = "ON";
/// A switch off.
pub(crate) const OFF: &str = "OFF";
/// The switches' values.
pub(crate) const SWITCH: &[&str] = &[ON, OFF];
/// \[DESIGN\] The default target accuracy, a percentage (R10.2).
pub(crate) const DEFAULT_ACCURACY: u8 = 90;
/// A percentage's whole.
const PERCENT: i128 = 100;
/// The most code bytes.
const MOST_CODE: i128 = 65_535;

/// The option `suffix` of the group `group`, as `HNSW.TARGET_ACCURACY`.
pub(crate) fn named(group: &str, suffix: &str) -> String {
    format!("{group}.{suffix}")
}

/// The options of group `group` every vector method declares.
pub(crate) fn shared(group: &str) -> Vec<TableOption> {
    vec![
        TableOption::new(
            (&named(group, TARGET_ACCURACY), Kind::Integer { min: 1, max: PERCENT }, whole(DEFAULT_ACCURACY)),
            Rebuild::No,
            "The share of the true nearest rows an approximate search aims to find, a percentage, unless the query sets its own or the index's own search list or probes are set.",
        ),
        TableOption::new(
            (&named(group, PQ_BYTES), Kind::Integer { min: 0, max: MOST_CODE }, whole(0_u8)),
            Rebuild::Yes,
            "Bytes of a quantizer's code of a vector; 0 for the quantizer's default.",
        ),
        TableOption::new(
            (&named(group, NORMALIZED), Kind::Choice(SWITCH), Value::Char(OFF.into())),
            Rebuild::Yes,
            "ON declares every vector of the column of norm 1, refusing another, so a search of COSINE, DOT, EUCLIDEAN or EUCLIDEAN_SQUARED may use the index whichever its metric.",
        ),
    ]
}

fn whole(n: impl Into<i128>) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// What `desc` sets of the shared options of `group`, the others their
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shared {
    /// The accuracy a search of no list or accuracy of its own aims at:
    /// none if the index's own `list` option is set, which it then
    /// searches by.
    pub(crate) accuracy: Option<u8>,
    /// Code bytes; 0 for the quantizer's default.
    pub(crate) code_bytes: usize,
    /// Whether the column is declared normalized.
    pub(crate) normalized: bool,
}

/// The shared options `desc` sets of `group`, of a method whose own
/// search list or probes option is `list`.
pub(crate) fn shared_of(desc: &IndexDesc, group: &str, list: &str) -> Shared {
    let declared = shared(group);
    let value = |suffix: &str| option_value(&declared, &desc.options, &named(group, suffix));
    let count = |suffix: &str| match value(suffix) {
        Some(Value::Exact(d)) => d.units,
        Some(_) | None => 0,
    };
    let own_list = desc.options.iter().any(|(name, _)| name.eq_ignore_ascii_case(list));
    let accuracy = u8::try_from(count(TARGET_ACCURACY)).ok().filter(|a| (1..=100).contains(a) && !own_list);
    let normalized = matches!(value(NORMALIZED), Some(Value::Char(w)) if w.trim().eq_ignore_ascii_case(ON));
    Shared { accuracy, code_bytes: usize::try_from(count(PQ_BYTES)).unwrap_or(0), normalized }
}

/// The metric `metric` orders as on vectors of norm 1, of the four that
/// order alike there; else itself.
fn on_unit(metric: &str) -> &str {
    let alike = [COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, L2_SQUARED].iter().any(|m| metric.eq_ignore_ascii_case(m));
    if alike { EUCLIDEAN } else { metric }
}

/// \[EXTENSION\] Whether an index of metric `index`, of a column declared
/// `normalized` or not, serves a call of metric `call` (R13.1): its own
/// metric, EUCLIDEAN and EUCLIDEAN_SQUARED each other; and, normalized,
/// any of COSINE, DOT, EUCLIDEAN and EUCLIDEAN_SQUARED any other.
pub(crate) fn serves(index: &str, normalized: bool, call: &str) -> bool {
    crate::index::same_metric(index, call) || (normalized && on_unit(index).eq_ignore_ascii_case(on_unit(call)))
}

/// The radius, in EUCLIDEAN's terms, of a call of metric `call` and
/// radius `radius` of a query of norm `norm` over vectors of norm 1: from
/// `|a − q|² = 1 + |q|² − 2 a·q`, DOT `−a·q` and COSINE `1 − a·q / |q|`.
/// None of a metric not of the four.
pub(crate) fn unit_radius(call: &str, radius: f64, norm: f64) -> Option<f64> {
    let squared = if call.eq_ignore_ascii_case(EUCLIDEAN) {
        radius.max(0.0) * radius.max(0.0)
    } else if call.eq_ignore_ascii_case(EUCLIDEAN_SQUARED) || call.eq_ignore_ascii_case(L2_SQUARED) {
        radius
    } else if call.eq_ignore_ascii_case(DOT) {
        1.0_f64 + norm * norm + 2.0_f64 * radius
    } else if call.eq_ignore_ascii_case(COSINE) {
        1.0_f64 + norm * norm - 2.0_f64 * norm * (1.0_f64 - radius)
    } else {
        return None;
    };
    Some(squared.max(0.0).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    fn desc(options: Vec<(String, Value)>) -> IndexDesc {
        IndexDesc {
            name: "V".into(),
            table: QualifiedName::of_schema("S"),
            columns: vec![0],
            method: "hnsw".into(),
            unique: false,
            options,
            collations: Vec::new(),
            nulls: false,
            expressions: Vec::new(),
        }
    }

    /// Verifies: [EXTENSION] vector_standard R10.2, §10.2: TARGET_ACCURACY
    /// defaults to 90 and is read as set, but is none once the index's own
    /// list option is set; PQ_BYTES and NORMALIZED are read as set.
    #[test]
    fn shared_options_default_and_yield_to_an_own_list() {
        let read = shared_of(&desc(Vec::new()), "HNSW", "HNSW.EF_SEARCH");
        assert_eq!(read, Shared { accuracy: Some(DEFAULT_ACCURACY), code_bytes: 0, normalized: false });
        let set = vec![
            ("HNSW.TARGET_ACCURACY".into(), whole(99_u8)),
            ("HNSW.PQ_BYTES".into(), whole(8_u8)),
            ("HNSW.NORMALIZED".into(), Value::Char("on".into())),
        ];
        assert_eq!(shared_of(&desc(set.clone()), "HNSW", "HNSW.EF_SEARCH"), Shared { accuracy: Some(99), code_bytes: 8, normalized: true });
        let mut own = set;
        own.push(("HNSW.EF_SEARCH".into(), whole(40_u8)));
        assert_eq!(shared_of(&desc(own), "HNSW", "HNSW.EF_SEARCH").accuracy, None);
        assert!(shared("IVF").iter().all(|o| o.name.starts_with("IVF.")));
    }

    /// Verifies: [EXTENSION] vector_standard R13.1: an index serves its own
    /// metric, EUCLIDEAN and EUCLIDEAN_SQUARED each other; of a normalized
    /// column, COSINE a DOT index and the four each other, never MANHATTAN.
    #[test]
    fn a_normalized_index_serves_the_metrics_alike_on_unit_vectors_r13_1() {
        assert!(serves(DOT, false, DOT) && serves(EUCLIDEAN, false, EUCLIDEAN_SQUARED));
        assert!(!serves(DOT, false, COSINE));
        assert!(serves(DOT, true, COSINE) && serves(COSINE, true, EUCLIDEAN) && serves(EUCLIDEAN_SQUARED, true, DOT));
        assert!(!serves(DOT, true, "MANHATTAN") && !serves("MANHATTAN", true, COSINE));
    }

    /// Verifies: [EXTENSION] vector_standard §9.4 R9.1, R13.1: a radius of
    /// each of the four metrics over unit vectors is the EUCLIDEAN radius
    /// of the same rows: of a = (1, 0) and q = (2, 0), |a − q| = 1, DOT
    /// −2, COSINE 0.
    #[test]
    fn a_radius_is_carried_to_euclidean_on_unit_vectors() {
        let near = |x: Option<f64>, y: f64| x.is_some_and(|x| (x - y).abs() < 1e-12_f64);
        assert!(near(unit_radius(EUCLIDEAN, 1.0_f64, 2.0_f64), 1.0_f64));
        assert!(near(unit_radius(EUCLIDEAN_SQUARED, 1.0_f64, 2.0_f64), 1.0_f64));
        assert!(near(unit_radius(DOT, -2.0_f64, 2.0_f64), 1.0_f64));
        assert!(near(unit_radius(COSINE, 0.0_f64, 2.0_f64), 1.0_f64));
        assert!(near(unit_radius(DOT, -9.0_f64, 2.0_f64), 0.0_f64));
        assert_eq!(unit_radius("MANHATTAN", 1.0_f64, 2.0_f64), None);
    }
}
