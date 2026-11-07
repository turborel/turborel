//! The mutations of shadow files (RULES.md R-TEST-9): a granule held once
//! in its side file, and what recovery takes a torn block for.

use super::{Mutation, Package};

/// The mutations of shadow files, in the order they run.
pub const SHADOW: [Mutation; 3] = [
    Mutation {
        name: "side-granule-once",
        package: Package::Core,
        breaks: "shadow files: a granule written again is appended, so a side file grows by what is rewritten",
        file: "src/disk/shadow/side.rs",
        original: "            (Some(&block), Some(blocks)) => blocks.overwrite(block, &encode(op)).map(|()| block),",
        replacement: "            (Some(&block), Some(blocks)) if false => blocks.overwrite(block, &encode(op)).map(|()| block),",
        integration: &[],
        unit: &["disk::shadow::tests::a_granule_written_again_takes_its_block"],
    },
    Mutation {
        name: "side-synced-before-commit",
        package: Package::Core,
        breaks: "shadow files: a commit block can reach the disk with a block written over torn before it",
        file: "src/disk/shadow/file.rs",
        original: "        blocks.sync(Durability::Full)?;\n        blocks.append(&encode(&Op::Commit))?;",
        replacement: "        blocks.append(&encode(&Op::Commit))?;",
        integration: &[],
        unit: &["disk::shadow::tests::a_block_damaged_before_a_commit_block_is_refused"],
    },
    Mutation {
        name: "side-damage-refused",
        package: Package::Core,
        breaks: "shadow files: a block damaged before a commit block is taken for a crash, and the commit is lost",
        file: "src/disk/shadow/side.rs",
        original: "            && (committed || ends_committed(&blocks)?)",
        replacement: "            && committed",
        integration: &[],
        unit: &["disk::shadow::tests::a_block_damaged_before_a_commit_block_is_refused"],
    },
];
