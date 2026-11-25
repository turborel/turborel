//! The mutations of the row store's count and its scans read in place
//! (RULES.md R-TEST-9, D350, D351), the zones counted by their flag, none
//! counted twice, the read noted, a table that holds groups counted from
//! its directory (D1180), the rows the tree shadows and past the groups,
//! what the scan reads in place and what after its leaf's latch, where a
//! visit goes on, the aggregations the executor counts so, and their
//! staying off the parallel path.

use super::super::{Mutation, Package};

/// The zone count's unit tests, and the integration test of its levels.
const COUNTED: &str = "engine::rows::count::tests::a_count_is_the_scans_for_every_reader";
const LEVELS: &str = "row_count::a_count_is_the_scans_at_every_isolation_level";
/// The grouped count's unit test, and the integration test of its levels
/// (RULES.md D1180).
const GROUPED: &str = "engine::rows::frozen::tests::counted::a_grouped_count_is_the_scans_for_every_reader";
const GROUPED_LEVELS: &str = "row_count::a_grouped_count_is_the_scans_at_every_isolation_level";
/// The summaries' integration tests (RULES.md D1181).
const SUMMARIZED: &str = "summarized::min_and_max_by_summaries_are_the_scans";
const SUMMARIZED_PLAN: &str = "summarized::explain_notes_min_and_max_by_summaries";
/// The scan in place's unit test.
const IN_PLACE: &str = "engine::rows::in_place::tests::batches_read_in_place_are_the_rows_each_reader_sees";
/// The tree's visit's unit test.
const VISITED: &str = "engine::tree::tests::a_visit_reads_what_a_scan_gives";

/// Mechanisms whose removal must fail the count's requirements.
pub const COUNT: [Mutation; 19] = [
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
        name: "count-grouped-zones",
        package: Package::Core,
        breaks: "a table that holds groups is counted from its zones",
        file: "src/engine/rows/count.rs",
        original: "if table.holds_groups() { table.grouped_counts(tx) } else { table.counts(tx) }",
        replacement: "table.counts(tx)",
        integration: &[GROUPED_LEVELS],
        unit: &[GROUPED],
    },
    Mutation {
        name: "count-grouped-shadowed",
        package: Package::Core,
        breaks: "a group's rows the tree's entries stand for are counted as the group holds them",
        file: "src/engine/rows/frozen/counts.rs",
        original: "Ok(rows.saturating_sub(as_count(replaced)).saturating_add(seen))",
        replacement: "Ok(rows.saturating_add(seen).saturating_sub(as_count(replaced.min(0))))",
        integration: &[GROUPED_LEVELS],
        unit: &[GROUPED],
    },
    Mutation {
        name: "count-grouped-outside",
        package: Package::Core,
        breaks: "the tree's entries in a group's range are counted again with the rows past the groups",
        file: "src/engine/rows/frozen/counts.rs",
        original: "        from = entry.summary.last.saturating_add(1);\n",
        replacement: "",
        integration: &[],
        unit: &["engine::rows::frozen::tests::counted::a_grouped_count_is_the_scans_for_every_reader"],
    },
    Mutation {
        name: "count-grouped-note-read",
        package: Package::Core,
        breaks: "a count of a table that holds groups is not noted as a read of the whole table",
        file: "src/engine/rows/frozen/counts.rs",
        original: "self.reader(tx).and_then(|reader| self.note_read(tx, || Read::All(EVERY_COLUMN)).map(|()| reader));",
        replacement: "self.reader(tx);",
        integration: &[GROUPED_LEVELS],
        unit: &["engine::rows::frozen::tests::counted::a_grouped_count_conflicts_with_an_insert_as_a_scan_does"],
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
        original: "crate::exec::storage_counted(plan, asked.db).or_else(",
        replacement: "None.or_else(",
        integration: &["row_count::explain_notes_a_count_by_the_storage"],
        unit: &[],
    },
    Mutation {
        name: "summarized-shadowed-whole",
        package: Package::Core,
        breaks: "a group the tree's entries shadow is given whole by its summary",
        file: "src/engine/rows/frozen/summarized.rs",
        original: ".partition(|e| self.shadow_ranges(e).is_empty());",
        replacement: ".partition(|e| e.summary.rows > 0);",
        integration: &[SUMMARIZED],
        unit: &["engine::rows::frozen::tests::counted::a_group_no_entry_shadows_is_given_whole"],
    },
    Mutation {
        name: "summarized-least",
        package: Package::Core,
        breaks: "MIN is fed a whole group's greatest value and MAX its least",
        file: "src/exec/batch/count.rs",
        original: "state.update(if least_of { least } else { greatest })?;",
        replacement: "state.update(if least_of { greatest } else { least })?;",
        integration: &[SUMMARIZED],
        unit: &[],
    },
    Mutation {
        name: "summarized-types",
        package: Package::Core,
        breaks: "MIN and MAX of a text are taken from summaries, compared as values, not by the text's collation",
        file: "src/exec/batch/count.rs",
        original: "let ordered = matches!(a.result, DataType::Exact { .. } | DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. });",
        replacement: "let ordered = !matches!(a.result, DataType::Boolean);",
        integration: &[SUMMARIZED_PLAN],
        unit: &[],
    },
    Mutation {
        name: "summarized-parallel",
        package: Package::Core,
        breaks: "MIN and MAX summaries answer are planned in parallel, never summarized",
        file: "src/physical/parallel.rs",
        original: ".or_else(|| crate::exec::storage_summarized(plan, asked.db))",
        replacement: "",
        integration: &[SUMMARIZED_PLAN],
        unit: &[],
    },
];
