//! The mutations of the order entry paths (RULES.md R-TEST-9, D413 to
//! D420): MIN and MAX read from the wrong end of the rows sorted, and of a
//! column that may hold a null; a kept body's leaf keeping the plan made
//! with one run's values; a table changed keeping what the catalog knew
//! of its constraints, a row's checks reading the row of another, and a
//! kept body's cursors opening from one cursor's plan; a write giving
//! the row it wrote as the row it replaced; a kept hash join's inner
//! input left unfilled; a node's edges not acted on; a row drawing a
//! generator's next value evaluated as one that reads nothing; a short
//! fixed-length string cast without its padding; a deferrable row
//! constraint checked at once; a kept write keeping the plan one run
//! made with its values.

use super::super::{Mutation, Package};

/// Mechanisms of the order entry paths whose removal must fail their tests.
pub const TPCC: [Mutation; 13] = [
    Mutation {
        name: "tpcc-extreme-wrong-end",
        package: Package::Core,
        breaks: "MIN reads the last row sorted and MAX the first",
        file: "src/optimizer/extremes.rs",
        original: "        let key = SortKey::new(column, SortOrder::of(descending, None));",
        replacement: "        let key = SortKey::new(column, SortOrder::of(!descending, None));",
        integration: &["rewrites::extremes::extremes_are_read_from_the_first_row"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-extreme-of-nulls",
        package: Package::Core,
        breaks: "MIN and MAX of a column that may hold a null are read from the first row, losing the warning of its nulls eliminated",
        file: "src/optimizer/extremes.rs",
        original: "        if !ctx.db.catalog.table(table)?.columns.get(column)?.not_null {",
        replacement: "        if ctx.db.catalog.table(table)?.columns.get(column)?.data_type == DataType::Boolean {",
        integration: &["rewrites::extremes::extremes_are_read_from_the_first_row"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-leaf-learns-values",
        package: Package::Core,
        breaks: "a kept body's leaf keeps the plan one run made with its values, not the one with its slots",
        file: "src/db/routines/kept/leaves.rs",
        original: "            Learned::Kept(slotted) => match prepared_plan(*slotted, crate::optimizer::prepared::budget(st)) {",
        replacement: "            Learned::Kept(_) => match prepared_plan(custom.clone(), crate::optimizer::prepared::budget(st)) {",
        integration: &["kept_routines::a_kept_leaf_learns_its_plan_and_reads_each_calls_rows"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-constrained-kept-over-change",
        package: Package::Core,
        breaks: "a table changed by ALTER TABLE keeps what the catalog knew of foreign keys, so one added is never checked",
        file: "src/catalog/memory.rs",
        original: "            self.table_order = OnceLock::new();\n        }\n        self.constrained = OnceLock::new();",
        replacement: "            self.table_order = OnceLock::new();\n        }",
        integration: &["statement_ends::constraints_added_after_statements_ended_without_any_are_checked"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-check-reads-another-row",
        package: Package::Core,
        breaks: "a row's checks reuse the row the last check of the table read, of another row",
        file: "src/db/integrity.rs",
        original: "if read.as_ref().is_none_or(|(t, at, _)| t != table || *at != id) {",
        replacement: "if read.as_ref().is_none_or(|(t, _, _)| t != table) {",
        integration: &["statement_ends::each_row_is_checked_of_its_own_values"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-cursors-share-plans",
        package: Package::Core,
        breaks: "every local cursor of a kept body opens from the first cursor's specification and plan",
        file: "src/db/routines/kept.rs",
        original: "        self.cursors.get(at)\n",
        replacement: "        self.cursors.get(at.min(0))\n",
        integration: &["kept_routines::a_kept_bodys_cursors_read_each_calls_rows"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-replaced-is-new",
        package: Package::Core,
        breaks: "an update gives the row it wrote as the row it replaced, so undo, triggers and indexes see the new row",
        file: "src/engine/rows/mvcc/write.rs",
        original: "        self.counted_change(tx);\n        self.replaced(id, replaced.as_deref())\n    }",
        replacement: "        self.counted_change(tx);\n        Ok(row)\n    }",
        integration: &["replaced_rows::each_write_replaces_the_row_it_found"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-join-right-unfilled",
        package: Package::Core,
        breaks: "a kept hash join runs its inner input with its slots unfilled",
        file: "src/physical/kept/filled.rs",
        original: "return Ok(Plan::HashJoin { left: self.boxed(*left)?, right: self.boxed(*right)?, keys, detail, est });",
        replacement: "return Ok(Plan::HashJoin { left: self.boxed(*left)?, right, keys, detail, est });",
        integration: &["kept_routines::a_kept_join_learns_its_plan_and_reads_each_calls_row"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-nodes-not-looked-for",
        package: Package::Core,
        breaks: "a statement's end skips referential actions where no table has a foreign key, though a node deleted has edges",
        file: "src/db/integrity/actions.rs",
        original: "        if !constrained.foreign_keys && !constrained.nodes {",
        replacement: "        if !constrained.foreign_keys {",
        integration: &["graph_native::statements::a_node_deleted_takes_or_keeps_its_edges_r12_1"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-next-value-reads-nothing",
        package: Package::Core,
        breaks: "a row of VALUES drawing a generator's next value is evaluated alone, each name drawing its own value",
        file: "src/logical/invoked.rs",
        original: "if matches!(e, Expr::Subquery(..) | Expr::Invoked(_) | Expr::NextValue(_)) {",
        replacement: "if matches!(e, Expr::Subquery(..) | Expr::Invoked(_)) {",
        integration: &["sequences::one_value_a_row_6_14"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-char-cast-unpadded",
        package: Package::Core,
        breaks: "a CHARACTER value shorter than its type is cast as it is, not padded to the length",
        file: "src/value/types/character.rs",
        original: "if units.count(&s) == usize::try_from(length).unwrap_or(usize::MAX) {",
        replacement: "if units.count(&s) <= usize::try_from(length).unwrap_or(usize::MAX) {",
        integration: &["rows::cast_and_assigned_6_13"],
        unit: &[],
    },
    Mutation {
        name: "tpcc-deferrable-checked-at-once",
        package: Package::Core,
        breaks: "a row's deferrable constraint is checked as the statement's end gathers it, not when deferred",
        file: "src/db/integrity/gather.rs",
        original: "out.is_empty() && !c.attributes.deferrable",
        replacement: "out.is_empty()",
        integration: &[
            "deferred::deferred_constraints_wait_for_commit_10_6",
            "statement_ends::immediate_row_checks_come_around_deferred_ones",
        ],
        unit: &[],
    },
    Mutation {
        name: "tpcc-write-learns-values",
        package: Package::Core,
        breaks: "a kept write keeps the plan one run made with its values, not the one with its slots",
        file: "src/db/prepared/plans.rs",
        original: "let learned = was.after(&plan, super::super::plan_rows_slotted(self.st, w.rows.clone(), values));",
        replacement: "let learned = was.after(&plan, plan.clone());",
        integration: &["kept_routines::a_kept_write_learns_its_plan_and_changes_each_calls_rows"],
        unit: &[],
    },
];
