//! The mutations of compiled procedures (RULES.md R-TEST-9, D841): a
//! delta table's statement shape, no data of a SELECT ... INTO of a delta
//! table or of a point read, the CASE statement with no WHEN true, the
//! end of a change run straight, and an atomic statement undone.

use super::super::{Mutation, Package};

/// Constraints broken by changes run straight and by the general path.
const CONSTRAINTS: &str = "compiled_procedures::constraint_errors_are_the_interpreters";

/// Mechanisms whose removal must fail compiled procedures' requirements.
pub const COMPILED: [Mutation; 6] = [
    Mutation {
        name: "compiled-old-table-of-update",
        package: Package::Core,
        breaks: "a SELECT ... INTO of an UPDATE's OLD TABLE runs straight, as of a DELETE's",
        file: "src/db/routines/program/compiled/change.rs",
        original: "        DeltaResult::Old => delete,",
        replacement: "        DeltaResult::Old => update,",
        integration: &[],
        unit: &["db::routines::program::compiled::change::tests::the_shapes_that_run_straight"],
    },
    Mutation {
        name: "compiled-delta-no-data",
        package: Package::Core,
        breaks: "a SELECT ... INTO of a delta table of no row run straight completes successfully",
        file: "src/db/routines/program/compiled/change.rs",
        original: "        [] => Ok(Completion::NoData),",
        replacement: "        [] => Ok(Completion::Success),",
        integration: &["compiled_procedures::old_tables_deletes_and_generated_values_alike"],
        unit: &[],
    },
    Mutation {
        name: "compiled-into-no-data",
        package: Package::Core,
        breaks: "a compiled SELECT ... INTO of no row completes successfully, no NOT FOUND handler activated",
        file: "src/db/routines/program/compiled/evaluated.rs",
        original: "            [] => Ok(Completion::NoData),",
        replacement: "            [] => Ok(Completion::Success),",
        integration: &["compiled_procedures::a_body_of_point_shapes_runs_as_interpreted"],
        unit: &[],
    },
    Mutation {
        name: "compiled-case-not-found",
        package: Package::Core,
        breaks: "a compiled CASE statement with no WHEN true and no ELSE goes on, and IF with none raises 20000",
        file: "src/db/routines/program/compiled/flow.rs",
        original: "        if !case {",
        replacement: "        if case {",
        integration: &["compiled_procedures::control_flow_and_conditions_are_the_interpreters"],
        unit: &[],
    },
    Mutation {
        name: "compiled-change-unended",
        package: Package::Core,
        breaks: "a change run straight leaves its statement's effects unchecked at its end",
        file: "src/db/routines/program/compiled/change.rs",
        original: "        if done.is_some() {",
        replacement: "        if done.is_none() {",
        integration: &[CONSTRAINTS],
        unit: &[],
    },
    Mutation {
        name: "compiled-statement-not-undone",
        package: Package::Core,
        breaks: "a compiled statement that fails keeps what it changed before it failed",
        file: "src/db/routines/program/compiled/run.rs",
        original: "            (Err(e), Some(m)) => Err(self.host.undo(m, &e).err().unwrap_or(e)),",
        replacement: "            (Err(e), Some(_)) => Err(e),",
        integration: &[CONSTRAINTS],
        unit: &[],
    },
];
