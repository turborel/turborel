//! The mutations of triggers (RULES.md R-TEST-9): an INSERT's BEFORE
//! triggers skipped, AFTER triggers never executed, a statement trigger
//! skipped for no rows, UPDATE OF and WHEN ignored, the order of creation
//! reversed, generated columns left as a BEFORE trigger found them, a
//! referential action's, a positioned statement's and a MERGE clause's
//! BEFORE triggers skipped, the cause's condition lost, the action run as
//! the user, not the schema's owner, too shallow a nesting limit, a write
//! through a view that has an INSTEAD OF trigger, its privileges kept
//! after the trigger is dropped, triggers kept past the drop of a column
//! they use or of a table they name, a BEFORE trigger reading a generated
//! column, the TRIGGER privilege unchecked and a triggered action
//! exception told with no condition left out. Of the INSTEAD OF triggers
//! that take a change of a view: a MERGE's clauses routed to none, a
//! clause they take written, its insert's values lost, its generated
//! columns computed, a view's rows of one row id, a view not updatable
//! read through a base table, a positioned or nested change written, a
//! nested insert's or a nested view's columns not mapped, a view WITH
//! CHECK OPTION over one, a triggered action refused when it changes one,
//! and a statement of it blind to its action's transition variables or
//! tables. A MERGE's BEFORE trigger given the source's columns as its old
//! row. Of the Information Schema: a trigger's place counted across row and
//! statement triggers, and its condition kept as nothing.
//!
//! An AFTER DELETE of a positioned statement is no separate mutation. It
//! records its transition through the call that also keeps the row's
//! cursor state, so skipping one skips the other and the cursor tests
//! catch it.

use super::{Mutation, Package};

/// The file of triggers activated by a state change, executed.
const FIRE: &str = "src/db/triggers/fire.rs";
/// The file of a triggered action, executed.
const RUN: &str = "src/db/triggers/run.rs";
/// The file of CREATE TRIGGER and DROP TRIGGER.
const DEFINE: &str = "src/db/triggers/define.rs";
/// The file of the objects dropped with what a trigger uses.
const DEPENDENTS: &str = "src/db/triggers/dependents.rs";
/// The test of an UPDATE OF trigger.
const UPDATE_OF: &str = "triggers::an_update_of_trigger_runs_only_for_its_columns";
/// The test of a BEFORE trigger changing a row.
const BEFORE: &str = "triggers::a_before_row_trigger_changes_the_new_row";
/// The test of an AFTER trigger.
const AFTER: &str = "triggers::an_after_row_trigger_sees_each_row_inserted";
/// The test of generated columns and BEFORE triggers.
const GENERATED: &str = "triggers::a_before_trigger_leaves_generated_columns_alone";
/// The test of a failed action.
const FAILED: &str = "triggers_firing::a_failure_names_the_trigger_in_the_diagnostics_area";
/// The file of a MERGE's clauses INSTEAD OF triggers take.
const MERGE_ROUTED: &str = "src/db/merge/routed.rs";
/// The file of the INSTEAD OF trigger that takes a change of a view.
const ROUTED: &str = "src/db/triggers/routed.rs";
/// The test of a MERGE into a read-only view with INSTEAD OF triggers.
const MERGE_VIEW: &str = "triggers_routed::a_merge_into_a_read_only_view_runs_its_triggers";
/// The test of a view over a view with INSTEAD OF triggers.
const NESTED: &str = "triggers_routed::a_view_over_a_trigger_view_is_changed_by_its_triggers";
/// The test of a positioned change of a view with INSTEAD OF triggers.
const POSITIONED: &str = "triggers_routed::a_positioned_change_of_a_view_runs_its_triggers";
/// The test of a triggered action changing a view with INSTEAD OF triggers.
const ACTION: &str = "triggers_routed::a_triggered_action_changes_a_trigger_view";

