//! The mutations of a correlated subquery kept probed by a unique key
//! until its probes cost what its build would (RULES.md D790, R-TEST-9),
//! and of its probe run as a point read (D900): a value its slots do not
//! take read so, and a key of two columns probed by its first value twice.

use super::super::{Mutation, Package};

/// The mutations of probed subqueries, in the order they run.
pub const PROBED_SUBQUERIES: [Mutation; 4] = [
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
    Mutation {
        name: "subquery-point-read-unfit",
        package: Package::Core,
        breaks: "a subquery's point read takes a null or a value of another kind than its slot's, which its plan is planned alone for",
        file: "src/exec/subquery/pointed.rs",
        original: "self.kept_run(subquery.id()).filter(|kept| kept.fits(outer))",
        replacement: "self.kept_run(subquery.id()).filter(|kept| kept.fits(outer) || !outer.is_empty())",
        integration: &[],
        unit: &["exec::subquery::pointed::tests::a_kept_unique_probe_is_read_as_a_point_read"],
    },
    Mutation {
        name: "subquery-point-read-misplaced",
        package: Package::Core,
        breaks: "a subquery's point read places its first outer value for every slot, so a key of two columns reads another row",
        file: "src/exec/point.rs",
        original: "            *at? = values.get(s.slot.index)?.clone();",
        replacement: "            *at? = values.first()?.clone();",
        integration: &["point_operations::a_subquery_of_a_whole_key_reads_as_the_executor_does"],
        unit: &[],
    },
];
