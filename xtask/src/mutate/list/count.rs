//! The mutations of the row store's count and its scans read in place
//! (RULES.md R-TEST-9, D350, D351), the zones counted by their flag, none
//! counted twice, the read noted, a table that freezes left to its scan,
//! what the scan reads in place and what after its leaf's latch, where a
//! visit goes on, the aggregations the executor counts so, and their
//! staying off the parallel path.

use super::super::{Mutation, Package};

/// The zone count's unit tests, and the integration test of its levels.
const COUNTED: &str = "engine::rows::count::tests::a_count_is_the_scans_for_every_reader";
const LEVELS: &str = "row_count::a_count_is_the_scans_at_every_isolation_level";
/// The scan in place's unit test.
const IN_PLACE: &str = "engine::rows::in_place::tests::batches_read_in_place_are_the_rows_each_reader_sees";
/// The tree's visit's unit test.
const VISITED: &str = "engine::tree::tests::a_visit_reads_what_a_scan_gives";

/// Mechanisms whose removal must fail the count's requirements.
pub const COUNT: [Mutation; 12] = [
    Mutation {
        name: "count-zone-flag",
        package: Package::Core,
        breaks: "a zone's rows are counted whether or not every reader sees them alone",
        file: "src/engine/rows/zones/visible.rs",
        original: "            if slot.visible.load(AtomicOrdering::Acquire) {",
        replacement: "            if zone.known {",
        integration: &[LEVELS],
        unit: &[COUNTED],
    },
    Mutation {
        name: "count-zone-twice",
        package: Package::Core,
        breaks: "a zone counted from its rows is read again with the rest",
        file: "src/engine/rows/zones/visible.rs",
        original: "                rows = rows.saturating_add(u64::from(zone.rows));",
        replacement: "                rows = rows.saturating_add(u64::from(zone.rows));\n                joined(&mut ranges, at);",
        integration: &[LEVELS],
        unit: &[COUNTED],
    },
    Mutation {
        name: "count-note-read",
        package: Package::Core,
        breaks: "a count is not noted as a read of the whole table under SERIALIZABLE",
        file: "src/engine/rows/count.rs",
        original: "        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, || Read::All(EVERY_COLUMN)).map(|()| reader)) {",
        replacement: "        let reader = match self.reader(tx) {",
        integration: &[LEVELS],
        unit: &["engine::rows::count::tests::a_count_read_conflicts_with_an_insert_as_a_scan_does"],
    },
    Mutation {
        name: "count-freezing",
        package: Package::Core,
        breaks: "a table that freezes rows is counted from its zones",
        file: "src/engine/rows/count.rs",
        original: "        if freezes || !self.frozen.directory()",
        replacement: "        if !self.frozen.directory()",
        integration: &[],
        unit: &["engine::rows::count::tests::a_count_of_a_table_opened_reads_until_settled"],
    },
    Mutation {
        name: "in-place-sees",
        package: Package::Core,
        breaks: "a version the reader does not see is read in place",
        file: "src/engine/rows/in_place.rs",
        original: "        if !reader.sees(version.maker) {",
        replacement: "        if version.deleted && !reader.sees(version.maker) {",
        integration: &[LEVELS],
        unit: &[IN_PLACE, COUNTED],
    },
    Mutation {
        name: "in-place-sealed",
        package: Package::Core,
        breaks: "a sealed record is read in place, not opened by its encodings",
        file: "src/engine/rows/in_place.rs",
        original: "(sealed(version.record) || !gathering.inline(&self.format, version.record)?)",
        replacement: "!gathering.inline(&self.format, version.record)?",
        integration: &[LEVELS],
        unit: &[],
    },
    Mutation {
        name: "in-place-inline",
        package: Package::Core,
        breaks: "a record with a column asked for out of line is read in place",
        file: "src/engine/rows/in_place.rs",
        original: "(sealed(version.record) || !gathering.inline(&self.format, version.record)?)",
        replacement: "sealed(version.record)",
        integration: &[],
        unit: &[IN_PLACE],
    },
    Mutation {
        name: "visit-past-later",
        package: Package::Core,
        breaks: "a visit does not go on past an entry it gave back copied",
        file: "src/engine/tree/scan/visit.rs",
        original: "                taken = Some(key);\n",
        replacement: "",
        integration: &[],
        unit: &[VISITED],
    },
    Mutation {
        name: "visit-same-leaf",
        package: Package::Core,
        breaks: "a visit stopped inside a leaf goes on at the next leaf",
        file: "src/engine/tree/scan/visit.rs",
        original: "} else { Next::Leaf(leaf) };",
        replacement: "} else { shown.further.map_or(Next::Done, Next::Leaf) };",
        integration: &[],
        unit: &[VISITED, IN_PLACE],
    },
    Mutation {
        name: "count-all-counting",
        package: Package::Core,
        breaks: "an aggregation with a set function of a column beside COUNT(*) is counted by the storage",
        file: "src/exec/batch/count.rs",
        original: "!read.taken.iter().all(|t| matches!(t, Taken::Count))",
        replacement: "!read.taken.iter().any(|t| matches!(t, Taken::Count))",
        integration: &[],
        unit: &["exec::batch::count::tests::only_a_bare_count_is_counted_by_the_storage"],
    },
    Mutation {
        name: "count-located",
        package: Package::Core,
        breaks: "a located scan is counted by the storage",
        file: "src/exec/batch/count.rs",
        original: "let Plan::SeqScan { table, order, located: false, .. } = input",
        replacement: "let Plan::SeqScan { table, order, .. } = input",
        integration: &[],
        unit: &["exec::batch::count::tests::only_a_bare_count_is_counted_by_the_storage"],
    },
    Mutation {
        name: "count-parallel",
        package: Package::Core,
        breaks: "a bare COUNT(*) the storage counts is planned in parallel, its scan split into morsels",
        file: "src/physical/parallel.rs",
        original: "    if depth >= MAX_DEPTH || crate::exec::storage_counted(plan, asked.db).is_some() {",
        replacement: "    if depth >= MAX_DEPTH {",
        integration: &["row_count::explain_notes_a_count_by_the_storage"],
        unit: &[],
    },
];
