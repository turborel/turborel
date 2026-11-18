//! The mutations of shadow files (RULES.md R-TEST-9): a granule held once
//! in its side file, what recovery takes a torn block for, the size below
//! which a granule is read from the file; and the block files a crash left
//! half made, side files and catalog files among them, thrown away.

use super::{Mutation, Package};

/// The mutations of shadow files, in the order they run.
pub const SHADOW: [Mutation; 7] = [
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
    Mutation {
        name: "shadow-committed-bound",
        package: Package::Core,
        breaks: "shadow files: a granule in the upper half of the file is taken for zeros, so writing part of it loses the rest",
        file: "src/disk/shadow/file.rs",
        original: "pending.committed.is_some_and(|size| start < size)",
        replacement: "pending.committed.is_some_and(|size| start < size / 2)",
        integration: &[],
        unit: &["disk::shadow::tests::a_granule_written_in_part_keeps_the_rest_of_the_file"],
    },
    Mutation {
        name: "half-made-shorter-than-header",
        package: Package::Core,
        breaks: "a block file a crash left shorter than its header is refused as damaged, and the database cannot be opened",
        file: "src/disk/block/tail.rs",
        original: "    Ok(fs.open(path, OpenMode::ReadOnly)?.size()? < format::HEADER_LEN)",
        replacement: "    Ok(fs.open(path, OpenMode::ReadOnly)?.size()? > format::HEADER_LEN)",
        integration: &["log::a_crashed_file_log_recovers"],
        unit: &[
            "disk::shadow::tests::a_side_file_a_crash_left_half_made_is_thrown_away",
            "db::catalog_file::tests::a_file_the_switch_left_half_made_is_removed",
        ],
    },
    Mutation {
        name: "side-half-made-thrown-away",
        package: Package::Core,
        breaks: "shadow files: a side file a crash left half made is refused, not thrown away",
        file: "src/disk/shadow/side.rs",
        original: "                let half = half_made(&*at.fs, &side, &e)?;",
        replacement: "                let half = half_made(&*at.fs, &side, &e)? && false;",
        integration: &[],
        unit: &["disk::shadow::tests::a_side_file_a_crash_left_half_made_is_thrown_away"],
    },
    Mutation {
        name: "catalog-half-made-removed",
        package: Package::Core,
        breaks: "the catalog: a file the switch left half made is refused, not removed",
        file: "src/db/catalog_file.rs",
        original: "                    let half = half_made(&*fs, &path, &e)?;",
        replacement: "                    let half = half_made(&*fs, &path, &e)? && false;",
        integration: &[],
        unit: &["db::catalog_file::tests::a_file_the_switch_left_half_made_is_removed"],
    },
];
