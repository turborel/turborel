//! The mutations of DISTINCT set functions computed by two groupings
//! (RULES.md R-TEST-9, D340 to D344): the rule's place among the rules,
//! MAX and MIN without DISTINCT, the slots, the marker of each copy, the
//! filter's truth, the copy of the functions computed whole, the parts
//! merged and given, and the bag of values a function took.

use super::super::{Mutation, Package};

/// The alike test of the rule, which compares every form of query.
const ALIKE: &str = "distinct_aggregates::distinct_functions_alike_with_and_without_the_rewrite_10_9";

/// Mechanisms whose removal must fail the DISTINCT set functions'
/// requirements.
pub const DISTINCT_AGGREGATES: [Mutation; 11] = [
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
];
