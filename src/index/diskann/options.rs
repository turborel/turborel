//! \[EXTENSION\] A DiskANN index's options (RULES.md D41), set by `CREATE
//! INDEX ... USING diskann WITH (...)` and changed by `ALTER INDEX ...
//! SET`. Those fixing a node's record or code (its degree, vector format,
//! quantizer, code bytes and metric) make the index anew; the rest change
//! in place (`DiskAnn::retune`). Defaults are the paper's [DiskANN,
//! NeurIPS 2019 §2.3, §3.1, §3.3, §3.4, §4.3] where it gives one.

use crate::catalog::IndexDesc;
use crate::engine::diskann::{Element, MOST_DEGREE, Tuning, defaults};
use crate::error::{Code, Result, SqlError};
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};

/// R, the most edges of a node.
pub const MAX_DEGREE: &str = "DISKANN.MAX_DEGREE";
/// L of builds and inserts.
pub const BUILD_LIST: &str = "DISKANN.BUILD_LIST";
/// α of RobustPrune.
pub const ALPHA: &str = "DISKANN.ALPHA";
/// Vamana passes, 2 for α = 1 then α.
pub const BUILD_PASSES: &str = "DISKANN.BUILD_PASSES";
/// L of searches.
pub const SEARCH_LIST: &str = "DISKANN.SEARCH_LIST";
/// W of BeamSearch.
pub const BEAM_WIDTH: &str = "DISKANN.BEAM_WIDTH";
/// The quantizer of the guiding codes, by name, or `NONE`.
pub const QUANTIZER: &str = "DISKANN.QUANTIZER";
/// Bytes of a code; 0 for a quarter of the dimensions.
pub const PQ_BYTES: &str = "DISKANN.PQ_BYTES";
/// The metric, by name.
pub const METRIC: &str = "DISKANN.METRIC";
/// C, the hops from the medoid cached.
pub const CACHE_HOPS: &str = "DISKANN.CACHE_HOPS";
/// The most nodes cached, a percentage.
pub const CACHE_PERCENT: &str = "DISKANN.CACHE_PERCENT";
/// k of the merged build.
pub const CLUSTERS: &str = "DISKANN.CLUSTERS";
/// ℓ of the merged build.
pub const CLUSTER_COPIES: &str = "DISKANN.CLUSTER_COPIES";
/// The recall a search aims at, a percentage.
pub const TARGET_ACCURACY: &str = "DISKANN.TARGET_ACCURACY";
/// Whether the executor re-ranks the final candidates by the SQL measure.
pub const RERANK: &str = "DISKANN.RERANK";
/// How node vectors are stored.
pub const NODE_VECTORS: &str = "DISKANN.NODE_VECTORS";
/// Dead nodes, a percentage, past which consolidation is due.
pub const CONSOLIDATE_PERCENT: &str = "DISKANN.CONSOLIDATE_PERCENT";
/// Nodes before an index made by inserts trains its codes.
pub const TRAIN_AT: &str = "DISKANN.TRAIN_AT";
/// Bytes a build holds in memory before it builds merged.
pub const BUILD_MEMORY: &str = "DISKANN.BUILD_MEMORY";

/// The quantizer option's value for none: guided by full vectors.
pub const NO_QUANTIZER: &str = "NONE";
/// The built-in quantizer, the default.
pub const PQ: &str = "PQ";
/// The default metric (the vector document's R10.2).
pub const COSINE: &str = "COSINE";
/// `RERANK`'s values.
pub const SWITCH: &[&str] = &["ON", "OFF"];
/// `NODE_VECTORS`' values: binary32, or binary16 at half the bytes.
pub const ELEMENTS: &[&str] = &[FLOAT32, FLOAT16];
/// Binary32 node vectors.
pub const FLOAT32: &str = "FLOAT32";
/// Binary16 node vectors.
pub const FLOAT16: &str = "FLOAT16";

