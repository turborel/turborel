//! The mutations of EXPLAIN of a data change without ANALYZE (RULES.md
//! R-TEST-9, D980 to D984): EXPLAIN not read before a data change, or its
//! change planned as a query; a row of values planned; a view's INSTEAD OF
//! trigger passed over; an undeclared cursor taken; an index maintained by
//! an UPDATE of no column its key reads, or one not maintained under a
//! BEFORE row trigger; an action of an UPDATE of no referenced column, or
//! a cascade's table not followed; a trigger's column list not read; and
//! the point path's read, its setting and the procedures' setting.

use super::super::{Mutation, Package};

/// The test of each searched change's text.
const SEARCHED: &str = "explain_changes::each_searched_change_shows_its_plan_and_what_it_would_do";
/// The test of MERGE, of an INSERT of a query and of a view's trigger.
const MERGED: &str = "explain_changes::a_merge_an_insert_of_a_query_and_a_view_its_trigger_takes_are_shown";
/// The test of the point path.
const POINT: &str = "explain_changes::the_point_path_is_the_one_a_prepared_statement_or_procedure_runs";

/// Mechanisms whose removal must fail the requirements of the forms.
pub const EXPLAIN_UNRUN: [Mutation; 12] = [
    Mutation {
        name: "explain-change-unread",
        package: Package::Core,
        breaks: "EXPLAIN alone is not read before a data change",
        file: "src/parser/data_statements.rs",
        original: "    if begins_query(second) || begins_change(second, third) {",
        replacement: "    if begins_query(second) {",
        integration: &[MERGED],
        unit: &["parser::data_statements::tests::explain_is_read_before_a_data_change"],
    },
    Mutation {
        name: "explain-change-as-query",
        package: Package::Core,
        breaks: "EXPLAIN of a data change, as a direct statement, is planned as a query",
        file: "src/db/explain.rs",
        original: "            let text = if change {",
        replacement: "            let text = if change && false {",
        integration: &[MERGED],
        unit: &[],
    },
    Mutation {
        name: "explain-values-planned",
        package: Package::Core,
        breaks: "an INSERT's row of values is shown planned, though it is evaluated",
        file: "src/db/explain/unrun.rs",
        original: ".filter(|l| !evaluates(l))",
        replacement: ".filter(|_| true)",
        integration: &[SEARCHED],
        unit: &[],
    },
    Mutation {
        name: "explain-instead-passed",
        package: Package::Core,
        breaks: "an INSERT into a view an INSTEAD OF trigger takes is shown as an INSERT into its table",
        file: "src/db/explain/unrun.rs",
        original: "        (!instead.is_empty()).then_some(",
        replacement: "        (instead.is_empty() && false).then_some(",
        integration: &[MERGED],
        unit: &[],
    },
    Mutation {
        name: "explain-cursor-undeclared",
        package: Package::Core,
        breaks: "a positioned change through a cursor not declared is shown",
        file: "src/db/explain/unrun.rs",
        original: "        if !self.inner.borrow().cursors.contains_key(&key) {",
        replacement: "        if key.name.is_empty() {",
        integration: &["explain_changes::a_positioned_change_shows_its_cursors_row"],
        unit: &[],
    },
    Mutation {
        name: "explain-maintains-every",
        package: Package::Core,
        breaks: "an UPDATE maintains every index, of a column its key reads or not",
        file: "src/db/explain/unrun/effects.rs",
        original: "*e != Event::Update || global || before || reads.iter().any(|r| c.contains(r))",
        replacement: "*e != Event::Update || global || before || !reads.is_empty()",
        integration: &[SEARCHED],
        unit: &[],
    },
    Mutation {
        name: "explain-before-unread",
        package: Package::Core,
        breaks: "a BEFORE row trigger, which may set any column, leaves an UPDATE's indexes unmaintained",
        file: "src/db/explain/unrun/effects.rs",
        original: "    let before = changes.events.iter().any(",
        replacement: "    let before = false && changes.events.iter().any(",
        integration: &["explain_changes::a_before_row_trigger_makes_an_update_maintain_every_index"],
        unit: &[],
    },
    Mutation {
        name: "explain-action-any-update",
        package: Package::Core,
        breaks: "an UPDATE of no referenced column is shown cascading",
        file: "src/db/explain/unrun/effects.rs",
        original: "fk.referenced.iter().any(|c| columns.contains(c))",
        replacement: "!fk.referenced.is_empty()",
        integration: &[SEARCHED],
        unit: &[],
    },
    Mutation {
        name: "explain-cascade-unfollowed",
        package: Package::Core,
        breaks: "a table a referential action writes is not followed, so its triggers are not shown",
        file: "src/db/explain/unrun/effects.rs",
        original: "            if !reached.contains(&next) {",
        replacement: "            if reached.is_empty() {",
        integration: &[SEARCHED],
        unit: &[],
    },
    Mutation {
        name: "explain-trigger-columns-unread",
        package: Package::Core,
        breaks: "an UPDATE's columns are not given, so a trigger of a column list is not shown",
        file: "src/db/explain/unrun/effects.rs",
        original: "activates(t, (table, event, &names))",
        replacement: "activates(t, (table, event, &[]))",
        integration: &[SEARCHED],
        unit: &[],
    },
    Mutation {
        name: "explain-point-setting-unread",
        package: Package::Core,
        breaks: "the point read is shown with EXECUTOR.POINT_OPERATIONS OFF",
        file: "src/db/explain/unrun.rs",
        original: ".filter(|_| point_operations(st));",
        replacement: ".filter(|_| true);",
        integration: &[POINT],
        unit: &[],
    },
    Mutation {
        name: "explain-procedures-setting-unread",
        package: Package::Core,
        breaks: "a procedure's straight write is shown with EXECUTOR.COMPILED_PROCEDURES OFF",
        file: "src/db/explain/unrun.rs",
        original: "        compiled_procedures(st, self) && runs_straight(st, stmt, (named, table))",
        replacement: "        runs_straight(st, stmt, (named, table))",
        integration: &[POINT],
        unit: &[],
    },
];
