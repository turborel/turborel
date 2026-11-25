//! The mutations of a hash join's operand built by the threads that read
//! it (RULES.md R-TEST-9, D1080): a run's rows sorted by part in the
//! order built, a part made of the runs in order, a direct table's rows
//! of a null key kept, keys given as numbers hashed for a hashed table,
//! each vector of the batch joined of its own column, the runtime
//! filters made beside the table, and an operand of a morsel's rows left
//! to the statement's thread.

use super::super::{Mutation, Package};

/// The parted table's file.
const PARTED: &str = "src/exec/table/join/parted.rs";

/// The file of the build by many threads.
const BUILD: &str = "src/exec/parallel/build.rs";

/// The test of a hashed table made in parts.
const HASHED: &str = "exec::table::join::parted::tests::a_hashed_table_in_parts_finds_what_one_thread_s_finds";

/// The test of a table built by many threads.
const BUILT: &str = "exec::parallel::build::tests::a_table_built_by_many_threads_is_the_one_a_thread_builds";

/// Mechanisms whose removal must fail the requirements of a join's
/// operand built by many threads.
pub const JOIN_BUILD: [Mutation; 7] = [
    Mutation {
        name: "join-build-run-order",
        package: Package::Core,
        breaks: "a run's rows of a part stay in the order they were built",
        file: PARTED,
        original: "        for (part, pair) in keyed {",
        replacement: "        for (part, pair) in keyed.into_iter().rev() {",
        integration: &[],
        unit: &[HASHED],
    },
    Mutation {
        name: "join-build-runs-in-order",
        package: Package::Core,
        breaks: "a part is made of every run's rows, the runs in order",
        file: PARTED,
        original: "        let built = || runs.iter().flat_map(",
        replacement: "        let built = || runs.iter().rev().flat_map(",
        integration: &[],
        unit: &[HASHED],
    },
    Mutation {
        name: "join-build-null-rows",
        package: Package::Core,
        breaks: "a direct table made in parts keeps the rows of a null key whose nulls match",
        file: PARTED,
        original: "                nulls.push(row);",
        replacement: "                nulls.clear();",
        integration: &[],
        unit: &["exec::table::join::parted::tests::a_direct_table_in_parts_finds_what_one_thread_s_finds"],
    },
    Mutation {
        name: "join-build-numbers-unhashed",
        package: Package::Core,
        breaks: "keys asked as numbers are hashed for a table that is not direct",
        file: BUILD,
        original: "        if numbers && !parting.direct() {",
        replacement: "        if numbers && parting.direct() {",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-one-column",
        package: Package::Core,
        breaks: "each vector of the built batch is joined of the parts' vectors at its place",
        file: "src/storage/batch/held.rs",
        original: "            parts.iter().map(|part| (part.columns.get(at), all.get(..part.rows).unwrap_or_default())).collect();",
        replacement: "            parts.iter().map(|part| (part.columns.get(at.min(1)), all.get(..part.rows).unwrap_or_default())).collect();",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-no-filters",
        package: Package::Core,
        breaks: "a table built by many threads makes the runtime filters of its keys",
        file: BUILD,
        original: "            Some(k) => Ok(Made::Filter(held.filter(k)?.map(Arc::new))),",
        replacement: "            Some(_) => Ok(Made::Filter(None)),",
        integration: &[],
        unit: &[BUILT],
    },
    Mutation {
        name: "join-build-small-alone",
        package: Package::Core,
        breaks: "an operand of no more rows than a morsel is built by the statement's thread",
        file: BUILD,
        original: "    if crew.degree < 2 || rows <= crew.run_rows() {",
        replacement: "    if crew.degree < 2 || rows < crew.run_rows() {",
        integration: &[],
        unit: &["exec::parallel::build::tests::a_small_operand_or_one_thread_builds_alone"],
    },
];
