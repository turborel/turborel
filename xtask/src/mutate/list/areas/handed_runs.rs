//! The mutations of a morsel's runs of one key handed on an entry each
//! (RULES.md R-TEST-9, D1325): what the morsels' tables tell of their
//! lookups, which batches are held by their runs, how an entry's first
//! row is numbered, and the states an entry is given of its run's rows.

use super::super::{Mutation, Package};

/// The file the waves' morsels are grouped and judged in.
const WAVES: &str = "src/exec/parallel/grouped/waves.rs";

/// The file a morsel's rows are grouped in.
const MORSEL: &str = "src/exec/batch/groups/morsel.rs";

/// The file a morsel's runs are held in.
const RUNS: &str = "src/exec/table/made/apart/runs.rs";

/// The file the states of runs are set in.
const STATES: &str = "src/exec/table/states/typed/apart/runs.rs";

/// Morsels hand on their runs after a table that found no group.
const HAND_ON: &str =
    "exec::parallel::grouped::waves::tests::runs::morsels_hand_on_their_runs_after_a_table_that_found_no_group_for_its_lookups";

/// Runs handed on are the runs appended and fed.
const APPENDED: &str = "exec::table::made::apart::tests::runs::runs_handed_on_are_the_runs_appended_and_fed";

/// An entry is numbered and fed among all the morsel's rows.
const NUMBERED: &str = "exec::table::made::apart::tests::runs::an_entry_is_numbered_and_fed_among_all_the_morsels_rows";

/// A batch is held by its runs only where they cover its rows.
const COVERED: &str = "exec::table::made::apart::tests::runs::a_batch_is_held_by_its_runs_only_where_they_cover_its_rows";

/// States set as their runs say whether a null was passed over.
const NULLS: &str = "exec::table::made::apart::tests::runs::states_set_as_their_runs_say_whether_a_null_was_passed_over";

/// A run whose sum leaves 64 bits is fed.
const WIDE: &str = "exec::table::made::apart::tests::runs::a_run_whose_sum_leaves_64_bits_is_fed";

