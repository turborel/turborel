//! The mutations of xxHash (RULES.md R-TEST-9, D460): XXH3's mid-size
//! seed, its scramble, its streamed last stripe and its mid-size rounds,
//! XXH64's merge, the hasher's widths and the words' hash.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the xxHash requirements.
pub const XXHASH: [Mutation; 7] = [
    Mutation {
        name: "xxh3-mid-seed-sign",
        package: Package::Core,
        breaks: "XXH3 of 17 to 240 bytes adds the seed to a chunk's second key word, not takes it away",
        file: "src/hash/xxhash/mid.rs",
        original: "high ^ key_high.wrapping_sub(seed))",
        replacement: "high ^ key_high.wrapping_add(seed))",
        integration: &[],
        unit: &["hash::xxhash::tests::published_vectors_at_the_boundaries", "hash::xxhash::tests::every_published_vector_folds_alike"],
    },
    Mutation {
        name: "xxh3-mid-rounds",
        package: Package::Core,
        breaks: "XXH3 of 97 to 128 bytes mixes three pairs of chunks, not four",
        file: "src/hash/xxhash/mid.rs",
        original: "(len.wrapping_sub(1) >> 5_u32).min(3)",
        replacement: "(len.wrapping_sub(1) >> 5_u32).min(2)",
        integration: &[],
        unit: &["hash::xxhash::tests::published_vectors_at_the_boundaries", "hash::xxhash::tests::every_published_vector_folds_alike"],
    },
    Mutation {
        name: "xxh3-scramble-shift",
        package: Package::Core,
        breaks: "XXH3's scramble shifts an accumulator by 46, not 47",
        file: "src/hash/xxhash/long.rs",
        original: "const SCRAMBLE_SHIFT: u32 = 47;",
        replacement: "const SCRAMBLE_SHIFT: u32 = 46;",
        integration: &[],
        unit: &["hash::xxhash::tests::published_vectors_at_the_boundaries", "hash::xxhash::tests::every_published_vector_folds_alike"],
    },
    Mutation {
        name: "xxh3-stream-last-stripe",
        package: Package::Core,
        breaks: "a stream given many stripes at once keeps no last stripe for its digest",
        file: "src/hash/xxhash/stream.rs",
        original: "                *slot = *last;",
        replacement: "                let _ = (slot, last);",
        integration: &[],
        unit: &["hash::xxhash::tests::xxh3_streamed_equals_at_once_at_every_split"],
    },
    Mutation {
        name: "xxh64-merge-round",
        package: Package::Core,
        breaks: "XXH64 merges its accumulators adding the third prime, not the fourth",
        file: "src/hash/xxhash/xxh64.rs",
        original: "(acc ^ round(0, value)).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4)",
        replacement: "(acc ^ round(0, value)).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_3)",
        integration: &[],
        unit: &["hash::xxhash::tests::published_vectors_at_the_boundaries", "hash::xxhash::tests::every_published_vector_folds_alike"],
    },
    Mutation {
        name: "xxh3-hasher-width",
        package: Package::Core,
        breaks: "the hasher counts an integer written one byte short",
        file: "src/hash/xxhash/hasher.rs",
        original: "self.len = self.len.wrapping_add(N);",
        replacement: "self.len = self.len.wrapping_add(N).saturating_sub(1);",
        integration: &[],
        unit: &["hash::xxhash::tests::the_hasher_is_xxh3_of_the_bytes_written"],
    },
    Mutation {
        name: "xxh3-word-rotation",
        package: Package::Core,
        breaks: "a word hashes unrotated, not as XXH3 of its 8 bytes",
        file: "src/hash/xxhash.rs",
        original: "rrmxmx(word.rotate_left(32) ^ self.bitflip, WORD_LEN)",
        replacement: "rrmxmx(word ^ self.bitflip, WORD_LEN)",
        integration: &[],
        unit: &["hash::xxhash::tests::words_hash_as_their_bytes"],
    },
];