/// The least R: below it a pruned graph is too sparse to navigate.
const LEAST_DEGREE: i128 = 4;
/// The most a list may be.
const MOST_LIST: i128 = 10_000;
/// α's greatest. Past it nearly every edge is kept and pruning stops
/// meaning anything.
const MOST_ALPHA: f64 = 2.0;
/// W's greatest [§3.3: "16 or more ... could be wasted"].
const MOST_WIDTH: i128 = 16;
/// C's greatest [§3.4: "3 or 4 hops"].
const MOST_HOPS: i128 = 4;
/// k's greatest.
const MOST_CLUSTERS: i128 = 4096;
/// ℓ's greatest.
const MOST_COPIES: i128 = 4;
/// A percentage's whole.
const PERCENT: i128 = 100;
/// The default target accuracy, a percentage.
const DEFAULT_ACCURACY: i128 = 90;
/// The least build memory, 1 MiB.
const LEAST_MEMORY: i128 = 1 << 20;
/// The most code bytes.
const MOST_CODE: i128 = 65_535;

fn whole(n: impl TryInto<i128>) -> Value {
    Value::Exact(Decimal::new(n.try_into().unwrap_or(i128::MAX), 0))
}

fn integer(name: &str, (min, max): (i128, i128), default: Value, (rebuild, doc): (Rebuild, &str)) -> TableOption {
    TableOption::new((name, Kind::Integer { min, max }, default), rebuild, doc)
}

/// The options of a DiskANN index, as the method declares them.
pub fn options() -> Vec<TableOption> {
    let mut all = graph_options();
    all.extend(kept_options());
    all.extend(answer_options());
    all
}

/// An option naming a plug-in, which rebuilds the index.
fn word(name: &str, default: &str, doc: &str) -> TableOption {
    TableOption::new((name, Kind::Identifier, Value::Char(default.into())), Rebuild::Yes, doc)
}

/// The options of the graph's shape, build and search.
fn graph_options() -> Vec<TableOption> {
    vec![
        integer(
            MAX_DEGREE,
            (LEAST_DEGREE, i128::from(MOST_DEGREE)),
            whole(defaults::DEGREE),
            (Rebuild::Yes, "R, the most edges of a node; a node's record is its vector and R edges, which must fit a page."),
        ),
        integer(
            BUILD_LIST,
            (1, MOST_LIST),
            whole(defaults::BUILD_LIST),
            (Rebuild::No, "L of a build and of each insert: the candidate list of the search that finds a node's edges."),
        ),
        TableOption::new(
            (ALPHA, Kind::Number { min: 1.0, max: MOST_ALPHA }, Value::Approx(defaults::ALPHA)),
            Rebuild::No,
            "α of RobustPrune: an edge is kept only if α times nearer than any it shadows, so above 1 long edges stay and searches take fewer hops.",
        ),
        integer(
            BUILD_PASSES,
            (1, 2),
            whole(defaults::PASSES),
            (Rebuild::No, "Vamana passes of a build: 2 for α = 1 then α, 1 for α alone."),
        ),
        integer(
            SEARCH_LIST,
            (1, MOST_LIST),
            whole(defaults::SEARCH_LIST),
            (Rebuild::No, "L of a search, at least the rows asked: longer finds more of the true nearest, slower."),
        ),
        integer(
            BEAM_WIDTH,
            (1, MOST_WIDTH),
            whole(defaults::BEAM_WIDTH),
            (Rebuild::No, "W, the nodes whose pages a search reads together each hop."),
        ),
        word(
            QUANTIZER,
            PQ,
            "The quantizer of the compressed codes in memory that guide a search, by name, or NONE to guide by full vectors.",
        ),
        integer(PQ_BYTES, (0, MOST_CODE), whole(0_u32), (Rebuild::Yes, "Bytes of a node's code; 0 for a quarter of the dimensions.")),
        word(METRIC, COSINE, "The distance the index orders by, by name; a search uses it only for a distance of the same metric."),
    ]
}

