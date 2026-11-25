//! The mutations of a long load kept within the process's limits
//! (RULES.md R-TEST-9, D1136 to D1143): a file opened with no descriptor
//! left, a checkpoint due by the log's bytes, a load's given rows held
//! unspilled, a checkpoint's captures written to disk (D1139), a
//! value out of line freed only once no read may fetch it (D1140), a
//! long log record written after what the log's buffer holds (D1141), and
//! loads at once each writing their own groups (D1142), and a clean
//! close keeping the groups of loads committed (D1143).

use super::super::{Mutation, Package};

/// Each mechanism of a load's limits broken alone, and the tests that fail.
pub const LOAD_LIMITS: [Mutation; 11] = [
    Mutation {
        name: "files-spent-retried",
        package: Package::Core,
        breaks: "a file the process has no descriptor left for fails at once, though the database holds others it could close",
        file: "src/files/spent.rs",
        original: "            if !spent || self.open.load(Ordering::Acquire) <= own {",
        replacement: "            if true {",
        integration: &[],
        unit: &["files::tests::a_process_out_of_descriptors_closes_a_file_of_its_own"],
    },
    Mutation {
        name: "files-spent-told",
        package: Package::Core,
        breaks: "an open refused for want of a descriptor is an I/O error the file manager does not answer",
        file: "src/disk/io.rs",
        original: "    if e.raw_os_error().is_some_and(|code| spent.contains(&code)) {",
        replacement: "    if false {",
        integration: &[],
        unit: &["files::tests::a_process_out_of_descriptors_closes_a_file_of_its_own"],
    },
    Mutation {
        name: "checkpoint-log-bytes",
        package: Package::Core,
        breaks: "CHECKPOINT.AFTER_BYTES counts the side file alone, so the log grows without a checkpoint",
        file: "src/db/checkpoint.rs",
        original: "        let written = files.shadows.pending_bytes().max(current.tx.bytes_since_checkpoint());",
        replacement: "        let written = files.shadows.pending_bytes();",
        integration: &["load_limits::a_log_grown_past_after_bytes_brings_a_checkpoint"],
        unit: &[],
    },
    Mutation {
        name: "load-given-held",
        package: Package::Core,
        breaks: "the rows an execution of a load is given spill past a work area, each batch through a spill file",
        file: "src/db/write/given.rs",
        original: "    let mut rows = HeldRows::new(ListSpace { limit: usize::MAX, areas: None, ..st.list_space() });",
        replacement: "    let mut rows = HeldRows::new(st.list_space());",
        integration: &["load_limits::a_load_batch_past_work_memory_opens_no_spill_file"],
        unit: &[],
    },
    Mutation {
        name: "captures-on-disk",
        package: Package::Core,
        breaks: "a checkpoint's captures are held in memory until its commit, a copy of the pool's dirty pages",
        file: "src/disk/shadow/file.rs",
        original: "        if sealed.side.append_captured(&self.0.at, first, data.to_vec()).is_err() {",
        replacement: "        if true {",
        integration: &[],
        unit: &["disk::buffer::pool::capture::tests::captures_go_to_the_side_file"],
    },
    Mutation {
        name: "frozen-values-held",
        package: Package::Core,
        breaks: "a freeze frees the values out of line of the entries it takes out while a read that copied one still holds the directory",
        file: "src/engine/rows/frozen/freeze.rs",
        original: "            let unread = freeing.iter().take_while(|f| f.after.strong_count() == 0).count();",
        replacement: "            let unread = freeing.len();",
        integration: &[],
        unit: &["engine::rows::frozen::tests::retired::a_scan_fetches_its_value_after_the_freeze"],
    },
    Mutation {
        name: "frozen-read-pinned",
        package: Package::Core,
        breaks: "a read by row id holds no directory, so a freeze frees the value of the entry it copied",
        file: "src/engine/rows/frozen.rs",
        original: "        if self.plain() { None } else { self.directory().ok() }",
        replacement: "        if true { None } else { self.directory().ok() }",
        integration: &[],
        unit: &["engine::rows::frozen::tests::retired::a_read_by_row_id_fetches_its_value_after_the_freeze"],
    },
    Mutation {
        name: "log-long-record-ordered",
        package: Package::Core,
        breaks: "a log record as long as the buffer goes to the file ahead of a segment header the buffer still holds",
        file: "src/log/file/buffered.rs",
        original: "            if data.iter().any(|&b| b != 0) {",
        replacement: "            if false {",
        integration: &[],
        unit: &["log::file::buffered::tests::a_long_record_goes_to_the_file_after_what_is_held"],
    },
    Mutation {
        name: "groups-loads-together",
        package: Package::Core,
        breaks: "a load into groups takes the table's lock alone, so a second load waits for the first to end",
        file: "src/db/write/bulk/load.rs",
        original: "    if !together.indexes.iter().any(Indexed::loads) {",
        replacement: "    if false {",
        integration: &["bulk_groups::loads_at_once_each_write_their_own_groups"],
        unit: &[],
    },
    Mutation {
        name: "groups-ids-apart",
        package: Package::Core,
        breaks: "a group's row ids are counted on from one id, so they run into ids given to other rows",
        file: "src/engine/rows/frozen/stamped.rs",
        original: "        let first = self.next_ids(rows.len())?;",
        replacement: "        let first = self.next_id()?;",
        integration: &["bulk_groups::loads_at_once_each_write_their_own_groups"],
        unit: &[],
    },
    Mutation {
        name: "groups-kept-closed",
        package: Package::Core,
        breaks: "a database closed cleanly reopens holding apart the groups of loads stored before they settled, their rows lost",
        file: "src/db/files.rs",
        original: "            self.settle_closed_loads(mode)?;",
        replacement: "",
        integration: &["bulk_durable::a_loads_committed_rows_outlive_the_end_of_the_database"],
        unit: &[],
    },
];
