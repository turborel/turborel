//! The mutations of a long load kept within the process's limits
//! (RULES.md R-TEST-9, D1136 to D1139): a file opened with no descriptor
//! left, a checkpoint due by the log's bytes, a load's given rows held
//! unspilled, and a checkpoint's captures written to disk (D1139).

use super::super::{Mutation, Package};

/// Each mechanism of a load's limits broken alone, and the tests that fail.
pub const LOAD_LIMITS: [Mutation; 5] = [
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
];
