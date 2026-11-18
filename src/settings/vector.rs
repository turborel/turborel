//! \[EXTENSION\] The settings of approximate vector search (RULES.md D36,
//! D-APPROX; the vector design document R9.2, R9.3, R13.1), declared after
//! the other built-in settings, each at its key's place. An index method
//! reads VECTOR.BUILD_MEMORY_BYTES as it is told the settings
//! (`Configurable::configure`).

use super::builtin::{OFF, ON, SWITCHES, text, whole};
use super::{Definition, Effect, Key, Kind};

/// VECTOR.SEARCH_LIST: the candidates a search keeps, 0 for the index's own.
pub const VECTOR_SEARCH_LIST: Key = Key(47);
/// VECTOR.BEAM_WIDTH: the nodes whose neighbours a beam reads together.
pub const VECTOR_BEAM_WIDTH: Key = Key(48);
/// VECTOR.BUILD_MEMORY_BYTES: the bytes a build holds before it merges.
pub const VECTOR_BUILD_MEMORY_BYTES: Key = Key(49);
/// VECTOR.APPROX_FALLBACK: whether a short approximate search reads the table.
pub const VECTOR_APPROX_FALLBACK: Key = Key(50);
/// VECTOR.APPROX_DEFAULT: whether a FETCH FIRST of neither APPROX nor
/// EXACT may read an approximate index.
pub const VECTOR_APPROX_DEFAULT: Key = Key(51);
/// The name of VECTOR.BUILD_MEMORY_BYTES, as an index method looks it up.
pub const BUILD_MEMORY_BYTES: &str = "VECTOR.BUILD_MEMORY_BYTES";

/// \[EXTENSION\] The most candidates a search keeps: DiskANN's and HNSW's
/// own greatest list.
const MOST_SEARCH_LIST: i128 = 10_000;
/// \[EXTENSION\] The widest beam: past 16 a beam wastes reads (the DiskANN
/// paper §3.3).
const MOST_BEAM_WIDTH: i128 = 16;
/// \[EXTENSION\] The least build memory: one 64 KiB page.
const LEAST_BUILD_MEMORY: i128 = 1 << 16_i32;
/// \[EXTENSION\] The default build memory, 256 MiB, as DiskANN's own.
const BUILD_MEMORY: i128 = 256 << 20_i32;

/// A session's own setting, taking effect for the statements after it.
const SESSION: (bool, Effect) = (true, Effect::Statement);

fn defined((name, kind, default): (&str, Kind, super::Value), (per_session, effect): (bool, Effect), doc: &str) -> Definition {
    Definition { name: name.to_owned(), kind, default, per_session, node: false, effect, doc: doc.to_owned() }
}

/// The settings, in the order of their keys.
pub(super) fn vector() -> [Definition; 5] {
    [
        defined(
            ("VECTOR.SEARCH_LIST", Kind::Integer { min: 0, max: MOST_SEARCH_LIST }, whole(0)),
            SESSION,
            "The candidates an approximate search keeps, HNSW's ef and DiskANN's L, unless the query names its own; 0 for each index's own.",
        ),
        defined(
            ("VECTOR.BEAM_WIDTH", Kind::Integer { min: 0, max: MOST_BEAM_WIDTH }, whole(0)),
            SESSION,
            "The nodes whose neighbours a DiskANN search reads together each hop, unless the query names its own; 0 for each index's own.",
        ),
        defined(
            (BUILD_MEMORY_BYTES, Kind::Integer { min: LEAST_BUILD_MEMORY, max: i128::from(i64::MAX) }, whole(BUILD_MEMORY)),
            (false, Effect::Statement),
            "The bytes a vector index's build holds in memory; past them it builds parts and merges them.",
        ),
        defined(
            ("VECTOR.APPROX_FALLBACK", Kind::Choice(SWITCHES), text(ON)),
            SESSION,
            "Whether an approximate search that finds fewer rows than asked, once widened, or whose index declines, reads the whole table exactly: ON; or is refused, OFF.",
        ),
        defined(
            ("VECTOR.APPROX_DEFAULT", Kind::Choice(SWITCHES), text(OFF)),
            SESSION,
            "Whether a FETCH FIRST of neither APPROX nor EXACT may read an approximate index, as FETCH APPROX does: ON; or is exact, OFF.",
        ),
    ]
}
