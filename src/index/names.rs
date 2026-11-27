//! The names of the built-in methods the rest of the database names.

/// The name of `BTreeMethod`, which also enforces UNIQUE constraints unless told otherwise.
pub const BTREE_METHOD: &str = "btree";
/// The name of `HashMethod`.
pub const HASH_METHOD: &str = "hash";

/// The name of `RTreeMethod`, for finite Cartesian point and rectangle keys.
pub const RTREE_METHOD: &str = "rtree";
