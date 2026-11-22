//! The mutations of what EXPLAIN ANALYZE shows beside an operator's rows
//! and of its data change statements (RULES.md R-TEST-9, D620 to D622): a
//! spill area not noted, a pass noted at every read, batches, leaf pages
//! or fetches not noted, a pull that leaves the thread's notes unset, a
//! data change's plan not counted or its access run uncounted, and a data
//! change refused after EXPLAIN ANALYZE.

use super::super::{Mutation, Package};

/// The test of what an operator spilled.
const SPILLED: &str = "explain_analyze::notes::operators_past_their_work_area_show_what_they_spilled";
/// The test of an index scan's reads.
const READS: &str = "explain_analyze::notes::an_index_scan_shows_its_reads";
/// The test of an UPDATE run by EXPLAIN ANALYZE.
const UPDATED: &str = "explain_analyze::changes::an_update_runs_in_the_transaction_and_shows_its_plan";

/// Mechanisms whose removal must fail the requirements of the forms.
pub const ANALYZE_MORE: [Mutation; 9] = [
    Mutation {
        name: "analyze-spill-unnoted",
        package: Package::Core,
        breaks: "a spill area made for a counted operator notes nothing",
        file: "src/spill.rs",
        original: "            Some(notes) => noted::noted(made, notes),",
        replacement: "            Some(_) => made,",
        integration: &[SPILLED],
        unit: &[],
    },
    Mutation {
        name: "analyze-pass-every-read",
        package: Package::Core,
        breaks: "a spill area notes a pass at every read, not only after writing",
        file: "src/spill/noted.rs",
        original: "        if self.writing.swap(false, Ordering::Relaxed) {",
        replacement: "        if self.writing.swap(false, Ordering::Relaxed) || true {",
        integration: &[],
        unit: &["spill::noted::tests::an_area_notes_what_it_spills_and_its_passes"],
    },
    Mutation {
        name: "analyze-batches-unnoted",
        package: Package::Core,
        breaks: "an operator run by batches notes none",
        file: "src/exec/counted.rs",
        original: "        if self.batches > 0 {",
        replacement: "        if self.batches > u64::MAX - 1 {",
        integration: &["explain_analyze::notes::a_scan_by_batches_shows_its_batches"],
        unit: &[],
    },
    Mutation {
        name: "analyze-leaves-unnoted",
        package: Package::Core,
        breaks: "a scan of a B-link tree notes no leaf it reads",
        file: "src/engine/tree/scan.rs",
        original: "        noted::add(self.notes.as_deref(), Noted::Leaves, 1);",
        replacement: "        noted::add(None, Noted::Leaves, 1);",
        integration: &[READS],
        unit: &[],
    },
    Mutation {
        name: "analyze-fetch-unnoted",
        package: Package::Core,
        breaks: "an index scan notes no row it fetches",
        file: "src/exec/entries.rs",
        original: "    counts[1] = counts[1].saturating_add(1);",
        replacement: "    counts[1] = counts[1].saturating_add(0);",
        integration: &[READS],
        unit: &[],
    },
    Mutation {
        name: "analyze-notes-unset",
        package: Package::Core,
        breaks: "a pull leaves the thread without its operator's notes",
        file: "src/noted.rs",
        original: "    let before = NOTING.try_with(|n| n.replace(notes)).ok().flatten();",
        replacement: "    let before = NOTING.try_with(|n| n.replace(notes.filter(|_| false))).ok().flatten();",
        integration: &[READS],
        unit: &["noted::tests::work_notes_of_the_innermost_operator_pulled"],
    },
    Mutation {
        name: "analyze-change-uncounted",
        package: Package::Core,
        breaks: "the plan of a data change EXPLAIN ANALYZE runs is not counted",
        file: "src/db/explain/changed.rs",
        original: "    if !changed.taken() {",
        replacement: "    if changed.taken() || true {",
        integration: &[UPDATED, "explain_analyze::changes::each_data_change_counts_its_rows_and_its_plan"],
        unit: &[],
    },
    Mutation {
        name: "analyze-change-access-uncounted",
        package: Package::Core,
        breaks: "the counted access of a data change's object rows runs uncounted",
        file: "src/exec/entries.rs",
        original: "        Plan::Counted(c) => super::counted::entries_through(*c, db, |input| table_entries(input, db, tx)),",
        replacement: "        Plan::Counted(c) => table_entries(c.input, db, tx),",
        integration: &[UPDATED],
        unit: &[],
    },
    Mutation {
        name: "analyze-change-refused",
        package: Package::Core,
        breaks: "a data change after EXPLAIN ANALYZE is refused",
        file: "src/parser/data_statements.rs",
        original: "    (analyze && (query || begins_change(third, fourth))).then_some((2, true))",
        replacement: "    (analyze && query).then_some((2, true))",
        integration: &[UPDATED],
        unit: &["parser::data_statements::tests::analyze_is_read_before_a_data_change"],
    },
];
