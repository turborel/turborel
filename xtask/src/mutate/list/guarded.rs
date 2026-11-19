//! Mutations of the read of a table below the guard its raising
//! conjuncts are moved over: the guard left out of the read, and the read
//! not below it (RULES.md R-TEST-9, D15).

use super::{Mutation, Package};

/// Each mechanism broken alone, and the tests that fail.
pub const GUARDED: [Mutation; 2] = [
    Mutation {
        name: "guard-left-out",
        package: Package::Core,
        breaks: "an index read below a guard is no product with it, so its conjuncts are evaluated of a table whose guard has no row",
        file: "src/physical/access/guarded.rs",
        original: "            Some(guard) => Plan::Product { left: Box::new(plan), right: Box::new(self.build_node(guard, &[]).plan) },",
        replacement: "            Some(_) => plan,",
        integration: &["guarded_reads::errors_below_a_guard_are_a_full_reads"],
        unit: &[],
    },
    Mutation {
        name: "guard-not-read-below",
        package: Package::Core,
        breaks: "a filter over a table's read and a guard reads no index",
        file: "src/physical/access/guarded.rs",
        original: "        self.guarded_access(below, conds, (want, guard))",
        replacement: "        self.guarded_access(below, conds, (want, guard)).filter(|_| false)",
        integration: &[
            "guarded_reads::raising_conditions_read_by_an_index_below_their_guard",
            "guarded_reads::any_method_is_read_below_a_guard",
        ],
        unit: &[],
    },
];
