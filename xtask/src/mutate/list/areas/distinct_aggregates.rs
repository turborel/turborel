//! The mutations of DISTINCT set functions computed by two groupings
//! (RULES.md R-TEST-9, D340 to D344): the rule's place among the rules,
//! MAX and MIN without DISTINCT, the slots, the marker of each copy, the
//! filter's truth, the copies read again, the copy of the functions computed whole, the parts
//! merged and given, and the bag of values a function took; and, of a part
//! kept in its function's column of states (D1186), what a group gives,
//! what a column sent between threads is, and what it turns into; and,
//! of a part's column the second grouping takes as it is (D1280), that it
//! is taken, which function takes it, and the group each state is merged
//! into.

use super::super::{Mutation, Package};

/// The alike test of the rule, which compares every form of query.
const ALIKE: &str = "distinct_aggregates::distinct_functions_alike_with_and_without_the_rewrite_10_9";

/// A part kept in a column is its accumulator's part.
const PART_COLUMN: &str = "exec::table::states::tests::a_part_kept_in_a_column_is_its_accumulators_part";

/// The parts kept in columns give the results the query has without the rule.
const PARTS_ALIKE: &str = "distinct_aggregates::parts_kept_in_columns_give_the_results_without_the_rule";

/// A second grouping that takes the first's columns gives its values' groups.
const COLUMNS_MERGED: &str = "exec::table::merged::tests::columns_of_parts_merged_as_they_are_give_the_groups_their_values_give";