/// The options of what is kept in memory, and of builds past it.
fn kept_options() -> Vec<TableOption> {
    vec![
        integer(
            CACHE_HOPS,
            (0, MOST_HOPS),
            whole(defaults::CACHE_HOPS),
            (Rebuild::No, "C: the nodes within C hops of the medoid are kept in memory, read by no search."),
        ),
        integer(
            CACHE_PERCENT,
            (0, PERCENT),
            whole(defaults::CACHE_PERCENT),
            (Rebuild::No, "The most nodes cached, a percentage of the index's nodes."),
        ),
        integer(
            CLUSTERS,
            (2, MOST_CLUSTERS),
            whole(defaults::CLUSTERS),
            (
                Rebuild::No,
                "k of a build past its memory: the clusters whose graphs are built one at a time and merged; more if one would not fit.",
            ),
        ),
        integer(
            CLUSTER_COPIES,
            (1, MOST_COPIES),
            whole(defaults::COPIES),
            (Rebuild::No, "ℓ of a build past its memory: the nearest clusters each point is sent to, so their graphs overlap."),
        ),
    ]
}

/// The options of how a search answers, of maintenance, and of a build's
/// memory.
fn answer_options() -> Vec<TableOption> {
    vec![
        integer(
            TARGET_ACCURACY,
            (1, PERCENT),
            whole(DEFAULT_ACCURACY),
            (
                Rebuild::No,
                "The share of the true nearest rows an approximate search aims to find, a percentage, unless the query sets its own.",
            ),
        ),
        TableOption::new(
            (RERANK, Kind::Choice(SWITCH), Value::Char(SWITCH.first().copied().unwrap_or_default().into())),
            Rebuild::No,
            "Whether the final candidates are ordered again by the SQL distance of each row's vector: ON, or OFF to take the index's order.",
        ),
        TableOption::new(
            (NODE_VECTORS, Kind::Choice(ELEMENTS), Value::Char(FLOAT32.into())),
            Rebuild::Yes,
            "How a node stores its vector: FLOAT32 as given, or FLOAT16 at half the bytes, more nodes to a page.",
        ),
        integer(
            CONSOLIDATE_PERCENT,
            (1, PERCENT),
            whole(defaults::CONSOLIDATE_PERCENT),
            (Rebuild::No, "Deleted nodes, a percentage of the nodes, past which maintenance takes them out of the graph."),
        ),
        integer(
            TRAIN_AT,
            (1, i128::from(u32::MAX)),
            whole(defaults::TRAIN_AT),
            (
                Rebuild::No,
                "Nodes an index made by inserts holds before maintenance trains its codes; until then searches use full vectors.",
            ),
        ),
        integer(
            BUILD_MEMORY,
            (LEAST_MEMORY, i128::from(i64::MAX)),
            whole(defaults::BUILD_MEMORY),
            (Rebuild::No, "Bytes a build holds in memory; past them it builds clusters and merges them."),
        ),
    ]
}

/// An index's options, as its descriptor sets them.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// R.
    pub degree: u32,
    /// How node vectors are stored.
    pub element: Element,
    /// What changes in place.
    pub tuning: Tuning,
    /// The quantizer's name, upper case; `NONE` for none.
    pub quantizer: String,
    /// Code bytes; 0 for a quarter of the dimensions.
    pub code_bytes: u32,
    /// The metric's name, upper case.
    pub metric: String,
    /// The target accuracy, a percentage.
    pub accuracy: u32,
    /// Whether the executor re-ranks.
    pub rerank: bool,
}

