//! The mutations of the order entry paths (RULES.md R-TEST-9, D413 to
//! D420): MIN and MAX read from the wrong end of the rows sorted, and of a
//! column that may hold a null; a kept body's leaf keeping the plan made
//! with one run's values.

use super::super::{Mutation, Package};

/// Mechanisms of the order entry paths whose removal must fail their tests.
pub const TPCC: [Mutation; 3] = [
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
];
