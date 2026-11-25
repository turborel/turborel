//! The mutations of work a statement does once where it did it for each
//! row or each reference (RULES.md R-TEST-9): SUBSTRING and IN a list of
//! strings evaluated of a vector's text (D1126), a filter's scalar
//! subquery with no outer reference made a constant as its first row is
//! judged (D1127), a subquery run once planned with its parts in parallel
//! (D1128), and a grouping a filter's input and its subqueries compute
//! alike computed once (D1129).

use super::super::{Mutation, Package};

/// Each slice of a vector against the call's.
const SLICES: &str = "functions::strings::slices::tests::a_vector_s_slices_are_its_rows_calls";
/// IN a list of strings of a vector against a row's evaluation.
const LISTED: &str = "exec::batch::expr::tests::in_a_list_of_strings_is_judged_of_a_vector_s_text";
/// A filter holding a subquery, by batch and a row at a time.
const BOUND: &str = "batch_exprs::a_filter_s_subquery_of_no_outer_reference_is_run_once_as_rows_run_it";

/// Mechanisms of the work done once whose removal must fail their tests.
pub const SHARED_WORK: [Mutation; 7] = [
    Mutation {
        name: "substring-vector-slice-end",
        package: Package::Core,
        breaks: "a string's slice over a vector ends at the string's end, not at the end SUBSTRING's length gives (D1126)",
        file: "src/functions/strings/slices.rs",
        original: "        return s.get(index(from)..index(to));",
        replacement: "        return s.get(index(from)..);",
        integration: &[],
        unit: &[SLICES],
    },
    Mutation {
        name: "text-list-unknown-as-false",
        package: Package::Core,
        breaks: "a string not found in a list holding a null is false of a vector's text, not unknown (D1126)",
        file: "src/exec/batch/expr/compare.rs",
        original: "    let found = |s: Option<&str>| s.and_then(|s| set.text_truth(s)).and_then(decided);",
        replacement: "    let found = |s: Option<&str>| s.and_then(|s| set.text_truth(s)).map(|t| t == Truth::True);",
        integration: &[],
        unit: &[LISTED],
    },
    Mutation {
        name: "bound-subquery-loses-not",
        package: Package::Core,
        breaks: "a filter judged with its subquery's value a constant loses the NOT over the comparison holding it (D1127)",
        file: "src/exec/batch/judge/bound.rs",
        original: "        Cond::Not(a) => Cond::Not(Box::new(walked(*a, bind, deeper)?)),",
        replacement: "        Cond::Not(a) => walked(*a, bind, deeper)?,",
        integration: &[BOUND],
        unit: &[],
    },
    Mutation {
        name: "bound-subquery-error-before-its-row-s",
        package: Package::Core,
        breaks: "a subquery that raises an error raises it before the error of an operand before it of the first row (D1127)",
        file: "src/exec/batch/judge.rs",
        original: "            && let Ok(made) = self.bound(late, runner)\n        {\n            return made.in_order_of(batch, rows, runner);\n        }",
        replacement: "        {\n            return match self.bound(late, runner) {\n                Ok(made) => made.in_order_of(batch, rows, runner),\n                Err(e) => (Sel::of(Vec::new()), Some(e)),\n            };\n        }",
        integration: &[BOUND],
        unit: &[],
    },
    Mutation {
        name: "subquery-run-streamed-distinct-whole-first",
        package: Package::Core,
        breaks: "a streamed DISTINCT counts as reading its input whole first, so a run stopping at its first rows reads every row (D1128)",
        file: "src/physical/parallel.rs",
        original: "    matches!(plan, Plan::Aggregate { streamed: false, .. } | Plan::Distinct { streamed: false, .. } | Plan::Sort { .. })",
        replacement: "    matches!(plan, Plan::Aggregate { streamed: false, .. } | Plan::Distinct { .. } | Plan::Sort { .. })",
        integration: &[],
        unit: &["physical::parallel::tests::a_plan_reads_whole_first_under_a_grouping_a_distinct_or_a_sort"],
    },
    Mutation {
        name: "shared-grouping-written-without-its-filter",
        package: Package::Core,
        breaks: "a grouping is written without the condition of a filter below it, so one under another filter is read as the same (D1129)",
        file: "src/exec/shared/written.rs",
        original: "        pure_cond(cond, 0).then(|| write!(out, \"filter({cond:?} \").ok()).flatten()?;",
        replacement: "        pure_cond(cond, 0).then(|| write!(out, \"filter( \").ok()).flatten()?;",
        integration: &[],
        unit: &["exec::shared::tests::groupings_are_written_alike_only_of_the_same_plan"],
    },
    Mutation {
        name: "shared-reader-runs-its-own",
        package: Package::Core,
        breaks: "a reader of a cell another reader filled runs its own subplan, so a shared grouping is computed in each place (D1129)",
        file: "src/exec/shared.rs",
        original: "        if let Some(held) = rows.held()? {\n            return Ok(held);\n        }",
        replacement: "        rows.held()?;",
        integration: &[],
        unit: &["exec::shared::tests::readers_of_a_cell_read_what_the_first_ran"],
    },
];
