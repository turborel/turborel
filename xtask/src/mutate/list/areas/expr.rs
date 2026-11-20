//! The mutations of expressions over vectors (RULES.md R-TEST-9, D362,
//! D363): the flag of a batch's integer arithmetic, its rows null, the
//! runs a result keeps, three-valued OR, the bits of a null truth value,
//! and the rows a CASE's WHEN leaves to the next.

use super::super::{Mutation, Package};

/// The expressions given by batch and by rows, of either engine.
const EXPRESSIONS: &str = "batch_exprs::expressions_give_by_batch_what_rows_give";

/// Mechanisms whose removal must fail the requirements of expressions
/// over vectors.
pub const EXPR: [Mutation; 6] = [
    Mutation {
        name: "expr-flag-ignored",
        package: Package::Core,
        breaks: "a batch whose integer arithmetic overflows gives its wrapped integers",
        file: "src/exec/batch/expr/ints.rs",
        original: "    if flag {\n        return None;\n    }\n    let made",
        replacement: "    if flag && units.is_empty() {\n        return None;\n    }\n    let made",
        integration: &["batch_exprs::raising_expressions_raise_as_rows_do"],
        unit: &["exec::batch::expr::ints::tests::the_flag_counts_rows_not_null"],
    },
    Mutation {
        name: "expr-flag-of-nulls",
        package: Package::Core,
        breaks: "an overflow of a null row's garbage slot sends the batch to rows",
        file: "src/exec/batch/expr/ints.rs",
        original: "flag = (0..count).any(|n| !nulls.get(n) && apply(left.unit(n), right.unit(n)).1);",
        replacement: "flag = (0..count).any(|n| apply(left.unit(n), right.unit(n)).1);",
        integration: &[],
        unit: &["exec::batch::expr::ints::tests::the_flag_counts_rows_not_null"],
    },
    Mutation {
        name: "expr-runs-of-one",
        package: Package::Core,
        breaks: "a sum of two vectors in runs keeps the runs of the first alone",
        file: "src/exec/batch/expr/ints.rs",
        original: "            first.extend(second);",
        replacement: "            drop(second);",
        integration: &["batches::columns_of_runs_answer_as_row_by_row"],
        unit: &["exec::batch::expr::ints::tests::runs_of_both_are_kept"],
    },
    Mutation {
        name: "expr-or-of-unknown",
        package: Package::Core,
        breaks: "OR of unknown and true is unknown",
        file: "src/exec/batch/expr/logic.rs",
        original: "    let unknown = either(a, b).map(|u| u.and_not(&true_of));",
        replacement: "    let unknown = either(a, b);",
        integration: &[EXPRESSIONS],
        unit: &["exec::batch::expr::tests::nan_compares_unknown_and_logic_is_three_valued"],
    },
    Mutation {
        name: "expr-null-truth-bits",
        package: Package::Core,
        breaks: "a null truth value whose bit is set reads true",
        file: "src/exec/batch/expr/logic.rs",
        original: "    let bits = unknown.as_ref().map_or_else(|| bits.clone(), |u| bits.and_not(u));",
        replacement: "    let bits = bits.clone();",
        integration: &[],
        unit: &["exec::batch::expr::tests::null_truth_values_read_unknown_whatever_their_bit"],
    },
    Mutation {
        name: "expr-case-rows-not-left",
        package: Package::Core,
        breaks: "a CASE's later WHENs and its ELSE take the rows an earlier WHEN took",
        file: "src/exec/batch/expr/case.rs",
        original: "        left = rest;",
        replacement: "        drop(rest);",
        integration: &[EXPRESSIONS],
        unit: &["exec::batch::expr::tests::overflows_give_none_and_untaken_branches_none"],
    },
];
