//! \[EXTENSION\] The settings of graph pattern matching [GS §20]: the
//! limits of a GRAPH_TABLE's path search, what reaching one does, whether
//! its paths are found in an order repeated run to run, and the work a
//! search needs before it uses workers. A GRAPH_TABLE's OPTIONS overrides
//! each limit for itself (`exec/graph_table/limits.rs`).

use super::builtin::{text, whole};
use super::{Definition, Effect, Key, Kind, OFF, SWITCHES};

/// GRAPH.MAX_PATH_LENGTH.
pub const GRAPH_MAX_PATH_LENGTH: Key = Key(53);
/// GRAPH.MAX_PATHS_PER_PARTITION.
pub const GRAPH_MAX_PATHS_PER_PARTITION: Key = Key(54);
/// GRAPH.MAX_SEARCH_STATES.
pub const GRAPH_MAX_SEARCH_STATES: Key = Key(55);
/// GRAPH.ON_LIMIT.
pub const GRAPH_ON_LIMIT: Key = Key(56);
/// GRAPH.DETERMINISTIC_PATHS.
pub const GRAPH_DETERMINISTIC_PATHS: Key = Key(57);
/// GRAPH.PARALLEL_MIN_STATES.
pub const GRAPH_PARALLEL_MIN_STATES: Key = Key(60);

/// \[DESIGN\] The default of GRAPH.PARALLEL_MIN_STATES: below it, starting
/// and joining workers costs more than the states they would share.
pub const DEFAULT_PARALLEL_MIN_STATES: i128 = 16_384;

/// \[DESIGN\] The default of GRAPH.MAX_SEARCH_STATES: enough for any
/// search of a graph held in memory, and a bound on one that never ends.
pub const DEFAULT_MAX_SEARCH_STATES: i128 = 10_000_000;

/// The words of GRAPH.ON_LIMIT.
pub const ON_LIMIT_WORDS: &[&str] = &[ERROR, TRUNCATE];
/// ON LIMIT ERROR.
pub const ERROR: &str = "ERROR";
/// ON LIMIT TRUNCATE.
pub const TRUNCATE: &str = "TRUNCATE";

/// A setting a session may set for itself, from its next statement.
const SESSION: (bool, Effect) = (true, Effect::Statement);

/// The largest whole number a count setting holds.
const MOST: i128 = 9_223_372_036_854_775_807;

/// A whole number from 0 up.
const COUNT: Kind = Kind::Integer { min: 0, max: MOST };

/// One setting.
fn defined((name, kind, default): (&str, Kind, crate::value::Value), doc: &str) -> Definition {
    let (per_session, effect) = SESSION;
    Definition { name: name.to_owned(), kind, default, per_session, node: false, effect, doc: doc.to_owned() }
}

/// The settings of graph pattern matching, in the order of their keys.
pub(super) fn definitions() -> [Definition; 5] {
    [
        defined(
            ("GRAPH.MAX_PATH_LENGTH", COUNT, whole(0)),
            "The most edges of a path a GRAPH_TABLE's search follows; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.",
        ),
        defined(
            ("GRAPH.MAX_PATHS_PER_PARTITION", COUNT, whole(0)),
            "The most paths a GRAPH_TABLE's search finds between one start and one end; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.",
        ),
        defined(
            ("GRAPH.MAX_SEARCH_STATES", COUNT, whole(DEFAULT_MAX_SEARCH_STATES)),
            "The most states a GRAPH_TABLE's search makes, each a vertex reached at a place of its pattern; 0 for no limit. Reaching it does as GRAPH.ON_LIMIT says.",
        ),
        defined(
            ("GRAPH.ON_LIMIT", Kind::Choice(ON_LIMIT_WORDS), text(ERROR)),
            "What a GRAPH_TABLE's search does on reaching a limit: fails, SQLSTATE 54061, ERROR; or ends with the paths found, warning 01W61, TRUNCATE.",
        ),
        defined(
            ("GRAPH.DETERMINISTIC_PATHS", Kind::Choice(SWITCHES), text(OFF)),
            "Whether a GRAPH_TABLE's selectors that keep some of equal paths, as ANY and ANY SHORTEST, keep the same ones each run, ON, ordered by the elements' identities; or any, OFF.",
        ),
    ]
}

/// The settings of graph pattern matching declared after GQL's, in the
/// order of their keys.
pub(super) fn later() -> [Definition; 1] {
    [defined(
        ("GRAPH.PARALLEL_MIN_STATES", COUNT, whole(DEFAULT_PARALLEL_MIN_STATES)),
        "The least states, estimated from the graph's size, a GRAPH_TABLE's search makes for it to run on the workers EXECUTOR.GRAPH_THREADS allows; a smaller one runs on the statement's thread alone; 0 for any search. Its results are the same either way.",
    )]
}
