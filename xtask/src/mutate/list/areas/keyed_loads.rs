//! The mutations of loads by column into tables of keys and indexes
//! (RULES.md R-TEST-9, D1235): a row not alone under a unique key noted,
//! each entry's alone told back in its row's place, a run of keys kept to
//! the leaf that covers them, a foreign key's missing parent refusing the
//! group, an open load's group pending to a key check, and recovery's
//! entries of a group taken back and made again.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail a keyed load by column.
pub const KEYED_LOADS: [Mutation; 7] = [
    Mutation {
        name: "keyed-note-not-alone",
        package: Package::Core,
        breaks: "a row of a load by column whose key was not alone is never checked, so a duplicate stands",
        file: "src/db/write/lanes.rs",
        original: "            if checked.needs_note(alone) {",
        replacement: "            if false && checked.needs_note(alone) {",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::a_keyed_load_goes_by_column_and_its_duplicates_fail"],
    },
    Mutation {
        name: "keyed-sorted-alone",
        package: Package::Core,
        breaks: "every entry a B-link tree takes in key order is told alone, so a duplicate stands",
        file: "src/index/blink/sorted.rs",
        original: "                *slot = was;",
        replacement: "                *slot = true;",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::a_keyed_load_goes_by_column_and_its_duplicates_fail"],
    },
    Mutation {
        name: "keyed-run-covered",
        package: Package::Core,
        breaks: "a run of keys goes on into a leaf past the keys it covers",
        file: "src/engine/tree/sorted.rs",
        original: "                if n > from && (!node.covers(key) || node.deleted) {",
        replacement: "                if n > from && node.deleted {",
        integration: &[],
        unit: &["engine::tree::tests::sorted::readers_find_every_key_while_runs_split_leaves"],
    },
    Mutation {
        name: "keyed-foreign-refused",
        package: Package::Core,
        breaks: "a foreign key that finds no parent for a load's row lets the group in",
        file: "src/db/integrity/inserted.rs",
        original: "                    Err(e) if e.code == Code::Constraint => return Ok(false),",
        replacement: "                    Err(e) if e.code == Code::Constraint => continue,",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::a_load_refused_part_way_leaves_nothing_behind"],
    },
    Mutation {
        name: "keyed-open-group-pending",
        package: Package::Core,
        breaks: "a key check takes a row of another transaction's open load as committed",
        file: "src/engine/rows/frozen/stamped.rs",
        original: "            Some(Maker::Open(by)) if by != tx => Newest {",
        replacement: "            Some(Maker::Open(by)) if by != tx && by == 0 => Newest {",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::two_loads_of_overlapping_keys_never_both_stand"],
    },
    Mutation {
        name: "keyed-group-taken-back",
        package: Package::Core,
        breaks: "recovery keeps a checkpoint's entries of a group that does not stand",
        file: "src/db/recovery.rs",
        original: "            return self.take_back_group(change, extent, kept);",
        replacement: "            return Ok(());",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::a_checkpoint_within_a_load_leaves_no_entry_of_it_after_a_crash"],
    },
    Mutation {
        name: "keyed-group-mended",
        package: Package::Core,
        breaks: "recovery makes no entry of a group logged after the checkpoint",
        file: "src/db/recovery/groups.rs",
        original: "            index.insert_sorted(&lacked, false)?;",
        replacement: "            drop(lacked);",
        integration: &[],
        unit: &["db::write::bulk::keyed::tests::a_crash_at_each_point_of_a_group_leaves_its_indexes_whole"],
    },
];
