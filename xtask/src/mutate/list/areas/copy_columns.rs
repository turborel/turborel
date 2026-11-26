//! The mutations of a copy by column and an import over several
//! connections (RULES.md R-TEST-9, D1148): CopyColumns refused without
//! `columns` granted, a column's null bits checked to be one for each
//! row, a null cell's bit set, a file's ranges begun at line starts, and
//! a range that would begin within a quoted field found.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail a copy by column or an import by
/// jobs.
pub const COPY_COLUMNS: [Mutation; 5] = [
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
        original: "        starts.push(Range { start: scan.at, end: len, lines: scan.lines, records });",
        replacement: "        starts.push(Range { start: from, end: len, lines: scan.lines, records });",
        integration: &[],
        unit: &["run::ranges::tests::ranges_begin_at_lines_and_cover_the_file_once"],
    },
    Mutation {
        name: "jobs-straddle-found",
        package: Package::Cli,
        breaks: "a range that begins within a quoted field, after a line break it holds, is imported",
        file: "cli/src/run/ranges.rs",
        original: "        if scan.quoted {",
        replacement: "        if false {",
        integration: &[],
        unit: &["run::ranges::tests::a_record_split_between_jobs_is_found"],
    },
];
