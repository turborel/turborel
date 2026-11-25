//! The mutations of work a statement does once where it did it for each
//! row or each reference (RULES.md R-TEST-9): SUBSTRING and IN a list of
//! strings evaluated of a vector's text (D1126), a filter's scalar
//! subquery with no outer reference made a constant as its first row is
//! judged (D1127), a subquery run once planned with its parts in parallel
//! (D1128), and a grouping a filter's input and its subqueries compute
//! alike computed once (D1129), a chunk's texts judged UTF-8 once and
//! strings looked for among a list's in order (D1131), and a filter's
//! quiet scalar subqueries bound before a part's morsels (D1132).

use super::super::{Mutation, Package};

/// Each slice of a vector against the call's.
const SLICES: &str = "functions::strings::slices::tests::a_vector_s_slices_are_its_rows_calls";
/// IN a list of strings of a vector against a row's evaluation.
const LISTED: &str = "exec::batch::expr::tests::in_a_list_of_strings_is_judged_of_a_vector_s_text";
/// A filter holding a subquery, by batch and a row at a time.
const BOUND: &str = "batch_exprs::a_filter_s_subquery_of_no_outer_reference_is_run_once_as_rows_run_it";

/// The judgment of a set function's nulls and of what raises.
const QUIET_NULLS: &str = "optimizer::unnesting::quiet::tests::a_set_function_of_a_filled_column_is_quiet_and_one_of_nulls_is_not";
/// The judgment of what raises.
const QUIET_RAISES: &str = "optimizer::unnesting::quiet::tests::a_subquery_that_may_raise_is_not_quiet";
/// A quiet subquery's filter in a part, the others serial.
const QUIET_PART: &str = "optimizer::unnesting::quiet::tests::a_filter_of_a_quiet_subquery_runs_in_parallel_and_others_do_not";

/// A quiet subquery bound as a pipeline is made.
const SETTLED: &str = "exec::parallel::pipeline::tests::a_quiet_subquery_is_bound_before_the_morsels";
/// A quiet subquery joined only to probe an index.
const UNJOINED: &str = "optimizer::unnesting::quiet::tests::a_quiet_subquery_is_joined_only_to_probe_an_index";
/// SUBSTRING's bounds judged.
const SUBSTRING_FREE: &str = "logical::raises::tests::a_substring_of_constant_bounds_raises_nothing";

/// Mechanisms of the work done once whose removal must fail their tests.
pub const SHARED_WORK: [Mutation; 17] = [
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
    Mutation {
        name: "texts-span-unshifted",
        package: Package::Core,
        breaks: "a field of a chunk's texts judged UTF-8 at once is cut from the octets it begins at, past the range's start (D1131)",
        file: "src/engine/columns/encoded.rs",
        original: "            padded(text, span.get(a.checked_sub(start)?..b.checked_sub(start)?)?, pad);",
        replacement: "            padded(text, span.get(a..b)?, pad);",
        integration: &[],
        unit: &["engine::columns::tests::chunks_give_their_texts"],
    },
    Mutation {
        name: "text-list-unsorted",
        package: Package::Core,
        breaks: "a list's strings are looked for by halving them only once they are in order (D1131)",
        file: "src/logical/sets.rs",
        original: "            texts.sort_unstable();\n",
        replacement: "",
        integration: &[],
        unit: &[LISTED],
    },
    Mutation {
        name: "quiet-nulls-unjudged",
        package: Package::Core,
        breaks: "a set function of a column that may hold a null is taken to warn of nothing (D1132)",
        file: "src/optimizer/unnesting/quiet.rs",
        original: "                Some(Expr::Column(c)) => filled(input, *c, catalog, 0),",
        replacement: "                Some(Expr::Column(_)) => true,",
        integration: &[],
        unit: &[QUIET_NULLS],
    },
    Mutation {
        name: "quiet-raises-unjudged",
        package: Package::Core,
        breaks: "a subquery whose plan may raise is taken to raise nothing (D1132)",
        file: "src/optimizer/unnesting/quiet.rs",
        original: "    silent && !plan_raises(&s.plan, &RuleContext { db })",
        replacement: "    silent && plan_raises(&s.plan, &RuleContext { db }) == plan_raises(&s.plan, &RuleContext { db })",
        integration: &[],
        unit: &[QUIET_RAISES],
    },
    Mutation {
        name: "quiet-rows-unjudged",
        package: Package::Core,
        breaks: "a subquery that may give two rows is taken to raise nothing (D1132)",
        file: "src/optimizer/unnesting/quiet.rs",
        original: "s.outer.is_empty() && s.plan.one_row_at_most() && ",
        replacement: "s.outer.is_empty() && ",
        integration: &[],
        unit: &[QUIET_RAISES],
    },
    Mutation {
        name: "quiet-filter-serial",
        package: Package::Core,
        breaks: "a filter of quiet subqueries stays off the part below it (D1132)",
        file: "src/physical/parallel.rs",
        original: "        let calm = calm_cond(cond) || db.is_some_and(|db| bound_calm(cond, db));",
        replacement: "        let calm = calm_cond(cond) || db.is_some_and(|db| bound_calm(cond, db) && false);",
        integration: &[],
        unit: &[QUIET_PART],
    },
    Mutation {
        name: "quiet-filter-nulls-unjudged",
        package: Package::Core,
        breaks: "a column a filter below a set function compares is taken to may hold a null (D1132)",
        file: "src/optimizer/unnesting/quiet.rs",
        original: "        return conjuncts.iter().any(|c| rejects(c, column)) || filled(input, column, catalog, deeper);",
        replacement: "        return filled(input, column, catalog, deeper);",
        integration: &[],
        unit: &[QUIET_NULLS],
    },
    Mutation {
        name: "substring-length-unjudged",
        package: Package::Core,
        breaks: "a SUBSTRING of a negative constant length is taken to raise nothing (D1132)",
        file: "src/functions/strings.rs",
        original: "        [c, from, length] => string(c) && whole(from, i128::MIN) && whole(length, 0),",
        replacement: "        [c, from, length] => string(c) && whole(from, i128::MIN) && whole(length, i128::MIN),",
        integration: &[],
        unit: &[SUBSTRING_FREE],
    },
    Mutation {
        name: "pipeline-subquery-unbound",
        package: Package::Core,
        breaks: "a filter's quiet subquery is left for each morsel to run, not bound once before them (D1132)",
        file: "src/exec/parallel/pipeline.rs",
        original: "            return eval(&e, &none, &runner).map(Expr::Const);",
        replacement: "            return Ok(e);",
        integration: &[],
        unit: &[SETTLED],
    },
    Mutation {
        name: "quiet-subquery-joined",
        package: Package::Core,
        breaks: "a comparison with a quiet subquery of no index to probe is made a semi join (D1132)",
        file: "src/optimizer/unnesting/scalar.rs",
        original: "    uses_all && (!super::quiet(sub, ctx.db) || probed())",
        replacement: "    uses_all",
        integration: &[],
        unit: &[UNJOINED],
    },
];
