//! \[EXTENSION\] An IVF index's options (RULES.md D41), set by `CREATE
//! INDEX ... USING ivf WITH (...)` and changed by `ALTER INDEX ... SET`.
//! The metric and the quantizer fix what a list stores, so they make the
//! index anew; the rest change in place (`Ivf::tune`), a changed count of
//! lists taking effect at the next maintenance's training. Defaults are
//! the vector design document's (§10.4).

use crate::catalog::IndexDesc;
use crate::engine::ivf::{Config, MAX_LISTS};
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};

/// NLIST, the lists; 0 for sqrt(N).
pub const LISTS: &str = "IVF.LISTS";
/// NPROBE, the lists a search scans; 0 for an eighth of them.
pub const PROBES: &str = "IVF.PROBES";
/// The vectors sampled to train each list's centroid.
pub const SAMPLE_PER_LIST: &str = "IVF.SAMPLE_PER_LIST";
/// The metric, by name.
pub const METRIC: &str = "IVF.METRIC";
/// The quantizer of the lists' codes, by name, or `NONE`.
pub const QUANTIZER: &str = "IVF.QUANTIZER";
/// Whether a quantized search gives more candidates to re-rank.
pub const RERANK: &str = "IVF.RERANK";

/// The quantizer option's value for none: lists keep full vectors.
pub const NO_QUANTIZER: &str = "NONE";
/// The default metric (the vector document's R10.2).
pub const COSINE: &str = "COSINE";
/// The switches' values.
pub const SWITCH: &[&str] = &[ON, OFF];
/// A switch on.
pub const ON: &str = "ON";
/// A switch off.
pub const OFF: &str = "OFF";

/// \[DESIGN\] The default vectors sampled per list (the document's
/// SAMPLE_PER_PARTITION).
pub const DEFAULT_SAMPLE_PER_LIST: usize = 256;
/// \[DESIGN\] The seed of every index's k-means and quantizer training,
/// so one set of rows always makes one partitioning.
pub const SEED: u64 = 0x4956_4631;
/// The most vectors sampled per list.
const MOST_SAMPLE: i128 = 65_536;

fn whole(n: impl TryInto<i128>) -> Value {
    Value::Exact(Decimal::new(n.try_into().unwrap_or(i128::MAX), 0))
}

fn integer(name: &str, (min, max): (i128, i128), default: usize, doc: &str) -> TableOption {
    TableOption::new((name, Kind::Integer { min, max }, whole(default)), Rebuild::No, doc)
}

/// The options of an IVF index, as the method declares them.
pub fn options() -> Vec<TableOption> {
    let words =
        |name: &str, default: &str, doc: &str| TableOption::new((name, Kind::Identifier, Value::Char(default.into())), Rebuild::Yes, doc);
    let most = i128::try_from(MAX_LISTS).unwrap_or(i128::MAX);
    vec![
        integer(
            LISTS,
            (0, most),
            0,
            "NLIST, the lists vectors are partitioned into by their nearest centroid; 0 for the square root of the rows, at least 16. A change takes effect at the next training.",
        ),
        integer(PROBES, (0, most), 0, "NPROBE, the nearest lists a search scans, unless the query sets its own; 0 for an eighth of them."),
        integer(
            SAMPLE_PER_LIST,
            (1, MOST_SAMPLE),
            DEFAULT_SAMPLE_PER_LIST,
            "The vectors sampled to train each list's centroid, at most 100000 in all.",
        ),
        words(METRIC, COSINE, "The distance the index orders by, by name; a search uses it only for a distance of the same metric."),
        words(
            QUANTIZER,
            NO_QUANTIZER,
            "The quantizer of the codes lists keep in place of vectors, by name, as SQ8, or NONE to keep full vectors.",
        ),
        TableOption::new(
            (RERANK, Kind::Choice(SWITCH), Value::Char(ON.into())),
            Rebuild::No,
            "Whether a quantized search gives four times the rows asked, for the executor to order again by the SQL distance of each row's vector.",
        ),
    ]
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
        lists: count(LISTS),
        probes: count(PROBES),
        sample_per_list: count(SAMPLE_PER_LIST),
        seed: SEED,
        rerank: word(RERANK, ON) == ON,
    };
    Options { config, metric: word(METRIC, COSINE), quantizer: word(QUANTIZER, NO_QUANTIZER) }
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
            method: "ivf".into(),
            unique: false,
            options,
            collations: Vec::new(),
            nulls: false,
        }
    }

    /// Verifies: [EXTENSION] (RULES.md D41) every option is named once,
    /// upper case under IVF.; unset options take the defaults; set ones
    /// are read, words in any case; only the metric and quantizer rebuild.
    #[test]
    fn ivf_options_default_and_read_what_is_set() {
        let declared = options();
        let mut names: Vec<&str> = declared.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), declared.len());
        assert!(names.iter().all(|n| n.starts_with("IVF.")));
        let read = options_of(&desc(Vec::new()), 8);
        let config = Config { dims: 8, lists: 0, probes: 0, sample_per_list: 256, seed: SEED, rerank: true };
        assert_eq!(read, Options { config, metric: COSINE.into(), quantizer: NO_QUANTIZER.into() });
        let set = vec![
            (LISTS.into(), whole(100_u32)),
            (PROBES.into(), whole(10_u32)),
            (RERANK.into(), Value::Char("off".into())),
            (METRIC.into(), Value::Char("euclidean".into())),
        ];
        let changed = options_of(&desc(set), 8);
        assert_eq!(
            (changed.config.lists, changed.config.probes, changed.config.rerank, changed.metric.as_str()),
            (100, 10, false, "EUCLIDEAN")
        );
        let rebuilt: Vec<&str> = declared.iter().filter(|o| o.rebuild == Rebuild::Yes).map(|o| o.name.as_str()).collect();
        assert_eq!(rebuilt, vec![METRIC, QUANTIZER]);
    }
}
