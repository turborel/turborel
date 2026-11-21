//! \[DESIGN\] (RULES.md D654) What a row hashed into a table, a hash join's
//! built rows or a grouping's groups, costs by the size of the table: a
//! table a core's cache holds costs a hash and a probe of a line already
//! near, and one far past it a miss for each row, so a row hashed into a
//! large table costs several rows of a scan where one into a small table
//! costs about one, as `bench/analytics/costs.rs` measures (its grouping
//! of a thousand groups against its hash join of a million built rows).
//! Between the two the cost grows with the logarithm of the table's
//! bytes, as each doubling past the cache leaves fewer of its lines near.
//! So a hash join of a large build loses to probing an index sooner than
//! one of a small build, and the build side is weighed by its size.

/// \[DESIGN\] The cost of a row hashed into a table the cache holds, in rows
/// of a scan: the grouping of a thousand groups, a row hashed and folded.
pub(in crate::physical) const HASH_SMALL: f64 = 1.0;
/// \[DESIGN\] The cost of a row hashed into a table far past the cache, in
/// rows of a scan: the hash join of a million built rows, a row of either
/// operand hashed.
pub(in crate::physical) const HASH_LARGE: f64 = 3.7;
/// \[DESIGN\] The bytes of a table the cache of a core holds.
const CACHED_BYTES: f64 = 1_048_576.0;
/// \[DESIGN\] The bytes of a table each of whose rows misses the cache: the
/// measured hash join's built rows, a million of about 32 bytes.
const MISSED_BYTES: f64 = 33_554_432.0;

/// The cost of a row hashed into a table of `bytes` bytes, or probing it.
pub(in crate::physical) fn hash_row(bytes: f64) -> f64 {
    if bytes <= CACHED_BYTES {
        return HASH_SMALL;
    }
    let past = (bytes / CACHED_BYTES).log2() / (MISSED_BYTES / CACHED_BYTES).log2();
    HASH_SMALL + (HASH_LARGE - HASH_SMALL) * past.min(1.0_f64)
}
