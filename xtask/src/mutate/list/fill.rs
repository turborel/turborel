//! The mutations of zeros filled ahead of a log's records (RULES.md
//! R-TEST-9): the refill before an append, the fill kept on open, and its
//! cut when a segment is sealed.

use super::{Mutation, Package};

/// The mutations of zeros filled ahead, in the order they run.
pub const FILL: [Mutation; 3] = [
    Mutation {
        name: "fill-ahead",
        package: Package::Core,
        breaks: "zeros filled ahead: a log's segment grows by each append, so each commit's sync carries its growth",
        file: "src/disk/block/fill.rs",
        original: "        if ahead == 0 || self.ahead_of(end) {",
        replacement: "        if ahead > 0 {",
        integration: &["blocks::a_file_filled_ahead_keeps_zeros_past_its_blocks", "log::segments_keep_zeros_ahead_until_sealed"],
        unit: &[],
    },
    Mutation {
        name: "fill-kept-on-open",
        package: Package::Core,
        breaks: "zeros filled ahead: opening takes them for a torn write, cutting and reporting them",
        file: "src/disk/block/tail.rs",
        original: "        let written = if options.ahead > 0 { blocks.written_end(end, size)? } else { size };",
        replacement: "        let written = size;",
        integration: &[
            "blocks::a_filled_file_reopens_with_its_zeros_kept_and_a_torn_write_cut",
            "recovery::a_damaged_log_is_refused_and_a_torn_one_reported",
        ],
        unit: &[],
    },
    Mutation {
        name: "fill-cut-at-seal",
        package: Package::Core,
        breaks: "zeros filled ahead: a sealed segment keeps them, so its size is not where its records end",
        file: "src/log/file/segments.rs",
        original: "        segment.blocks.trim().map_err(|e| self.failed(e, &format!(\"segment {}\", segment.number)))?;\n",
        replacement: "",
        integration: &["log::segments_keep_zeros_ahead_until_sealed"],
        unit: &[],
    },
];