/// The mutations of triggers, in the order they run.
pub const TRIGGERS: [Mutation; 40] = [
    Mutation {
        name: "triggers-before-insert-skipped",
        package: Package::Core,
        breaks: "an INSERT writes its rows without executing its table's BEFORE triggers",
        file: "src/db/write.rs",
        original: "let mut rows = effects.before_insert(cx, &target.name, rows)?;",
        replacement: "let mut rows = rows;",
        integration: &[BEFORE, GENERATED],
        unit: &[],
    },
    Mutation {
        name: "triggers-after-never-executed",
        package: Package::Core,
        breaks: "a statement's AFTER triggers never execute once its constraints are checked",
        file: "src/db/integrity.rs",
        original: "triggered.map_or(Ok(()), |t| t.after(self, tx))",
        replacement: "triggered.map_or(Ok(()), |_| Ok(()))",
        integration: &[AFTER, "triggers::a_statement_trigger_runs_for_no_rows"],
        unit: &[],
    },
    Mutation {
        name: "triggers-statement-skipped-for-no-rows",
        package: Package::Core,
        breaks: "a DELETE of no rows returns before its statement triggers execute",
        file: "src/db/write.rs",
        original: "    if objects.is_empty() && effects.triggered.is_none() {\n        return Ok(0);\n    }\n    // [ISO/IEC 9075-2:2023 §15.8 GR2]",
        replacement: "    if objects.is_empty() {\n        return Ok(0);\n    }\n    // [ISO/IEC 9075-2:2023 §15.8 GR2]",
        integration: &["triggers::a_statement_trigger_runs_for_no_rows"],
        unit: &[],
    },
    Mutation {
        name: "triggers-update-of-ignored",
        package: Package::Core,
        breaks: "an UPDATE OF trigger is activated by an update of any column",
        file: "src/db/triggers/changes.rs",
        original: "list.is_empty() || list.iter().any(|c| columns.contains(c))",
        replacement: "list.is_empty() || !columns.is_empty() || list.iter().any(|c| columns.contains(c))",
        integration: &[UPDATE_OF],
        unit: &[],
    },
    Mutation {
        name: "triggers-when-ignored",
        package: Package::Core,
        breaks: "a triggered action executes whatever its WHEN condition gives",
        file: RUN,
        original: "Ok(matches!(eval(&e, &Row::new(), &Runner::new(cx.st, cx.tx.id))?, Value::Bool(true)))",
        replacement: "eval(&e, &Row::new(), &Runner::new(cx.st, cx.tx.id)).map(|_| true)",
        integration: &["triggers::a_when_condition_picks_the_rows", "triggers_firing::a_recursive_trigger_stops_at_the_limit"],
        unit: &[],
    },
    Mutation {
        name: "triggers-newest-first",
        package: Package::Core,
        breaks: "the triggers of a table execute newest first, not in the order they were created",
        file: "src/catalog/memory.rs",
        original: "triggers.sort_by_key(|t| t.created);\n        triggers\n    }\n    fn triggers_on",
        replacement: "triggers.sort_by_key(|t| std::cmp::Reverse(t.created));\n        triggers\n    }\n    fn triggers_on",
        integration: &["triggers::triggers_run_in_the_order_created"],
        unit: &[],
    },
    Mutation {
        name: "triggers-generated-not-recomputed",
        package: Package::Core,
        breaks: "a generated column keeps the value of the row before its BEFORE triggers changed it",
        file: FIRE,
        original: "if altered { regenerated(cx, of.0, changes) } else { Ok(changes) }",
        replacement: "if altered && false { regenerated(cx, of.0, changes) } else { Ok(changes) }",
        integration: &[GENERATED],
        unit: &[],
    },
    Mutation {
        name: "triggers-cascade-before-skipped",
        package: Package::Core,
        breaks: "a row a referential action updates is written without its table's BEFORE triggers",
        file: "src/db/integrity/actions.rs",
        original: "let next = effects.before_updated_row(cx, (child, &columns), (row_id, &row, next))?;",
        replacement: "let next = next;",
        integration: &["triggers_firing::a_referential_update_executes_before_triggers"],
        unit: &[],
    },
    Mutation {
        name: "triggers-positioned-before-skipped",
        package: Package::Core,
        breaks: "a positioned UPDATE writes its row without its table's BEFORE triggers",
        file: "src/db/cursor.rs",
        original: "let new = effects.before_updated_row(cx, (table, &set_columns(w)), (id, &old, new))?;",
        replacement: "let new = new;",
        integration: &["triggers_firing::positioned_statements_execute_triggers"],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-clause-before-skipped",
        package: Package::Core,
        breaks: "a MERGE clause's rows are written without their BEFORE triggers",
        file: "src/db/merge/apply.rs",
        original: "            before_clause(cx, (merge, clause), plan, effects)?;",
        replacement: "            let _ = before_clause;",
        integration: &["triggers_firing::a_merge_executes_triggers_by_clause"],
        unit: &[],
    },
    Mutation {
        name: "triggers-cause-state-kept",
        package: Package::Core,
        breaks: "a failed triggered action fails its statement with the cause's SQLSTATE, not 09000",
        file: RUN,
        original: "SqlError::new(cause.code, msg).with_state(SqlState::TRIGGERED_ACTION_EXCEPTION)",
        replacement: "SqlError::new(cause.code, msg).with_state(cause.sqlstate())",
        integration: &[FAILED, "triggers_firing::a_recursive_trigger_stops_at_the_limit"],
        unit: &[],
    },
    Mutation {
        name: "triggers-owner-not-pushed",
        package: Package::Core,
        breaks: "a triggered action executes as the session's user, not as its schema's owner",
        file: "src/db/nested.rs",
        original: "auth: self.auth.replace(owner),",
        replacement: "auth: { drop(owner); let current = self.auth.borrow().clone(); self.auth.replace(current) },",
        integration: &["triggers_privileges::an_action_runs_as_the_schema_owner"],
        unit: &[],
    },
    Mutation {
        name: "triggers-depth-too-shallow",
        package: Package::Core,
        breaks: "a chain of ten nested triggered actions fails at the nesting limit",
        file: "src/db/nested.rs",
        original: "MAX_NESTING: usize = 16",
        replacement: "MAX_NESTING: usize = 4",
        integration: &["triggers_firing::a_recursive_trigger_stops_at_the_limit"],
        unit: &[],
    },
    Mutation {
        name: "triggers-instead-of-bypassed",
        package: Package::Core,
        breaks: "a write into a view with an INSTEAD OF trigger is written through the view, not given to the trigger",
        file: "src/db/execute.rs",
        original: "if let Some(n) = self.instead_of(cx, stmt, params)? {",
        replacement: "if let Some(n) = self.instead_of(cx, stmt, params)?.filter(|_| false) {",
        integration: &["triggers_instead::an_insert_into_a_view_runs_its_trigger", "triggers_instead::a_delete_of_a_view_runs_its_trigger"],
        unit: &[],
    },
    Mutation {
        name: "triggers-instead-of-privileges-kept",
        package: Package::Core,
        breaks: "the privileges an INSTEAD OF trigger gave stay after it is dropped",
        file: DEFINE,
        original: "if desc.instead() && self.catalog.view(&desc.table).is_some() {",
        replacement: "if desc.instead() && self.catalog.view(&desc.table).is_none() {",
        integration: &["triggers_instead::an_instead_of_trigger_gives_its_privileges"],
        unit: &[],
    },
    Mutation {
        name: "triggers-column-drop-restrict-ignored",
        package: Package::Core,
        breaks: "dropping a column a trigger uses, RESTRICT, is not refused when the trigger still binds",
        file: DEPENDENTS,
        original: "if !refers(t, &desc.name, &column) && without.trigger_binds(&narrowed).is_ok() {",
        replacement: "if without.trigger_binds(&narrowed).is_ok() {",
        integration: &["triggers_drops::dropping_a_column_a_trigger_uses_needs_cascade"],
        unit: &[],
    },
    Mutation {
        name: "triggers-horizon-ignored",
        package: Package::Core,
        breaks: "a triggered action sees the columns added to its subject after it was defined",
        file: RUN,
        original: "columns.truncate(width);",
        replacement: "let _ = width;",
        integration: &["triggers_drops::an_added_column_changes_no_action"],
        unit: &[],
    },
    Mutation {
        name: "triggers-named-table-drop-restrict-ignored",
        package: Package::Core,
        breaks: "dropping a table a triggered action names, RESTRICT, drops the trigger instead of being refused",
        file: DEPENDENTS,
        original: "if let (Some(named), false) = (naming(t), cascade) {",
        replacement: "if let (Some(named), false) = (naming(t), true) {",
        integration: &["triggers_drops::dropping_what_an_action_names_needs_cascade"],
        unit: &[],
    },
    Mutation {
        name: "triggers-generated-read-allowed",
        package: Package::Core,
        breaks: "a BEFORE trigger's action may read a generated column of the new row",
        file: "src/binder/triggers.rs",
        original: "if transitions.withheld(&q.ident, &cs.column) {",
        replacement: "if transitions.withheld(&q.ident, &cs.column) && false {",
        integration: &[GENERATED],
        unit: &[],
    },
    Mutation {
        name: "triggers-privilege-unchecked",
        package: Package::Core,
        breaks: "a trigger is defined on a table its schema's owner holds no TRIGGER privilege on",
        file: DEFINE,
        original: "if !held.trigger {",
        replacement: "if !held.trigger && false {",
        integration: &["triggers_privileges::a_trigger_needs_the_trigger_privilege"],
        unit: &[],
    },
    Mutation {
        name: "triggers-cause-not-left-out",
        package: Package::Core,
        breaks: "after a triggered action exception MORE is N, as though no condition were left out",
        file: "src/db/area.rs",
        original: "let caused = condition.state == SqlState::TRIGGERED_ACTION_EXCEPTION;",
        replacement: "let caused = condition.state != condition.state;",
        integration: &[FAILED],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-routed-to-none",
        package: Package::Core,
        breaks: "a MERGE into a view finds no INSTEAD OF trigger for any of its clauses",
        file: MERGE_ROUTED,
        original: "stmt.clauses.iter().map(|c| instead_for(st, &name, event_of(c))).collect();",
        replacement: "stmt.clauses.iter().map(|c| instead_for(st, &name, event_of(c)).filter(|_| false)).collect();",
        integration: &[MERGE_VIEW, NESTED],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-routed-clause-written",
        package: Package::Core,
        breaks: "a MERGE clause an INSTEAD OF trigger takes is written through the view instead",
        file: "src/db/merge/apply.rs",
        original: "if let Some(instead) = merge.route(j) {",
        replacement: "if let Some(instead) = merge.route(j).filter(|_| false) {",
        integration: &["triggers_routed::a_merge_into_an_updatable_view_writes_what_no_trigger_takes", NESTED],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-insert-values-lost",
        package: Package::Core,
        breaks: "a MERGE insert an INSTEAD OF trigger takes gives it a row of nulls",
        file: MERGE_ROUTED,
        original: "Some(value) => assign(value.clone(), &c.data_type, STORED),",
        replacement: "Some(_) => Ok(Value::Null),",
        integration: &[MERGE_VIEW],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-routed-generated",
        package: Package::Core,
        breaks: "the new row a MERGE gives an INSTEAD OF trigger has its base table's generated columns computed again",
        file: "src/db/merge/decide.rs",
        original: "if merge.route(clause).is_some() { &[] } else { &bound.generated };",
        replacement: "if merge.route(clause).is_some() && false { &[] } else { &bound.generated };",
        integration: &["triggers_routed::a_trigger_view_row_keeps_its_generated_values"],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-held-rows-one-id",
        package: Package::Core,
        breaks: "every row of a view a MERGE reads for its INSTEAD OF triggers has one row id",
        file: MERGE_ROUTED,
        original: "row.push(located(RowId::try_from(n)",
        replacement: "row.push(located(RowId::try_from(n * 0)",
        integration: &[MERGE_VIEW],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-read-only-view-written",
        package: Package::Core,
        breaks: "a MERGE into a read-only view its triggers take whole is bound through a base table, and refused",
        file: MERGE_ROUTED,
        original: "let held = routes.iter().any(Option::is_some) && !updatable(st, &name)",
        replacement: "let held = routes.iter().any(Option::is_some) && !updatable(st, &name) && false",
        integration: &[MERGE_VIEW, ACTION],
        unit: &[],
    },
    Mutation {
        name: "triggers-merge-before-source-row",
        package: Package::Core,
        breaks: "a MERGE's BEFORE trigger is given other columns of the joined row than the target row's as its old row",
        file: "src/db/merge/apply.rs",
        original: "let target = |joined: &Row| target_part(merge, joined);",
        replacement: "let target = |joined: &Row| target_part(merge, joined).map(|mut r| { r.rotate_left(1); r });",
        integration: &["triggers_firing::a_merge_before_trigger_sees_the_target_row"],
        unit: &[],
    },
    Mutation {
        name: "triggers-positioned-instead-written",
        package: Package::Core,
        breaks: "a positioned change of a view with an INSTEAD OF trigger is written through the view",
        file: "src/db/execute.rs",
        original: "            return self.positioned_instead(cx, at, (&w, &instead, clause));",
        replacement: "            let _ = (instead, clause);",
        integration: &[POSITIONED],
        unit: &[],
    },
    Mutation {
        name: "triggers-nested-change-written",
        package: Package::Core,
        breaks: "an UPDATE or DELETE of a view over one with INSTEAD OF triggers is written through both",
        file: "src/db/execute.rs",
        original: "            return self.searched_instead(cx, w, &instead).map(|n| self.affected(n));",
        replacement: "            let _ = instead;",
        integration: &[NESTED],
        unit: &[],
    },
    Mutation {
        name: "triggers-nested-insert-unmapped",
        package: Package::Core,
        breaks: "an INSERT into a view over one with an INSTEAD OF trigger gives it the outer view's row as its own",
        file: "src/db/triggers/rerouted.rs",
        original: "under.push((id, old, instead.of_named(new, &defaults)))?;",
        replacement: "under.push((id, old, if defaults.is_empty() { new } else { new }))?;",
        integration: &[NESTED],
        unit: &[],
    },
    Mutation {
        name: "triggers-nested-columns-unmapped",
        package: Package::Core,
        breaks: "the columns of a view are taken as those of the view under it, as though in the same order",
        file: ROUTED,
        original: "from_named = from_named.iter().filter_map(|&k| columns.get(k).copied()).collect();",
        replacement: "from_named = from_named.iter().filter_map(|&k| columns.get(k).map(|_| k)).collect();",
        integration: &[NESTED],
        unit: &[],
    },
    Mutation {
        name: "triggers-base-columns-unmapped",
        package: Package::Core,
        breaks: "a view's row is taken from its base table's row as though its columns were the table's, in order",
        file: ROUTED,
        original: "columns = columns.iter().map(|&k| map.get(k).copied()).collect::<Option<_>>()?;",
        replacement: "columns = columns.iter().map(|&k| map.get(k).map(|_| k)).collect::<Option<_>>()?;",
        integration: &[POSITIONED],
        unit: &[],
    },
    Mutation {
        name: "triggers-check-option-over-trigger-view",
        package: Package::Core,
        breaks: "a view WITH CHECK OPTION is defined over a view with an INSTEAD OF trigger",
        file: "src/db/views.rs",
        original: "            self.unchecked_under(&name, under)?;",
        replacement: "            let _ = under;",
        integration: &["triggers_routed::a_view_with_check_option_reads_no_trigger_view"],
        unit: &[],
    },
    Mutation {
        name: "triggers-check-option-read-directly",
        package: Package::Core,
        breaks: "an INSTEAD OF trigger is refused only on a view a view WITH CHECK OPTION reads directly",
        file: DEFINE,
        original: "                if u == *table {",
        replacement: "                if u == *table && w.updates.as_ref().is_some_and(|(d, _)| d == table) {",
        integration: &["triggers_routed::a_view_with_check_option_reads_no_trigger_view"],
        unit: &[],
    },
    Mutation {
        name: "triggers-action-into-trigger-view-refused",
        package: Package::Core,
        breaks: "a triggered action that changes a read-only view with INSTEAD OF triggers is bound through a base table, and refused",
        file: DEFINE,
        original: "if bind_taken(cx, stmt)? {",
        replacement: "if bind_taken(cx, stmt)? && false {",
        integration: &[ACTION],
        unit: &[],
    },
    Mutation {
        name: "triggers-view-rows-hide-the-action",
        package: Package::Core,
        breaks: "a statement of a triggered action changing a view its triggers take sees no transition table of its action",
        file: "src/params/transition.rs",
        original: "None => self.beside.as_ref()?.table(name),",
        replacement: "None => None,",
        integration: &[ACTION],
        unit: &[],
    },
    Mutation {
        name: "triggers-view-rows-hide-the-variables",
        package: Package::Core,
        breaks: "a statement of a triggered action changing a view its triggers take sees no transition variable of its action",
        file: "src/params/transition.rs",
        original: "return self.beside.as_ref()?.variable(variable, column);",
        replacement: "return None;",
        integration: &[ACTION],
        unit: &[],
    },
    Mutation {
        name: "trigger-order-across-levels",
        package: Package::Core,
        breaks: "ACTION_ORDER counts a row trigger among the statement triggers of its table and event",
        file: "src/system/definition/triggers.rs",
        original: "&& o.time == t.time && o.level == t.level",
        replacement: "&& o.time == t.time",
        integration: &["trigger_views::triggers_are_described_5_69"],
        unit: &[],
    },
    Mutation {
        name: "trigger-condition-unkept",
        package: Package::Core,
        breaks: "a trigger's WHEN condition is kept as nothing, so ACTION_CONDITION shows none",
        file: "src/parser/triggers.rs",
        original: "Some((condition, written))",
        replacement: "Some((condition, String::new()))",
        integration: &["trigger_views::triggers_are_described_5_69"],
        unit: &["parser::triggers::tests::the_when_condition_is_kept_as_written"],
    },
];
