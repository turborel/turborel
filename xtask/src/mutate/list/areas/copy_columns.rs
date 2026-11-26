//! The mutations of a copy by column and an import over several
//! connections (RULES.md R-TEST-9, D1148): CopyColumns refused without
//! `columns` granted, a column's null bits checked to be one for each
//! row, a null cell's bit set, a file's ranges begun at line starts, a
//! range that would begin within a quoted field found, Ctrl-C cancelling
//! each job, and a password typed proving each job.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail a copy by column or an import by
/// jobs.
pub const COPY_COLUMNS: [Mutation; 8] = [
    Mutation {
        name: "copy-columns-gate",
        package: Package::Server,
        breaks: "a CopyColumns is taken from a client granted `copy` but not `columns`",
        file: "server/src/native/next.rs",
        original: "        Request::CopyColumns(_) => [Some(COPY), Some(COLUMNS)],",
        replacement: "        Request::CopyColumns(_) => one(COPY),",
        integration: &[],
        unit: &["native::next::tests::copy_columns_needs_columns_and_copy"],
    },
    Mutation {
        name: "copy-columns-null-bytes-checked",
        package: Package::Client,
        breaks: "a CopyColumns whose column has too few or too many bytes of null bits is taken",
        file: "protocol/src/request/columns.rs",
        original: "    if nulls.len() != rows.div_ceil(BITS) || !spare_clear(&nulls, rows) {",
        replacement: "    if !spare_clear(&nulls, rows) {",
        integration: &[],
        unit: &["import::cells::tests::cells_go_as_a_copy_columns_the_server_takes"],
    },
    Mutation {
        name: "copy-columns-null-bit",
        package: Package::Client,
        breaks: "a null field split into cells is sent as the empty string",
        file: "client/src/import/cells.rs",
        original: "                *byte |= bit;",
        replacement: "                *byte |= 0;",
        integration: &[],
        unit: &["import::delimited::tests::cells_hold_the_fields_a_record_does"],
    },
    Mutation {
        name: "jobs-ranges-at-line-starts",
        package: Package::Cli,
        breaks: "a file's range for a job begins at its share of the bytes, within a line",
        file: "cli/src/run/ranges.rs",
        original: "            Some(start) if start < len => starts.push(start),",
        replacement: "            Some(start) if start < len => starts.push(target(first, len, (k, jobs)).max(last)),",
        integration: &[],
        unit: &["run::ranges::tests::ranges_begin_at_lines_and_cover_the_file_once"],
    },
    Mutation {
        name: "jobs-straddle-found",
        package: Package::Cli,
        breaks: "a range that begins within a quoted field, after a line break it holds, is imported",
        file: "cli/src/run/ranges.rs",
        original: "            odd ^= q % 2 == 1;",
        replacement: "            odd ^= q % 2 == 2;",
        integration: &[],
        unit: &["run::ranges::tests::a_record_split_between_jobs_is_found"],
    },
    Mutation {
        name: "jobs-ctrl-c-cancels-each",
        package: Package::Cli,
        breaks: "Ctrl-C cancels the statement of the first of several connections alone",
        file: "cli/src/interrupt.rs",
        original: "        for e in by.iter().filter_map(|c| c.cancel().err()) {",
        replacement: "        for e in by.iter().take(1).filter_map(|c| c.cancel().err()) {",
        integration: &[],
        unit: &["interrupt::tests::ctrl_c_cancels_once_connected"],
    },
    Mutation {
        name: "jobs-ctrl-c-stops",
        package: Package::Cli,
        breaks: "a job of an import between statements at Ctrl-C reads on",
        file: "cli/src/run/jobs.rs",
        original: "        if self.pressed.is_some_and(|(interrupts, before)| interrupts.presses() != before) {",
        replacement: "        if false {",
        integration: &[],
        unit: &["run::jobs::tests::a_job_stops_at_ctrl_c_or_another_failing"],
    },
    Mutation {
        name: "jobs-password-kept",
        package: Package::Cli,
        breaks: "each job of an import asks for the password typed for the first connection again",
        file: "cli/src/dial.rs",
        original: "            Some(typed) => dialed(&self.options, &self.options.server, None, || Ok(typed)),",
        replacement: "            Some(_) => self.connect(None, None),",
        integration: &["imports::a_password_typed_proves_each_job"],
        unit: &[],
    },
];