/// Mechanisms whose removal must fail the requirements of runs handed
/// on an entry each.
pub const HANDED_RUNS: [Mutation; 14] = [
    Mutation {
        name: "morsel-table-judged-by-the-rows-it-took",
        package: Package::Core,
        breaks: "a morsel's table is judged by the rows it took, so one that finds no group for a row it looks up is still made (RULES.md D1325)",
        file: WAVES,
        original: "reduction.tell(m, (table.looked, table.len(), table.taken));",
        replacement: "reduction.tell(m, (table.taken, table.len(), table.taken));",
        integration: &[],
        unit: &[HAND_ON],
    },
    Mutation {
        name: "runs-a-table-followed-forgotten",
        package: Package::Core,
        breaks: "the morsels handed on after a table that followed runs look for none, so each row is an entry (RULES.md D1325)",
        file: WAVES,
        original: "self.runs.store(looked < rows, Ordering::Relaxed);",
        replacement: "self.runs.store(false, Ordering::Relaxed);",
        integration: &[],
        unit: &[HAND_ON],
    },
    Mutation {
        name: "entries-of-runs-handed-whole",
        package: Package::Core,
        breaks: "a morsel that hands its runs on is frozen whole where its entries are few beside its rows (RULES.md D1325)",
        file: WAVES,
        original: "let few = apart.is_none() && few(",
        replacement: "let few = few(",
        integration: &[],
        unit: &[HAND_ON],
    },
    Mutation {
        name: "lookups-counted-as-the-rows-of-their-runs",
        package: Package::Core,
        breaks: "a table that follows runs counts every row as looked up, so it is judged by its rows (RULES.md D1325)",
        file: MORSEL,
        original: "table.looked = table.looked.saturating_add(ends.len());",
        replacement: "table.looked = table.looked.saturating_add(count);",
        integration: &[],
        unit: &[HAND_ON],
    },
    Mutation {
        name: "runs-never-looked-for-in-a-morsel-handed-on",
        package: Package::Core,
        breaks: "a morsel handed on looks for no run though the tables followed runs, so each row is an entry (RULES.md D1325)",
        file: MORSEL,
        original: "if runs { super::run_ends(batch, at, rows) } else { None }",
        replacement: "if runs && batch.rows == 0 { super::run_ends(batch, at, rows) } else { None }",
        integration: &[],
        unit: &[HAND_ON],
    },
    Mutation {
        name: "batch-held-by-runs-short-of-its-rows",
        package: Package::Core,
        breaks: "a batch told runs that end before its last row is held by them, and the rows after them are lost (RULES.md D1325)",
        file: RUNS,
        original: "ends.len() < count && ends.last() == Some(&count)",
        replacement: "ends.len() < count",
        integration: &[],
        unit: &[COVERED],
    },
    Mutation {
        name: "first-rows-of-runs-read-past-a-filter",
        package: Package::Core,
        breaks: "a run's first row is read at its place among the rows a filter kept, not at its place in the batch (RULES.md D1325)",
        file: RUNS,
        original: "begins.map(|n| narrow(nth(given, n)))",
        replacement: "begins.map(narrow)",
        integration: &[],
        unit: &[COVERED, APPENDED],
    },
    Mutation {
        name: "entry-numbered-within-its-batch",
        package: Package::Core,
        breaks: "an entry's first row is numbered by its place in its batch, the rows of the batches before it not counted (RULES.md D1325)",
        file: RUNS,
        original: "base = base.saturating_add(wide(self.told(b).map_or(entries, Told::count)));",
        replacement: "base = base.saturating_add(0);",
        integration: &[],
        unit: &[NUMBERED, APPENDED],
    },
    Mutation {
        name: "entry-numbered-by-the-entries-before-it",
        package: Package::Core,
        breaks: "a batch's entries are numbered after the entries of the batches before it, not after their rows (RULES.md D1325)",
        file: RUNS,
        original: "base = base.saturating_add(wide(self.told(b).map_or(entries, Told::count)));",
        replacement: "base = base.saturating_add(wide(entries));",
        integration: &[],
        unit: &[NUMBERED, APPENDED],
    },
    Mutation {
        name: "rows-fed-by-the-first-runs-length",
        package: Package::Core,
        breaks: "each entry of a batch is fed as many rows as its first run holds, so rows go to entries of other runs (RULES.md D1325)",
        file: RUNS,
        original: "let (from, to) = told.span(j);",
        replacement: "let (from, to) = told.span(j.min(0));",
        integration: &[],
        unit: &[NUMBERED, APPENDED],
    },
    Mutation {
        name: "run-counted-as-one-row",
        package: Package::Core,
        breaks: "COUNT(*) of an entry is one whatever the rows of its run (RULES.md D1325)",
        file: STATES,
        original: "*length = i64::try_from(to.saturating_sub(from)).unwrap_or(i64::MAX);",
        replacement: "*length = 1;",
        integration: &[],
        unit: &[APPENDED],
    },
    Mutation {
        name: "run-given-the-state-of-its-first-row",
        package: Package::Core,
        breaks: "an entry's sum, least and greatest are its run's first row's, the rows after it not taken (RULES.md D1325)",
        file: STATES,
        original: "for n in from..to {",
        replacement: "for n in from..to.min(from.saturating_add(1)) {",
        integration: &[],
        unit: &[APPENDED],
    },
    Mutation {
        name: "null-passed-over-in-a-run-unheard",
        package: Package::Core,
        breaks: "a null passed over in a run whose states are set raises no warning (RULES.md D1325)",
        file: STATES,
        original: "any = any || usize::try_from(count).ok() != Some(to.saturating_sub(from));",
        replacement: "any = any && usize::try_from(count).ok() != Some(to.saturating_sub(from));",
        integration: &[],
        unit: &[NULLS],
    },
    Mutation {
        name: "run-sum-past-64-bits-wrapped",
        package: Package::Core,
        breaks: "a run's sum that leaves 64 bits wraps and is set, where the column must be fed and widen (RULES.md D1325, D1308)",
        file: STATES,
        original: "total.checked_add(unit)?",
        replacement: "total.wrapping_add(unit)",
        integration: &[],
        unit: &[WIDE],
    },
];
