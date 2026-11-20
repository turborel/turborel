//! The mutations of the order entry paths (RULES.md R-TEST-9, D413 to
//! D420): MIN and MAX read from the wrong end of the rows sorted, and of a
//! column that may hold a null; a kept body's leaf keeping the plan made
//! with one run's values; a table changed keeping what the catalog knew
//! of its constraints, a row's checks reading the row of another, and a
//! kept body's cursors opening from one cursor's plan; a write giving
//! the row it wrote as the row it replaced.

use super::super::{Mutation, Package};

/// Mechanisms of the order entry paths whose removal must fail their tests.
pub const TPCC: [Mutation; 7] = [
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
        original: "            Learned::Kept(slotted) => Physical::Fixed(Arc::from(slotted), Arc::clone(&reads), Some(rows_of(st, &reads))),",
        replacement: "            Learned::Kept(_) => Physical::Fixed(Arc::new(custom.clone()), Arc::clone(&reads), Some(rows_of(st, &reads))),",
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
];
