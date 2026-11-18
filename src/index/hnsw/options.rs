//! \[EXTENSION\] An HNSW index's options (RULES.md D41), set by `CREATE
//! INDEX ... USING hnsw WITH (...)` and changed by `ALTER INDEX ... SET`.
//! Those fixing the graph's shape or what a node stores (M, the metric
//! and the quantizer) make the index anew; the rest change in place
//! (`Hnsw::tune`). Defaults are the paper's where it gives one [Malkov
//! and Yashunin, TPAMI 2020 §4.1, §5].

use crate::catalog::IndexDesc;
use crate::engine::hnsw::{Config, SQ8};
use crate::index::vector::{Shared, shared, shared_of};
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};

/// M, the links a node keeps on each layer above the first; 2M on it.
pub const M: &str = "HNSW.M";
/// efConstruction, the candidates an insert's search keeps.
pub const EF_CONSTRUCTION: &str = "HNSW.EF_CONSTRUCTION";
/// efSearch, the candidates a search keeps, unless the query sets its own.
pub const EF_SEARCH: &str = "HNSW.EF_SEARCH";
/// Whether neighbour selection extends its candidates by theirs.
pub const EXTEND_CANDIDATES: &str = "HNSW.EXTEND_CANDIDATES";
/// Whether neighbour selection fills a node's links with pruned candidates.
pub const KEEP_PRUNED: &str = "HNSW.KEEP_PRUNED";
/// The metric, by name.
pub const METRIC: &str = "HNSW.METRIC";
/// The quantizer of the nodes' codes, by name, or `NONE`.
pub const QUANTIZER: &str = "HNSW.QUANTIZER";
/// The nodes before an index trains its quantizer.
pub const TRAIN_AT: &str = "HNSW.TRAIN_AT";
/// Whether a quantized search gives more candidates to re-rank.
pub const RERANK: &str = "HNSW.RERANK";

/// The group of its options.
const GROUP: &str = "HNSW";
/// The quantizer option's value for none: nodes keep full vectors.
pub const NO_QUANTIZER: &str = "NONE";
/// The default metric (the vector document's R10.2).
pub const COSINE: &str = "COSINE";
/// The switches' values.
pub const SWITCH: &[&str] = &[ON, OFF];
/// A switch on.
pub const ON: &str = "ON";
/// A switch off.
pub const OFF: &str = "OFF";

/// \[DESIGN\] The default M: the paper finds 5 to 48 good [§5], higher
/// for higher dimensions and recall.
pub const DEFAULT_M: usize = 16;
/// \[DESIGN\] The default efConstruction [§5: 100 to 200 gives ~0.95].
pub const DEFAULT_EF_CONSTRUCTION: usize = 200;
/// \[DESIGN\] The default efSearch.
pub const DEFAULT_EF_SEARCH: usize = 64;
/// \[DESIGN\] The default nodes before an index trains its quantizer.
pub const DEFAULT_TRAIN_AT: usize = 10_000;
/// \[DESIGN\] The seed of every index's levels and quantizer training,
/// so one set of rows always makes one graph.
pub const SEED: u64 = 0x484E_5357;
/// The least M.
const LEAST_M: i128 = 2;
/// The most M.
const MOST_M: i128 = 512;
/// The most any ef may be.
const MOST_EF: i128 = 10_000;

fn whole(n: impl TryInto<i128>) -> Value {
    Value::Exact(Decimal::new(n.try_into().unwrap_or(i128::MAX), 0))
}

fn integer(name: &str, (min, max): (i128, i128), default: usize, (rebuild, doc): (Rebuild, &str)) -> TableOption {
    TableOption::new((name, Kind::Integer { min, max }, whole(default)), rebuild, doc)
}

fn switch(name: &str, default: &str, doc: &str) -> TableOption {
    TableOption::new((name, Kind::Choice(SWITCH), Value::Char(default.into())), Rebuild::No, doc)
}

