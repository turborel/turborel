//! The mutations of compiled procedures (RULES.md R-TEST-9, D839): a
//! delta table's statement shape, no data of a SELECT ... INTO of a delta
//! table or of a point read, the CASE statement with no WHEN true, the
//! end of a change run straight, an atomic statement undone, and of a
//! point change written straight, its row's constraints, its UNIQUE ones,
//! its fallback where a constraint may be deferred or a foreign key
//! references its row, its OLD TABLE row, and an INSERT's checks.

use super::super::{Mutation, Package};

/// Constraints broken by changes run straight and by the general path.
const CONSTRAINTS: &str = "compiled_procedures::constraint_errors_are_the_interpreters";

/// A point change's own row's constraints checked as its end checks them.
const ROWS: &str = "compiled_writes::a_point_change_checks_its_rows_constraints";

/// A point change sent to the general path where its end checks more.
const MORE: &str = "compiled_writes::a_point_change_falls_back_where_its_end_checks_more";

/// Mechanisms whose removal must fail compiled procedures' requirements.
pub const COMPILED: [Mutation; 11] = [
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
    Mutation {
        name: "compiled-straight-row-unchecked",
        package: Package::Core,
        breaks: "a point change written straight leaves its row's NOT NULL, CHECK and foreign key constraints unchecked",
        file: "src/db/integrity/straight.rs",
        original: "            for c in &straight.row {",
        replacement: "            for c in straight.row.iter().take(0) {",
        integration: &[ROWS],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-unique-unchecked",
        package: Package::Core,
        breaks: "a point change written straight leaves the UNIQUE constraints on the columns it writes unchecked",
        file: "src/db/integrity/straight.rs",
        original: "        for constraint in &straight.unique {",
        replacement: "        for constraint in straight.unique.iter().take(0) {",
        integration: &[ROWS],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-deferrable",
        package: Package::Core,
        breaks: "a point change of a deferrable constraint is written straight, its check made at once though deferred",
        file: "src/db/integrity/straight.rs",
        original: "            if c.attributes.deferrable {",
        replacement: "            if c.attributes.deferrable && delete {",
        integration: &[MORE],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-referenced",
        package: Package::Core,
        breaks: "a point change of a key a foreign key references is written straight, the reference left unchecked",
        file: "src/db/integrity/straight.rs",
        original: "        if self.referenced_by(table, columns.as_deref()) {",
        replacement: "        if self.referenced_by(table, columns.as_deref()) && columns.is_none() && !delete {",
        integration: &[MORE],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-insert-unchecked",
        package: Package::Core,
        breaks: "an INSERT of one row written straight leaves its row's constraints unchecked",
        file: "src/db/write/direct.rs",
        original: "        st.end_straight(tx, (&target.name, straight), &ids)?;",
        replacement: "        st.end_straight(tx, (&target.name, straight), &[])?;",
        integration: &[CONSTRAINTS],
        unit: &[],
    },
];
