//! The mutations of compiled procedures (RULES.md R-TEST-9, D841): a
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

/// An INSERT of a query's rows run straight, each of its rows checked.
const QUERIED: &str = "compiled_writes::an_insert_of_a_querys_rows_checks_each";

/// An INSERT of a query's rows whose end checks more, or whose rows are
/// stamped as they go in, run by the general path.
const FALLS_BACK: &str = "compiled_writes::an_insert_of_a_querys_rows_falls_back_where_its_end_checks_more";

/// Mechanisms whose removal must fail compiled procedures' requirements.
pub const COMPILED: [Mutation; 18] = [
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
        breaks: "a SELECT ... INTO of a delta table of no row completes successfully, unseen by a NOT FOUND handler",
        file: "src/db/routines/program/compiled/change.rs",
        original: "        [] => Ok(Completion::NoData),",
        replacement: "        [] => Ok(Completion::Success),",
        integration: &[ROWS],
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
        breaks: "a change run by the write functions leaves its statement's effects unchecked at its end",
        file: "src/db/routines/program/compiled/change.rs",
        original: "        if done.is_some() {",
        replacement: "        if done.is_none() {",
        integration: &[MORE],
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
        original: "        for (constraint, alone) in &straight.unique {",
        replacement: "        for (constraint, alone) in straight.unique.iter().take(0) {",
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
        breaks: "an INSERT written straight leaves the constraints of the rows it holds unchecked (D901)",
        file: "src/db/integrity/straight/rows.rs",
        original: "                for c in &straight.row {",
        replacement: "                for c in straight.row.iter().take(0) {",
        integration: &[QUERIED],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-insert-unstamped",
        package: Package::Core,
        breaks: "an INSERT into a table with a system-time period is written straight, its rows' period unstamped",
        file: "src/db/write/direct.rs",
        original: "        if !self.checks.is_empty() || super::system_time::system_time(&self.desc).is_some() {",
        replacement: "        if !self.checks.is_empty() {",
        integration: &[FALLS_BACK],
        unit: &[],
    },
    Mutation {
        name: "kept-plan-lent-over-rows",
        package: Package::Core,
        breaks: "a kept projection of a collection derived table over other rows is read where it is kept, as if over one row of no column (D903)",
        file: "src/exec/lent.rs",
        original: "        let over_one = rows.len() == 1 && rows.iter().all(Vec::is_empty);",
        replacement: "        let over_one = true;",
        integration: &[],
        unit: &["exec::lent::tests::any_other_plan_is_opened_by_the_executor"],
    },
    Mutation {
        name: "compiled-straight-rows-settled-unasked",
        package: Package::Core,
        breaks: "an INSERT written straight takes a UNIQUE constraint as settled of a row whose key was not its key's only entry (D901)",
        file: "src/db/integrity/straight/rows.rs",
        original: "    (alone && !by.is_empty()) || settled(tx, by, id)",
        replacement: "    !by.is_empty() || settled(tx, by, id)",
        integration: &[QUERIED],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-held-unchecked",
        package: Package::Core,
        breaks: "a point change written straight leaves the constraints of the row it holds unchecked (D880)",
        file: "src/db/integrity/straight.rs",
        original: "                    Some(row) => self.check_row_values(tx, (desc, c), row, &mut checking.bound).map_err(|e| broken(e, (desc, c)))?,",
        replacement: "                    Some(_) => {}",
        integration: &[ROWS],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-settled-unasked",
        package: Package::Core,
        breaks: "a point change written straight takes a UNIQUE constraint as settled though its row's entry was not inserted alone (D880)",
        file: "src/db/integrity/straight.rs",
        original: "    alone.iter().any(|index| tx.alone(index, id))",
        replacement: "    !alone.is_empty()",
        integration: &[ROWS],
        unit: &[],
    },
    Mutation {
        name: "compiled-straight-reads-unchecked",
        package: Package::Core,
        breaks: "a point change written straight checks its foreign key with reads no commit checks (D880)",
        file: "src/db/integrity/straight.rs",
        original: "        if !ids.is_empty() && (straight.reads || reads_back || ids.iter().any(unsettled)) {",
        replacement: "        if !ids.is_empty() && (reads_back || ids.iter().any(unsettled)) {",
        integration: &["compiled_writes::a_point_change_checks_its_reference_serializably"],
        unit: &[],
    },
    Mutation {
        name: "compiled-insert-defaults-kept",
        package: Package::Core,
        breaks: "an INSERT of a compiled program takes its target as kept though a default reads the instant, so each row has the instant its target was kept at",
        file: "src/db/write/direct.rs",
        original: "                Some(ColumnDefault::User(_) | ColumnDefault::Datetime(..)) => false,",
        replacement: "                Some(ColumnDefault::User(_) | ColumnDefault::Datetime(..)) => true,",
        integration: &["compiled_writes::an_insert_takes_its_defaults_at_each_execution"],
        unit: &[],
    },
];
