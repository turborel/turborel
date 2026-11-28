//! The mutations of the hashes a memo of codes reads (RULES.md R-TEST-9,
//! D1330): the place a hash is read at, the order two columns' hashes are
//! joined in, the bound a run's entries are hashed whole within, the
//! dictionaries they are hashed of, a column grouped unpadded and a
//! null's place.

use super::super::{Mutation, Package};

/// The file the memo is in.
const CODED: &str = "src/exec/batch/groups/coded.rs";

/// The file a dictionary's entries are hashed in.
const HASH: &str = "src/exec/table/hash.rs";

/// A place reads the hash its row is given.
const READ: &str = "exec::batch::groups::coded::tests::a_place_reads_the_hash_its_row_is_given";

/// A run of more entries than rows given keeps no hash.
const BOUND: &str = "exec::batch::groups::coded::tests::a_run_of_more_entries_than_rows_given_is_not_hashed_whole";

/// Mechanisms whose removal must fail the requirements of the hashes a
/// memo of codes reads.
pub const CODED_HASHES: [Mutation; 6] = [
    Mutation {
        name: "coded-hash-read-of-the-next-place",
        package: Package::Core,
        breaks: "a place reads the hash of the entry after its own, so a key is parted by another key's hash (RULES.md D1330)",
        file: CODED,
        original: "return only.get(place).copied();",
        replacement: "return only.get(place.wrapping_add(1)).copied();",
        integration: &[],
        unit: &[READ],
    },
    Mutation {
        name: "coded-hashes-joined-out-of-order",
        package: Package::Core,
        breaks: "a key of two columns joins the second's hash before the first's, unlike the hash its row is given (RULES.md D1330)",
        file: CODED,
        original: "seed.joined(so_far, hash)",
        replacement: "seed.joined(hash, so_far)",
        integration: &[],
        unit: &[READ],
    },
    Mutation {
        name: "coded-run-hashed-past-its-rows",
        package: Package::Core,
        breaks: "a run's entries are hashed whole however few rows are given, so a few rows pay for every entry (RULES.md D1330)",
        file: CODED,
        original: "if entries > given || !plain {",
        replacement: "if !plain {",
        integration: &[],
        unit: &[BOUND],
    },
    Mutation {
        name: "coded-entries-of-codes-hashed-as-texts",
        package: Package::Core,
        breaks: "a dictionary whose entries are codes is hashed of its own empty text, so a key reads a null's hash (RULES.md D1330)",
        file: CODED,
        original: "if entries > given || !plain {",
        replacement: "if entries > given {",
        integration: &[],
        unit: &[BOUND],
    },
    Mutation {
        name: "coded-entry-of-an-unpadded-column-hashed-padded",
        package: Package::Core,
        breaks: "an entry of a column grouped unpadded is hashed less its trailing spaces, unlike its row (RULES.md D1183, D1330)",
        file: HASH,
        original: "None, (seed, kernels, seed.is_unpadded(place)), out);",
        replacement: "None, (seed, kernels, false), out);",
        integration: &[],
        unit: &[READ],
    },
    Mutation {
        name: "coded-null-place-of-no-null-hash",
        package: Package::Core,
        breaks: "a null's place reads a hash that is no null's, so a null key is parted apart from other tables' nulls (RULES.md D1330)",
        file: HASH,
        original: "    out.push(seed.null());\n",
        replacement: "    out.push(0);\n",
        integration: &[],
        unit: &[READ],
    },
];
