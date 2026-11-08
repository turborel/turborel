//! The mutations of prepared statements' kept plans (RULES.md R-TEST-9,
//! D45): what a plan is kept with, a plan holding the statement's
//! instant, and a physical plan kept with its slots, filled at each
//! execution (`physical/kept.rs`).

use super::{Mutation, Package};

/// The mutations of kept plans, in the order they run.
pub const KEPT: [Mutation; 4] = [
    Mutation {
        name: "kept-plan-definitions",
        package: Package::Core,
        breaks: "kept plans: a prepared statement's plan kept across a change of the catalog",
        file: "src/db/prepared/plans.rs",
        original: " && std::ptr::addr_eq(Weak::as_ptr(&key.catalog), Arc::as_ptr(&self.st.catalog));",
        replacement: ";",
        integration: &["kept_plans::a_kept_plan_follows_the_transactions_own_definitions"],
        unit: &[],
    },
    Mutation {
        name: "kept-plan-instant",
        package: Package::Core,
        breaks: "kept plans: a plan holding the statement's instant kept for the next execution",
        file: "src/value/datetime.rs",
        original: "            read.set(true);",
        replacement: "            read.set(false);",
        integration: &["kept_plans::a_plan_holding_the_instant_is_made_at_each_execution"],
        unit: &[],
    },
    Mutation {
        name: "kept-plan-probe",
        package: Package::Core,
        breaks: "kept plans: a kept probe's `?` left the null that stood in for its value",
        file: "src/physical/kept.rs",
        original: "            *at = value;\n            Ok(())",
        replacement: "            drop((at, value));\n            Ok(())",
        integration: &["point_plans::a_prepared_probe_of_a_whole_unique_key_is_kept_and_filled"],
        unit: &["physical::kept::tests::a_kept_plan_is_filled_with_each_executions_values"],
    },
    Mutation {
        name: "kept-plan-fixed",
        package: Package::Core,
        breaks: "kept plans: a projection or filter over a probe fixing a whole unique key planned at each execution",
        file: "src/physical/kept.rs",
        original: "        Plan::Project { input, .. } | Plan::Filter { input, .. } => keepable(input, st),",
        replacement: "        Plan::Project { .. } | Plan::Filter { .. } => false,",
        integration: &[],
        unit: &[
            "db::prepared::plans::tests::a_point_querys_physical_plan_is_kept_after_its_first_executions",
            "physical::kept::tests::a_probe_of_a_whole_unique_key_alone_is_kept",
        ],
    },
];
