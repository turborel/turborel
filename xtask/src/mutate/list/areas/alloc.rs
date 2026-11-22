//! The mutations of the allocations taken out of the row store's reads
//! and writes (RULES.md R-TEST-9): an undo batch's versions shared where
//! its record holds them, the batches read kept, a version narrowed to its
//! record, a record's columns decoded as asked and written in place, a
//! file log's record taken out of its reader, character strings compared
//! byte by byte, padded, zones kept due resting after a pass over every
//! zone, an update's log record written in place, and a scan's batch
//! given room for its rows at once.

use super::super::{Mutation, Package};

/// The undo store.
const UNDO_FILE: &str = "src/engine/rows/undo.rs";
/// Character string comparison.
const COMPARISON_FILE: &str = "src/value/comparison.rs";
/// The unit test of batches read back.
const READ_BACK: &str = "engine::rows::undo::tests::batched_versions_read_back_as_appended";
/// The test of an update's images in its log record.
const UPDATE_IMAGES: &str = "log::record::images::tests::an_update_logs_the_columns_it_changed";
/// The unit test of padded comparison.
const PADDED: &str = "value::comparison::strings::shorter_string_padded_with_spaces";

/// Mechanisms of the row store's reads without copies whose removal must fail their tests.
pub const ALLOC: [Mutation; 11] = [
    Mutation {
        name: "alloc-undo-span-length",
        package: Package::Core,
        breaks: "a version of an undo batch read from the log is taken with its length's bytes before it",
        file: UNDO_FILE,
        original: "        spans.push((end.saturating_sub(version.len()), end));",
        replacement: "        spans.push((end.saturating_sub(version.len()).saturating_sub(LENGTH_BYTES), end));",
        integration: &[],
        unit: &[READ_BACK, "engine::rows::undo::tests::batches_read_are_kept_the_last_few"],
    },
    Mutation {
        name: "alloc-undo-one-batch-kept",
        package: Package::Core,
        breaks: "the undo store keeps only the batch last read from the log, so a reader of a few read each again and again",
        file: UNDO_FILE,
        original: "    if cached.len() >= CACHED {",
        replacement: "    if !cached.is_empty() {",
        integration: &[],
        unit: &["engine::rows::undo::tests::batches_read_are_kept_the_last_few"],
    },
    Mutation {
        name: "alloc-seen-not-narrowed",
        package: Package::Core,
        breaks: "the record of an older version a reader sees is given with the version's header before it",
        file: "src/engine/rows/mvcc.rs",
        original: "                    return Ok((!version.deleted).then(|| bytes.last(record)));",
        replacement: "                    return Ok((!version.deleted).then_some(bytes).filter(|_| record > 0));",
        integration: &[],
        unit: &["engine::rows::tests::an_undone_change_keeps_its_older_version_for_open_readers"],
    },
    Mutation {
        name: "alloc-decode-every-column",
        package: Package::Core,
        breaks: "a record's columns not asked for are decoded too, not NULL",
        file: "src/engine/record.rs",
        original: "        let Some(columns) = columns else {",
        replacement: "        let Some(columns) = columns.filter(|_| false) else {",
        integration: &[],
        unit: &["engine::record::tests::rows_read_back_as_written"],
    },
    Mutation {
        name: "alloc-file-log-record-left",
        package: Package::Core,
        breaks: "a file log's record read by its position is not taken out of its reader, read as empty",
        file: "src/log/file.rs",
        original: ".filter(|&found| found == at).map(|_| reader.taken()))",
        replacement: ".filter(|&found| found == at).map(|_| Vec::new()))",
        integration: &["log::the_file_log_keeps_the_contract"],
        unit: &[],
    },
    Mutation {
        name: "alloc-compare-rest-unreversed",
        package: Package::Core,
        breaks: "a longer right string's rest is compared with the padding as if it were the left's",
        file: COMPARISON_FILE,
        original: ".then_with(|| against_spaces(right_rest).reverse())",
        replacement: ".then_with(|| against_spaces(right_rest))",
        integration: &[],
        unit: &[PADDED, "value::comparison::strings::bytes_compare_in_scalar_order"],
    },
    Mutation {
        name: "alloc-compare-rest-above-spaces",
        package: Package::Core,
        breaks: "a character after the common part is taken as above the padding though it is below a space",
        file: COMPARISON_FILE,
        original: "map_or(Ordering::Equal, |b| b.cmp(&SPACE))",
        replacement: "map_or(Ordering::Equal, |_| Ordering::Greater)",
        integration: &[],
        unit: &[PADDED, "value::comparison::strings::bytes_compare_in_scalar_order"],
    },
    Mutation {
        name: "alloc-zones-swept-no-rest",
        package: Package::Core,
        breaks: "zones writes keep due never rest while a call finds as many due as it sums up, so the maintenance thread sums them up over and over",
        file: "src/engine/rows/zones/summed.rs",
        original: "        if self.swept(looked, map.zones.len()) || due.len() < MOST_SUMMED {",
        replacement: "        if due.len() < MOST_SUMMED {",
        integration: &[],
        unit: &["engine::rows::zones::rested::zones_kept_due_rest_after_a_pass_over_every_zone"],
    },
    Mutation {
        name: "alloc-record-failed-left",
        package: Package::Core,
        breaks: "a record refused part way is left in the buffer it was written into",
        file: "src/engine/record.rs",
        original: "                out.truncate(base);\n",
        replacement: "",
        integration: &[],
        unit: &["engine::record::tests::records_are_appended_and_a_failed_one_leaves_none"],
    },
    Mutation {
        name: "alloc-update-image-uncounted",
        package: Package::Core,
        breaks: "an update's record tells none of the columns of its row before it wrote",
        file: "src/log/record/images.rs",
        original: "        place.copy_from_slice(&count.to_le_bytes());",
        replacement: "        place.copy_from_slice(&0_u32.to_le_bytes());",
        integration: &[],
        unit: &[UPDATE_IMAGES],
    },
    Mutation {
        name: "alloc-batch-unreserved",
        package: Package::Core,
        breaks: "a scan's batch grows its integers from none by doubling as it gathers them",
        file: "src/engine/rows/gathered.rs",
        original: "                Gathered::Ints { units, .. } => units.reserve(more),",
        replacement: "                Gathered::Ints { .. } => {}",
        integration: &[],
        unit: &["engine::rows::gathered::tests::a_batch_has_room_for_its_rows_at_once"],
    },
];