/// The options `desc` sets, the others their defaults.
///
/// # Errors
///
/// SQLCODE -103 for a build list below R.
pub fn options_of(desc: &IndexDesc) -> Result<Options> {
    let declared = options();
    let set = &desc.options;
    let count = |name: &str| match option_value(&declared, set, name) {
        Some(Value::Exact(d)) => u64::try_from(d.units).unwrap_or(0),
        Some(_) | None => 0,
    };
    let word = |name: &str, default: &str| match option_value(&declared, set, name) {
        Some(Value::Char(w)) => w.trim().to_ascii_uppercase(),
        Some(_) | None => default.to_owned(),
    };
    let small = |name: &str| u32::try_from(count(name)).unwrap_or(u32::MAX);
    let tuning = Tuning {
        build_list: small(BUILD_LIST),
        alpha: option_value(&declared, set, ALPHA).and_then(Value::as_f64).unwrap_or(defaults::ALPHA),
        passes: u8::try_from(count(BUILD_PASSES)).unwrap_or(defaults::PASSES),
        search_list: small(SEARCH_LIST),
        beam_width: small(BEAM_WIDTH),
        cache_hops: u8::try_from(count(CACHE_HOPS)).unwrap_or(defaults::CACHE_HOPS),
        cache_percent: small(CACHE_PERCENT),
        clusters: small(CLUSTERS),
        copies: small(CLUSTER_COPIES),
        consolidate_percent: small(CONSOLIDATE_PERCENT),
        train_at: small(TRAIN_AT),
        build_memory: usize::try_from(count(BUILD_MEMORY)).unwrap_or(usize::MAX),
    };
    let degree = small(MAX_DEGREE);
    if tuning.build_list < degree {
        return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] DISKANN.BUILD_LIST is at least DISKANN.MAX_DEGREE"));
    }
    let element = if word(NODE_VECTORS, FLOAT32) == FLOAT16 { Element::Float16 } else { Element::Float32 };
    Ok(Options {
        degree,
        element,
        tuning,
        quantizer: word(QUANTIZER, PQ),
        code_bytes: small(PQ_BYTES),
        metric: word(METRIC, COSINE),
        accuracy: small(TARGET_ACCURACY),
        rerank: word(RERANK, "ON") != "OFF",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    fn desc(options: Vec<(String, Value)>) -> IndexDesc {
        IndexDesc {
            name: "V".into(),
            table: QualifiedName { auth: "U".to_owned(), ident: "T".to_owned() },
            columns: vec![0],
            method: "diskann".into(),
            unique: false,
            options,
            collations: Vec::new(),
            nulls: false,
        }
    }

    /// Verifies: [EXTENSION] (RULES.md D41) Every option is named once,
    /// upper case under DISKANN.; unset options take the paper's
    /// defaults; set ones are read; a build list below R is refused with
    /// SQLCODE -103.
    #[test]
    fn options_default_to_the_paper_and_read_what_is_set() {
        let declared = options();
        let mut names: Vec<&str> = declared.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), declared.len());
        assert!(names.iter().all(|n| n.starts_with("DISKANN.")));
        let read = options_of(&desc(Vec::new())).unwrap();
        assert_eq!((read.degree, read.element, read.tuning), (defaults::DEGREE, Element::Float32, Tuning::default()));
        assert_eq!((read.quantizer.as_str(), read.metric.as_str(), read.accuracy, read.rerank), (PQ, COSINE, 90, true));
        let set = vec![
            (MAX_DEGREE.into(), whole(32_u32)),
            (ALPHA.into(), Value::Approx(1.5_f64)),
            (NODE_VECTORS.into(), Value::Char(FLOAT16.into())),
            (RERANK.into(), Value::Char("OFF".into())),
            (QUANTIZER.into(), Value::Char("none".into())),
        ];
        let changed = options_of(&desc(set)).unwrap();
        assert_eq!(
            (changed.degree, changed.element, changed.rerank, changed.quantizer.as_str()),
            (32, Element::Float16, false, NO_QUANTIZER)
        );
        assert!((changed.tuning.alpha - 1.5_f64).abs() < f64::EPSILON);
        let short = options_of(&desc(vec![(BUILD_LIST.into(), whole(10_u32))])).unwrap_err();
        assert_eq!(short.code, Code::SyntaxRule);
    }
}
