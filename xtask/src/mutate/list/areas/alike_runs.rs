//! The mutations of the runs of rows of one key a grouping finds before
//! it looks any up (RULES.md R-TEST-9, D1323): what makes two rows
//! alike, which row of a run is hashed, which rows its group takes, and
//! the hash of an integer lane's value by its word.

use super::super::{Mutation, Package};

/// The file the runs are found in.
const ALIKE: &str = "src/exec/batch/groups/alike.rs";

/// The file a morsel's rows are grouped in.
const MORSEL: &str = "src/exec/batch/groups/morsel.rs";

/// A run ends where any column changes.
const ANY: &str = "exec::batch::groups::alike::tests::a_run_ends_where_any_column_changes";

/// Selected rows are compared in the order given.
const SELECTED: &str = "exec::batch::groups::alike::tests::selected_rows_are_compared_in_the_order_given";

/// Texts, codes and datetimes tell their runs.
const TEXTS: &str = "exec::batch::groups::alike::tests::texts_codes_and_datetimes_tell_their_runs";

/// A column that tells no run tells none.
const NONE: &str = "exec::batch::groups::alike::tests::a_column_that_tells_no_run_tells_none";

/// Groups of rows in runs are the plan's.
const PLANS: &str = "alike_runs::groups_of_rows_in_runs_are_the_plans";

/// Mechanisms whose removal must fail the requirements of the runs of
/// rows of one key.
pub const ALIKE_RUNS: [Mutation; 12] = [
    Mutation {
        name: "run-of-the-last-column-of-words-alone",
        package: Package::Core,
        breaks: "a run ends only where the last grouping column read by its words changes, so rows of two keys go to one group (RULES.md D1323)",
        file: ALIKE,
        original: "*begun |= pair.first() != pair.last();",
        replacement: "*begun = pair.first() != pair.last();",
        integration: &[],
        unit: &[ANY],
    },
    Mutation {
        name: "run-of-the-last-column-with-nulls-alone",
        package: Package::Core,
        breaks: "a run ends only where the last grouping column that holds nulls changes, so rows of two keys go to one group (RULES.md D1323)",
        file: ALIKE,
        original: "*begun |= !alike;",
        replacement: "*begun = !alike;",
        integration: &[],
        unit: &[ANY],
    },
    Mutation {
        name: "null-alike-the-number-in-its-place",
        package: Package::Core,
        breaks: "a null is alike a row whose number its place holds, so it joins that row's group (RULES.md D1323)",
        file: ALIKE,
        original: "let alike = if was_null || this.1 { was_null && this.1 } else { word == this.0 };",
        replacement: "let alike = word == this.0;",
        integration: &[],
        unit: &[ANY],
    },
    Mutation {
        name: "selected-row-compared-as-the-batchs",
        package: Package::Core,
        breaks: "the row given at a place is read at that place of the batch, not where the selection says (RULES.md D1323)",
        file: ALIKE,
        original: "let r = nth(rows, n);\n        let this = (words.get(r)?, null(r));",
        replacement: "let r = n;\n        let this = (words.get(r)?, null(r));",
        integration: &[],
        unit: &[SELECTED],
    },
    Mutation {
        name: "null-text-alike-the-empty-string",
        package: Package::Core,
        breaks: "a null character string is alike the empty string, so the two share a group (RULES.md D1323)",
        file: ALIKE,
        original: "let this = texts.get(r);",
        replacement: "let this = Some(texts.get(r).unwrap_or(\"\"));",
        integration: &[],
        unit: &[TEXTS],
    },
    Mutation {
        name: "runs-told-of-a-short-vector",
        package: Package::Core,
        breaks: "runs are told of a vector shorter than its batch, the rows past it joined to the last run (RULES.md D1323)",
        file: ALIKE,
        original: "if words.len() != begins.len() {",
        replacement: "if words.len() > begins.len() {",
        integration: &[],
        unit: &[NONE],
    },
    Mutation {
        name: "run-first-row-hashed-at-its-place-given",
        package: Package::Core,
        breaks: "a run's first row is hashed of the batch's row at its place among the rows given, another row's under a WHERE clause (RULES.md D1323)",
        file: MORSEL,
        original: "let firsts: Vec<u32> = begins.iter().map(|&n| narrow(nth(rows, n))).collect();",
        replacement: "let firsts: Vec<u32> = begins.iter().map(|&n| narrow(n)).collect();",
        integration: &[PLANS],
        unit: &[],
    },
    Mutation {
        name: "run-rows-after-the-first-not-taken",
        package: Package::Core,
        breaks: "only the first row of a run is taken by its group, the rows after it counted nowhere (RULES.md D1323)",
        file: MORSEL,
        original: "for n in from..end {",
        replacement: "for n in from..end.min(from.saturating_add(1)) {",
        integration: &[PLANS],
        unit: &[],
    },
    Mutation {
        name: "integer-lane-hashed-of-another-word",
        package: Package::Core,
        breaks: "a group's hash of an integer lane is not the hash of that integer's value, so groups of one value go to two partitions (RULES.md D1323)",
        file: "src/exec/table/keys/words.rs",
        original: "return seed.word(unit.cast_unsigned());",
        replacement: "return seed.word(unit.cast_unsigned().wrapping_add(1));",
        integration: &[],
        unit: &["exec::table::keys::tests::a_groups_hash_of_its_lanes_is_that_of_their_values"],
    },
    Mutation {
        name: "texts-of-one-length-alike",
        package: Package::Core,
        breaks: "two character strings of one length are alike whatever their octets, so the second joins the first's group (RULES.md D1323)",
        file: ALIKE,
        original: "*begun |= this.len() != before.len() || (!this.is_empty() && this != before);",
        replacement: "*begun |= this.len() != before.len();",
        integration: &[],
        unit: &[TEXTS],
    },
    Mutation {
        name: "runs-followed-of-rows-taken-by-their-codes",
        package: Package::Core,
        breaks: "a grouping by batch that takes rows by their dictionary codes follows runs too, the rows after a run's first given to the group of an earlier run (RULES.md D1323)",
        file: "src/exec/batch/groups.rs",
        original: "let runs = if coded { None } else { run_ends(batch, &how.read.groups, rows) };",
        replacement: "let runs = run_ends(batch, &how.read.groups, rows);",
        integration: &[PLANS],
        unit: &[],
    },
    Mutation {
        name: "selected-text-compared-as-the-batchs",
        package: Package::Core,
        breaks: "the character string given at a place is read at that place of the batch, not where the selection says (RULES.md D1323)",
        file: ALIKE,
        original: "let r = nth(rows, n);\n        let from = r.checked_sub(1)",
        replacement: "let r = n;\n        let from = r.checked_sub(1)",
        integration: &[],
        unit: &[SELECTED],
    },
];
