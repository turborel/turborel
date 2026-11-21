//! The mutations of correlated subqueries' plans kept across their runs
//! (RULES.md R-TEST-9, D675): a null or a value of another kind filled
//! into a kept plan, a timestamp typed as a date, a value cast to its
//! slot's type, a plan learned and not kept, a null typing a slot, a
//! unique probe left to learn, a plan of slots of its own kept, and a
//! subquery decorrelated soon left to learn.

use super::super::{Mutation, Package};

/// The file the runner keeps a subquery's plan in.
const PLANNED: &str = "src/exec/subquery/planned.rs";
/// The unit test of a plan learned and kept.
const KEPT: &str = "exec::subquery::planned::tests::a_subquery_keeps_its_plan_once_its_runs_agree";
/// The unit test of a unique probe and values of another kind.
const UNIQUE: &str = "exec::subquery::planned::tests::a_unique_probe_is_kept_at_once_and_a_value_of_another_kind_is_planned_alone";
/// The unit test of the kinds a kept plan fits.
const KINDS: &str = "exec::subquery::planned::tests::a_kept_plan_fits_values_of_its_slots_kinds_alone";
/// The unit test of nulls and kinds no slot takes.
const WAITS: &str = "exec::subquery::planned::tests::a_null_waits_and_a_kind_no_slot_takes_plans_each_run";
/// The unit test of a subquery decorrelated soon.
const SOON: &str = "exec::subquery::planned::tests::a_subquery_decorrelated_soon_is_planned_at_each_run";

/// The mutations of kept subquery plans.
pub const SUBQUERY_PLANS: [Mutation; 9] = [
    Mutation {
        name: "subplan-fits",
        package: Package::Core,
        breaks: "a null is filled into the kept plan, its probe finding the rows of null keys",
        file: PLANNED,
        original: ".all(|(v, t)| slot_type(v).is_some_and(|own| discriminant(&own) == discriminant(t)))",
        replacement: ".all(|(v, t)| v.is_null() || slot_type(v).is_some_and(|own| discriminant(&own) == discriminant(t)))",
        integration: &[],
        unit: &[UNIQUE, KINDS],
    },
    Mutation {
        name: "subplan-kind",
        package: Package::Core,
        breaks: "a value of another kind than its slot's is filled into the kept plan",
        file: PLANNED,
        original: "discriminant(&own) == discriminant(t)",
        replacement: "discriminant(&own) == discriminant(&own)",
        integration: &[],
        unit: &[UNIQUE, KINDS],
    },
    Mutation {
        name: "subplan-moment",
        package: Package::Core,
        breaks: "a timestamp's slot is a date's, so a kept plan probes a date index by timestamps",
        file: PLANNED,
        original: "                Moment::Timestamp => DataType::Timestamp { precision, zoned: d.zoned },",
        replacement: "                Moment::Timestamp => DataType::Date,",
        integration: &[],
        unit: &[KINDS],
    },
    Mutation {
        name: "subplan-as-is",
        package: Package::Core,
        breaks: "an outer value is cast to its slot's type as it fills a kept plan",
        file: "src/logical/dynamic.rs",
        original: "        if self.placing == Placing::AsIs {",
        replacement: "        if false {",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "subplan-learned",
        package: Package::Core,
        breaks: "a plan learned is not kept, so each run is planned",
        file: PLANNED,
        original: "        Learned::Kept(plan) => Planned::Kept(slotted, plan),",
        replacement: "        Learned::Kept(_) => Planned::Varies,",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "subplan-null-waits",
        package: Package::Core,
        breaks: "a null outer value has its subquery planned at each run",
        file: PLANNED,
        original: "        if outer.iter().any(Value::is_null) {",
        replacement: "        if false {",
        integration: &[],
        unit: &[WAITS],
    },
    Mutation {
        name: "subplan-unique-at-once",
        package: Package::Core,
        breaks: "a probe of a whole unique key learns over runs instead of kept at once",
        file: PLANNED,
        original: "        if keepable(&with_slots, &self.db) {",
        replacement: "        if false {",
        integration: &[],
        unit: &[UNIQUE],
    },
    Mutation {
        name: "subplan-own-slots",
        package: Package::Core,
        breaks: "a plan holding slots of its own is planned with its outer references as slots too",
        file: PLANNED,
        original: "        if filled(subquery.plan.clone(), &[]).is_err() {",
        replacement: "        if false {",
        integration: &[],
        unit: &[WAITS],
    },
    Mutation {
        name: "subplan-decorrelated-soon",
        package: Package::Core,
        breaks: "a subquery decorrelated after a few runs learns its plan over them, planning each twice",
        file: PLANNED,
        original: "        if self.decorrelated_soon(subquery) {",
        replacement: "        if false {",
        integration: &[],
        unit: &[SOON],
    },
];
