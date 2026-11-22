//! The mutations of a correlated subquery kept probed by a unique key
//! until its probes cost what its build would (RULES.md D790, R-TEST-9).

use super::super::{Mutation, Package};

/// The mutations of probed subqueries, in the order they run.
pub const PROBED_SUBQUERIES: [Mutation; 2] = [
    Mutation {
        name: "probed-subqueries-built-at-once",
        package: Package::Core,
        breaks: "a subquery probed by a unique key is decorrelated after DECORRELATE_AFTER_RUNS runs, building the whole table",
        file: "src/exec/subquery.rs",
        original: "        if how.as_ref().is_some_and(|how| self.probes_pay(at, runs, how)) {\n",
        replacement: "        if how.as_ref().is_some_and(|how| self.probes_pay(at, runs, how) && false) {\n",
        integration: &[],
        unit: &["exec::subquery::probed::tests::a_unique_probe_is_decorrelated_once_its_probes_cost_a_build"],
    },
    Mutation {
        name: "probed-subqueries-never-built",
        package: Package::Core,
        breaks: "a subquery probed by a unique key is never decorrelated, however many runs it makes",
        file: "src/exec/subquery/probed.rs",
        original: "        crate::storage::cost_of(runs) * PROBE_ROWS < built\n",
        replacement: "        true\n",
        integration: &[],
        unit: &["exec::subquery::probed::tests::a_unique_probe_is_decorrelated_once_its_probes_cost_a_build"],
    },
];