/// The options of an HNSW index, as the method declares them.
pub fn options() -> Vec<TableOption> {
    let words =
        |name: &str, default: &str, doc: &str| TableOption::new((name, Kind::Identifier, Value::Char(default.into())), Rebuild::Yes, doc);
    vec![
        integer(
            M,
            (LEAST_M, MOST_M),
            DEFAULT_M,
            (Rebuild::Yes, "M, the most links of a node on each layer above the first, and 2M on the first."),
        ),
        integer(
            EF_CONSTRUCTION,
            (1, MOST_EF),
            DEFAULT_EF_CONSTRUCTION,
            (Rebuild::No, "efConstruction, the candidates an insert's search keeps to choose a node's links from."),
        ),
        integer(
            EF_SEARCH,
            (1, MOST_EF),
            DEFAULT_EF_SEARCH,
            (Rebuild::No, "efSearch, the candidates a search keeps, at least the rows asked, unless the query sets its own."),
        ),
        switch(
            EXTEND_CANDIDATES,
            OFF,
            "Whether choosing a node's links considers its candidates' own links too: ON for clustered data, slower inserts.",
        ),
        switch(KEEP_PRUNED, ON, "Whether a node's links the heuristic leaves unfilled are filled with the nearest candidates it pruned."),
        words(METRIC, COSINE, "The distance the index orders by, by name; a search uses it only for a distance of the same metric."),
        words(
            QUANTIZER,
            NO_QUANTIZER,
            "The quantizer of the codes nodes keep in place of their vectors, by name, as SQ8, or NONE to keep full vectors.",
        ),
        integer(
            TRAIN_AT,
            (1, i128::from(u32::MAX)),
            DEFAULT_TRAIN_AT,
            (Rebuild::No, "Nodes an index holds before maintenance trains its quantizer; until then nodes keep full vectors."),
        ),
        switch(
            RERANK,
            ON,
            "Whether a quantized search gives four times the rows asked, for the executor to order again by the SQL distance of each row's vector.",
        ),
    ]
    .into_iter()
    .chain(shared(GROUP))
    .collect()
}

/// An index's options, as its descriptor sets them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The engine's configuration, but for its dimensions.
    pub config: Config,
    /// The metric's name, upper case.
    pub metric: String,
    /// The quantizer's name, upper case; `NONE` for none.
    pub quantizer: String,
    /// TARGET_ACCURACY, PQ_BYTES and NORMALIZED (`index/vector/options.rs`).
    pub(crate) shared: Shared,
}

/// The options `desc` sets, the others their defaults, for vectors of
/// `dims` dimensions.
pub fn options_of(desc: &IndexDesc, dims: usize) -> Options {
    let declared = options();
    let set = &desc.options;
    let count = |name: &str| match option_value(&declared, set, name) {
        Some(Value::Exact(d)) => usize::try_from(d.units).unwrap_or(0),
        Some(_) | None => 0,
    };
    let word = |name: &str, default: &str| match option_value(&declared, set, name) {
        Some(Value::Char(w)) => w.trim().to_ascii_uppercase(),
        Some(_) | None => default.to_owned(),
    };
    let config = Config {
        dims,
        m: count(M),
        ef_construction: count(EF_CONSTRUCTION),
        ef_search: count(EF_SEARCH),
        extend: word(EXTEND_CANDIDATES, OFF) == ON,
        keep_pruned: word(KEEP_PRUNED, ON) == ON,
        seed: SEED,
        train_rows: count(TRAIN_AT),
        rerank: word(RERANK, ON) == ON,
    };
    let shared = shared_of(desc, GROUP, EF_SEARCH);
    Options { config, metric: word(METRIC, COSINE), quantizer: word(QUANTIZER, NO_QUANTIZER), shared }
}

/// Whether `name` is the built-in quantizer's.
pub fn built_in(name: &str) -> bool {
    name == SQ8
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
        }
    }

    /// Verifies: [EXTENSION] (RULES.md D41) every option is named once,
    /// upper case under HNSW.; unset options take the defaults; set ones
    /// are read, words in any case.
    #[test]
    fn hnsw_options_default_and_read_what_is_set() {
        let declared = options();
        let mut names: Vec<&str> = declared.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), declared.len());
        assert!(names.iter().all(|n| n.starts_with("HNSW.")));
        let read = options_of(&desc(Vec::new()), 8);
        let config = Config {
            dims: 8,
            m: 16,
            ef_construction: 200,
            ef_search: 64,
            extend: false,
            keep_pruned: true,
            seed: SEED,
            train_rows: 10_000,
            rerank: true,
        };
        let shared = Shared { accuracy: Some(90), code_bytes: 0, normalized: false };
        assert_eq!(read, Options { config, metric: COSINE.into(), quantizer: NO_QUANTIZER.into(), shared });
        let set = vec![
            (M.into(), whole(32_u32)),
            (EF_SEARCH.into(), whole(200_u32)),
            (EXTEND_CANDIDATES.into(), Value::Char("on".into())),
            (QUANTIZER.into(), Value::Char("sq8".into())),
        ];
        let changed = options_of(&desc(set), 8);
        assert_eq!((changed.config.m, changed.config.ef_search, changed.config.extend, changed.quantizer.as_str()), (32, 200, true, SQ8));
        assert!(built_in(&changed.quantizer));
        assert!(declared.iter().any(|o| o.name == M && o.rebuild == Rebuild::Yes));
        assert!(declared.iter().any(|o| o.name == EF_SEARCH && o.rebuild == Rebuild::No));
    }
}
