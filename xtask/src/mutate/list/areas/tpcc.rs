//! The mutations of the order entry paths (RULES.md R-TEST-9, D413 to
//! D420): MIN and MAX read from the wrong end of the rows sorted, and of a
//! column that may hold a null.

use super::super::{Mutation, Package};

/// Mechanisms of the order entry paths whose removal must fail their tests.
pub const TPCC: [Mutation; 2] = [
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
];