/// Mechanisms whose removal must fail the DISTINCT set functions'
/// requirements.
pub const DISTINCT_AGGREGATES: [Mutation; 19] = [
    Mutation {
        name: "distinct-aggregates-unregistered",
        package: Package::Core,
        breaks: "the rule is among the built-in rules",
        file: "src/optimizer.rs",
        original: "        o.add(Box::new(DistinctAggregates));",
        replacement: "",
        integration: &[ALIKE, "distinct_aggregates::explain_shows_the_two_groupings"],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-extremes-kept",
        package: Package::Core,
        breaks: "MAX and MIN lose DISTINCT",
        file: "src/optimizer/distinct_aggregates.rs",
        original: "            let aggs = aggs.iter().map(|a| AggCall { distinct: a.distinct && !extreme(a), ..a.clone() }).collect();",
        replacement: "            let aggs = aggs.iter().map(|a| AggCall { distinct: a.distinct, ..a.clone() }).collect();",
        integration: &[],
        unit: &["optimizer::distinct_aggregates::tests::extremes_lose_distinct"],
    },
    Mutation {
        name: "distinct-aggregates-slot-without-filter",
        package: Package::Core,
        breaks: "functions of one argument and different filters keep slots of their own",
        file: "src/optimizer/distinct_aggregates.rs",
        original: "            let key = format!(\"{arg:?} {:?}\", a.filter);",
        replacement: "            let key = format!(\"{arg:?}\");",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-whole-by-state",
        package: Package::Core,
        breaks: "a function writing no state is computed whole, in a copy of its own",
        file: "src/optimizer/distinct_aggregates.rs",
        original: "                    split = Split::Whole;",
        replacement: "                    split = Split::State;",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-marker-unread",
        package: Package::Core,
        breaks: "of more than one slot, a DISTINCT function takes the groups of its own copy alone",
        file: "src/optimizer/distinct_aggregates/levels.rs",
        original: "            (None, Some(_)) => Some(marked(self.width, s.saturating_add(1))),",
        replacement: "            (None, Some(_)) => None,",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-argument-unguarded",
        package: Package::Core,
        breaks: "an argument is computed only of the rows its filter admits",
        file: "src/optimizer/distinct_aggregates/levels.rs",
        original: "            (Some(t), _) => Some(Cond::Value(Expr::Column(layout.used.len().saturating_add(t)))),",
        replacement: "            (Some(_), _) => None,",
        integration: &["distinct_aggregates::arguments_are_computed_only_of_rows_admitted_10_9"],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-whole-of-first-copy",
        package: Package::Core,
        breaks: "functions computed whole take the rows of their own copy, one group of the first grouping",
        file: "src/optimizer/distinct_aggregates/levels.rs",
        original: "    let plain = if shape.copies() > shape.slots.len() { copies } else { 1 };",
        replacement: "    let plain = 1;",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-copy-of-every-slot",
        package: Package::Core,
        breaks: "a copy read again gives the value of its own slot alone",
        file: "src/optimizer/distinct_aggregates/copies.rs",
        original: "            exprs.push(if s.saturating_add(1) == n { slot.arg.clone() } else { Expr::Const(Value::Null) });",
        replacement: "            exprs.push(slot.arg.clone());",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "distinct-aggregates-scan-not-reread",
        package: Package::Core,
        breaks: "a table's scan is read again for each copy",
        file: "src/optimizer/distinct_aggregates/copies.rs",
        original: "        LogicalPlan::Scan { located, .. } => !located,",
        replacement: "        LogicalPlan::Scan { .. } => false,",
        integration: &["distinct_aggregates::explain_shows_the_two_groupings"],
        unit: &[],
    },
    Mutation {
        name: "partial-result-dropped",
        package: Package::Core,
        breaks: "a merge gives the result a part computed whole gave",
        file: "src/functions/partial.rs",
        original: "        self.result.clone().map_or_else(|| self.acc.finish(), Ok)",
        replacement: "        self.acc.finish()",
        integration: &[ALIKE],
        unit: &["functions::partial::tests::parts_merge_by_state_or_give_their_result"],
    },
    Mutation {
        name: "partial-state-dropped",
        package: Package::Core,
        breaks: "a merge takes in each part's state",
        file: "src/functions/partial.rs",
        original: "            Some((&STATE, rest)) => self.acc.merge(rest),",
        replacement: "            Some((&STATE, _)) => Ok(()),",
        integration: &[ALIKE],
        unit: &["functions::partial::tests::parts_merge_by_state_or_give_their_result"],
    },
    Mutation {
        name: "bag-key-always-new",
        package: Package::Core,
        breaks: "a key the bag holds is not added again",
        file: "src/exec/work.rs",
        original: "        Ok(self.change(key, |n| n.max(1))? == 0)",
        replacement: "        Ok(self.change(key, |n| n.max(1)).is_ok())",
        integration: &["distinct_aggregates::window_distinct_functions_keep_their_results", ALIKE],
        unit: &[],
    },
    Mutation {
        name: "group-distinct-key-without-place",
        package: Package::Core,
        breaks: "the values of two DISTINCT functions of one group are told apart by the function",
        file: "src/exec/group.rs",
        original: "                    scratch.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());",
        replacement: "",
        integration: &[ALIKE],
        unit: &[],
    },
    Mutation {
        name: "part-column-gives-result",
        package: Package::Core,
        breaks: "a part kept in a column gives its function's result, not the part the second grouping merges (D1186)",
        file: "src/exec/table/states.rs",
        original: "            States::Part(typed) => parts::finished(typed, g),",
        replacement: "            States::Part(typed) => typed.finish(g),",
        integration: &[PARTS_ALIKE],
        unit: &[PART_COLUMN],
    },
    Mutation {
        name: "part-column-sent-plain",
        package: Package::Core,
        breaks: "a part's column sent to another thread is its function's there, and gives results for parts (D1186)",
        file: "src/exec/table/states.rs",
        original: "            return Ok(if parts::is_part(agg) { States::Part(typed) } else { States::Typed(typed) });",
        replacement: "            return Ok(States::Typed(typed));",
        integration: &[PARTS_ALIKE],
        unit: &[PART_COLUMN],
    },
    Mutation {
        name: "part-column-boxed-bare",
        package: Package::Core,
        breaks: "a part's column turned into accumulators holds its function's, which give results for parts (D1186)",
        file: "src/exec/table/states/parts.rs",
        original: "    typed.to_boxed_by(agg, part_acc)",
        replacement: "    typed.to_boxed_by(agg, |acc| if agg.distinct { part_acc(acc) } else { acc })",
        integration: &[],
        unit: &["exec::table::states::tests::a_parts_column_widens_into_the_parts_accumulators"],
    },
    Mutation {
        name: "part-columns-given-as-values",
        package: Package::Core,
        breaks: "the second grouping of a pair takes a part's column of counts or exact totals as it is (D1280)",
        file: "src/exec/table/merged.rs",
        original: "from.push(part.filter(|&at| self.states.get(at).is_some_and(States::is_merged_whole)));",
        replacement: "from.push(part.filter(|_| false));",
        integration: &[],
        unit: &[COLUMNS_MERGED],
    },
    Mutation {
        name: "part-column-merged-by-first-group",
        package: Package::Core,
        breaks: "a part's state is merged into the group of the second its group of the first fell in (D1280)",
        file: "src/exec/table/merged.rs",
        original: "states.merge_part(part, (&self.pending.groups, &self.pending.read))?,",
        replacement: "states.merge_part(part, (&self.pending.read, &self.pending.read))?,",
        integration: &["regrouped_parts::parts_merged_from_their_columns_give_the_plans_groups"],
        unit: &[COLUMNS_MERGED],
    },
    Mutation {
        name: "part-column-taken-by-any-function",
        package: Package::Core,
        breaks: "a part's column is taken only by a function that merges its parts, and one of approximate sums by none (D1280)",
        file: "src/exec/table/merged.rs",
        original: "let part = part.filter(|at| merges && !from.contains(&Some(*at)));",
        replacement: "let part = part.filter(|at| !from.contains(&Some(*at)));",
        integration: &[],
        unit: &["exec::table::merged::tests::only_a_function_merging_a_part_takes_its_column"],
    },
];
