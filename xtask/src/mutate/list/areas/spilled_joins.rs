//! The mutations of a hash join run in parallel whose built operand
//! outgrows its grant (RULES.md R-TEST-9, D1240): its rows put in parts
//! by their keys' cells, a pass keeping the probing rows of its cells
//! alone and those of a key that matches nothing in the first pass alone,
//! the runtime filters of the whole operand on the first pass's table, a
//! pass past the first probing its own table, and a build past
//! the most passes the planner's constants allow given way.

use super::super::{Mutation, Package};

/// The file of the build in parts.
const SPILLED: &str = "src/exec/parallel/probe/spilled.rs";

/// The file of the passes.
const PASSES: &str = "src/exec/parallel/passes.rs";

/// The test of a pipeline probed in passes.
const PASSED: &str = "exec::parallel::passes::tests::a_build_past_the_grant_is_probed_in_passes_or_gives_way";

/// The test of the first pass's runtime filters.
const FILTERS: &str = "exec::parallel::passes::tests::the_first_pass_carries_the_whole_operands_filters";

/// The test of a build stopped as its morsels project too many passes.
const STOPPED: &str = "exec::parallel::probe::spilled::tests::a_build_stops_once_its_morsels_project_too_many_passes";

/// The file of a hash join's weight in the enumeration.
const WEIGHED: &str = "src/physical/enumerate/weighed.rs";

/// The test of the joins weighed probed in passes.
const WEIGHED_PASSES: &str = "physical::enumerate::weighed::tests::only_a_join_a_pipeline_builds_in_parts_costs_passes";

/// The test of joins of each kind built past the grant.
const JOINS: &str = "spilled_joins::joins_built_past_the_grant_give_their_rows";

/// Mechanisms whose removal must fail the requirements of a hash join
/// built in parts and probed in passes.
pub const SPILLED_JOINS: [Mutation; 6] = [
    Mutation {
        name: "passes-cells-ignored",
        package: Package::Core,
        breaks: "a pass keeps the probing rows whose keys fall in its cells alone (D1240)",
        file: SPILLED,
        original: "if *m { self.first } else { self.mask.holds(cell_of(*h)) }",
        replacement: "if *m { self.first } else { true }",
        integration: &[JOINS],
        unit: &[],
    },
    Mutation {
        name: "passes-misses-each-pass",
        package: Package::Core,
        breaks: "a probing row of a key that matches nothing is kept by the first pass alone (D1240)",
        file: SPILLED,
        original: "if *m { self.first } else {",
        replacement: "if *m { true } else {",
        integration: &[JOINS],
        unit: &[],
    },
    Mutation {
        name: "passes-first-filters-own",
        package: Package::Core,
        breaks: "the first pass's table carries the runtime filters of the whole operand (D1240)",
        file: SPILLED,
        original: "let table = table_of(&mut first.lists, (&shape, &self.join), crew, Some(filters))?;",
        replacement: "let table = table_of(&mut first.lists, (&shape, &self.join), crew, None)?;",
        integration: &[JOINS],
        unit: &[FILTERS],
    },
    Mutation {
        name: "passes-limit-ignored",
        package: Package::Core,
        breaks: "a build whose passes would pass the most allowed gives way (D1240)",
        file: SPILLED,
        original: "crate::storage::cost_of(projected.div_ceil(pass)) <= self.most",
        replacement: "crate::storage::cost_of(projected.div_ceil(pass)) >= 0.0_f64",
        integration: &[],
        unit: &[STOPPED],
    },
    Mutation {
        name: "passes-first-table-kept",
        package: Package::Core,
        breaks: "a pass past the first probes its own table, not the first pass's (D1240)",
        file: PASSES,
        original: "Arc::clone(if Arc::ptr_eq(t, first_table) { table } else { t })",
        replacement: "Arc::clone(if Arc::ptr_eq(t, table) { table } else { t })",
        integration: &[JOINS],
        unit: &[PASSED],
    },
    Mutation {
        name: "passes-weighed-of-any-join",
        package: Package::Core,
        breaks: "only an inner join and a LEFT one whose left probes are weighed probed in passes (D1240)",
        file: WEIGHED,
        original: "let passes = kind == Kind::Inner || (kind == Kind::Left && probes_left);",
        replacement: "let passes = kind != Kind::Full;",
        integration: &[],
        unit: &[WEIGHED_PASSES],
    },
];
