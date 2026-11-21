//! The mutations of the plans a kept statement or body keeps in a steady
//! state (RULES.md R-TEST-9, D613): a slot less a constant left out of an
//! index's probe, arithmetic of slots taken as raising, a slotted plan's
//! estimates reading its slots by default, a hash join's extra key told
//! apart, a probe's plan learned again at any growth or kept past any, or
//! kept after its table's statistics are gathered, a slot less a
//! constant estimated as an unknown bound, a FOR statement's cursor
//! opened updatable, and the instant bound as a constant.

use super::super::{Mutation, Package};

/// The test of a slot less a constant in a probe.
const SHIFTED: &str = "physical::probe::tests::a_slot_shifted_by_a_constant_is_compared_with_its_value";

/// The test of the order entry procedures in steady state.
const STEADY: &str = "tpcc_plans::tpcc_procedures_plan_nothing_in_steady_state";

/// The test of a probe's plan outgrown.
const OUTGROWN: &str = "tpcc_plans::a_probes_plan_is_learned_again_only_once_its_rows_per_probe_outgrow_it";

/// The test of a probe's plan learned before its table's statistics.
const UNKNOWN: &str = "tpcc_plans::a_probes_plan_learned_before_statistics_is_learned_again_once_they_are";

/// Mechanisms of plans kept in a steady state whose removal must fail their tests.
pub const STEADY_PLANS: [Mutation; 10] = [
    Mutation {
        name: "steady-probe-shifted-slot",
        package: Package::Core,
        breaks: "a column compared with a slot plus or minus a constant probes no index",
        file: "src/physical/probe.rs",
        original: "        (shifted_slot(a, b)?, Some(Box::new(other.clone())))",
        replacement: "        (shifted_slot(a, b).filter(|_| false)?, Some(Box::new(other.clone())))",
        integration: &[STEADY],
        unit: &[SHIFTED],
    },
    Mutation {
        name: "steady-arith-raises",
        package: Package::Core,
        breaks: "arithmetic of slots and constants is taken as raising whatever its types",
        file: "src/logical/expr.rs",
        original: "            Expr::Arith(..) => super::can_raise_typed(self, &|_| None),",
        replacement: "            Expr::Arith(..) => true,",
        integration: &[STEADY],
        unit: &[SHIFTED],
    },
    Mutation {
        name: "steady-estimates-unvalued",
        package: Package::Core,
        breaks: "a slotted plan's estimates read its slots by default, not as the run's values",
        file: "src/physical/estimate/valued.rs",
        original: "    let values = VALUES.with(|v| v.borrow().clone())?;",
        replacement: "    let values = VALUES.with(|v| v.borrow().clone()).filter(|_| false)?;",
        integration: &[STEADY],
        unit: &["physical::estimate::valued::tests::a_slot_reads_its_value_only_while_its_plan_is_made"],
    },
    Mutation {
        name: "steady-hash-keys-exact",
        package: Package::Core,
        breaks: "a hash join by more keys than the slotted plan's is of another shape",
        file: "src/physical/kept/shape.rs",
        original: "keys.iter().all(|k| keys2.contains(k)) && !keys.is_empty()",
        replacement: "keys == keys2 && !keys.is_empty()",
        integration: &[],
        unit: &["physical::kept::tests::a_hash_join_is_of_the_shape_of_one_by_more_keys"],
    },
    Mutation {
        name: "steady-outgrown-at-once",
        package: Package::Core,
        breaks: "a probe's plan is learned again as soon as its rows per probe grow at all",
        file: "src/db/prepared/plans/grown.rs",
        original: "    now > learned.max(1.0_f64) * OUTGROWN",
        replacement: "    now > learned.max(1.0_f64)",
        integration: &[],
        unit: &["db::prepared::plans::grown::tests::a_probe_outgrows_its_plan_past_ten_times_its_rows"],
    },
    Mutation {
        name: "steady-probes-relearn-on-drift",
        package: Package::Core,
        breaks: "a plan of index probes is learned again whenever a table it reads drifts",
        file: "src/db/prepared/plans/grown.rs",
        original: "        let Some(kept) = &self.probes else { return Fit::Moved };",
        replacement: "        let Some(kept) = self.probes.as_ref().filter(|_| false) else { return Fit::Moved };",
        integration: &[OUTGROWN],
        unit: &[],
    },
    Mutation {
        name: "steady-probes-known-ignored",
        package: Package::Core,
        breaks: "a probe's plan learned before its table's statistics keeps the whole table's rows as its figure",
        file: "src/db/prepared/plans/grown.rs",
        original: "        if probes.iter().any(|p| !p.known && st.statistics(&p.table).is_some()) {",
        replacement: "        if probes.iter().any(|p| !p.known && st.statistics(&p.table).is_none()) {",
        integration: &[UNKNOWN],
        unit: &[],
    },
    Mutation {
        name: "steady-slot-shift-unfolded",
        package: Package::Core,
        breaks: "a slot less a constant is estimated as an unknown bound, so a kept join weighs its probes at too many rows",
        file: "src/physical/estimate/valued.rs",
        original: "    Some(filled.map_exprs(&mut folded))",
        replacement: "    Some(filled.map_exprs(&mut |e| e))",
        integration: &[],
        unit: &[
            "physical::estimate::valued::tests::a_slot_less_a_constant_is_estimated_as_its_value",
            "db::prepared::plans::tests::the_stock_level_query_with_slots_probes_stock",
        ],
    },
    Mutation {
        name: "steady-for-cursor-updatable",
        package: Package::Core,
        breaks: "a FOR statement's cursor no statement can name is opened updatable, planned again at each open",
        file: "src/binder/psm/steps.rs",
        original: "            c.read_only = nameless && !statements.iter().any(Step::writes);",
        replacement: "            c.read_only = !nameless && !statements.iter().any(Step::writes);",
        integration: &[STEADY],
        unit: &[],
    },
    Mutation {
        name: "steady-instant-constant",
        package: Package::Core,
        breaks: "a kept write binds the statement's instant as a constant, bound and planned at each execution",
        file: "src/binder/datetime.rs",
        original: "        if let Some(index) = self.params.clock_slot() {",
        replacement: "        if let Some(index) = self.params.clock_slot().filter(|_| false) {",
        integration: &[STEADY],
        unit: &[],
    },
];
