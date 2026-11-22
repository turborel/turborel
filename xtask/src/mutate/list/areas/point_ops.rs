//! The mutations of point operations (RULES.md R-TEST-9, D752): a kept
//! plan is a point read only if it probes a whole unique key, and only
//! while the setting asks for one; a column given twice is copied, not
//! moved; and a kept write's object row is the one its read found.

use super::super::{Mutation, Package};

/// Mechanisms of point operations whose removal must fail their tests.
pub const POINT_OPS: [Mutation; 4] = [
    Mutation {
        name: "point-ops-any-probe-a-point",
        package: Package::Core,
        breaks: "a probe of a key many rows share is read as a point read, which gives one of them",
        file: "src/exec/point.rs",
        original: "        if !keepable(plan, st) {",
        replacement: "        if keepable(plan, st) && false {",
        integration: &[],
        unit: &["exec::point::tests::a_probe_of_many_rows_or_a_computed_value_is_no_point_read"],
    },
    Mutation {
        name: "point-ops-setting-ignored",
        package: Package::Core,
        breaks: "point operations turned off still run a kept plan as its point read",
        file: "src/physical/kept/filled.rs",
        original: "        if !crate::exec::point_operations(st) {",
        replacement: "        if false {",
        integration: &[],
        unit: &["exec::point::tests::point_operations_off_run_a_kept_plan_through_the_executor"],
    },
    Mutation {
        name: "point-ops-column-moved-twice",
        package: Package::Core,
        breaks: "a column given twice is moved out of the row each time, the second a null",
        file: "src/exec/point.rs",
        original: "            let found = if self.distinct { at.map(|v| mem::replace(v, Value::Null)) } else { at.map(|v| v.clone()) };",
        replacement: "            let found = at.map(|v| mem::replace(v, Value::Null));",
        integration: &["point_operations::point_operations_read_as_the_executor_does"],
        unit: &["exec::point::tests::a_probe_of_a_whole_unique_key_is_read_as_its_plan_reads"],
    },
    Mutation {
        name: "point-ops-object-row-lost",
        package: Package::Core,
        breaks: "a kept UPDATE or DELETE whose point read finds its row changes none",
        file: "src/db/write/pointed.rs",
        original: "        held.push(row)?;",
        replacement: "        drop(row);",
        integration: &["point_operations::point_operations_read_as_the_executor_does"],
        unit: &[],
    },
];
