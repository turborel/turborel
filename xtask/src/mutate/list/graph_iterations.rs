//! The mutations of GRAPH_TABLE's subpaths and iterations (RULES.md
//! R-TEST-9, D108): of a subpath variable, its span, its place outside
//! a quantifier and a path variable declared twice; of a parenthesized
//! WHERE, its set functions' elements, its local exposure, PREV and
//! FIRST; of FOR EACH SEGMENT, its test; of AS OF SCN, its range; and of
//! an inverse distribution function of a group variable, its pick.

use super::{Mutation, Package};

/// The mutations of subpaths and iterations, in the order they run.
pub const GRAPH_ITERATIONS: [Mutation; 10] = [
    Mutation {
        name: "graph-subpath-span-empty",
        package: Package::Core,
        breaks: "a subpath ends where it begins, so its path functions see no edge",
        file: "src/graph/binding.rs",
        original: "        let (from, to) = (at(start)?, at(end)?);",
        replacement: "        let (from, to) = (at(start)?, at(start)?);",
        integration: &["graph_table::subpaths::subpath_variables_6_4"],
        unit: &[],
    },
    Mutation {
        name: "graph-subpath-in-quantifier",
        package: Package::Core,
        breaks: "a subpath variable within a quantifier is taken, its marks joined across iterations",
        file: "src/binder/graph/pattern.rs",
        original: "        if self.depth > 0 {",
        replacement: "        if self.depth > 9 {",
        integration: &["graph_table::subpaths::subpath_rules_6_6"],
        unit: &[],
    },
    Mutation {
        name: "graph-path-declared-twice",
        package: Package::Core,
        breaks: "a path variable declared twice is taken as a join",
        file: "src/parser/graph/syntax.rs",
        original: "        if r.kind == Declared::Path {",
        replacement: "        if r.kind == Declared::Path && false {",
        integration: &["graph_table::subpaths::subpath_rules_6_6"],
        unit: &[],
    },
    Mutation {
        name: "graph-filter-aggregate-of-nothing",
        package: Package::Core,
        breaks: "a set function in a parenthesized WHERE is over no element",
        file: "src/exec/graph_table/shared.rs",
        original: "                Some(GraphValue::Aggregate { call, var }) => self.aggregate(call, (*var, bound.group(id)), &row, &runner)?,",
        replacement: "                Some(GraphValue::Aggregate { call, var }) => self.aggregate(call, (*var, &[]), &row, &runner)?,",
        integration: &["graph_table::subpaths::set_functions_in_a_group_filter_6_8"],
        unit: &[],
    },
    Mutation {
        name: "graph-filter-exposure-global",
        package: Package::Core,
        breaks: "a set function in a WHERE is judged by the degree seen from outside the pattern, so a singleton within an unbounded group is GRF-0032",
        file: "src/binder/graph/columns.rs",
        original: "        let seen = local.and_then(|l| l.get(name)).or_else(|| exposed.get(name));",
        replacement: "        let seen = exposed.get(name);",
        integration: &["graph_table::subpaths::bounded_where_the_filter_reads_7_4"],
        unit: &[],
    },
    Mutation {
        name: "graph-first-kept-as-prev",
        package: Package::Core,
        breaks: "FIRST's register is set as each iteration begins, so it is PREV",
        file: "src/graph/run.rs",
        original: "            if let Some(r) = thread.registers.get_mut(first).filter(|r| r.is_empty()) {",
        replacement: "            if let Some(r) = thread.registers.get_mut(first) {",
        integration: &["graph_table::iterations::first_is_the_first_iteration_11_1"],
        unit: &[],
    },
    Mutation {
        name: "graph-prev-read-as-first",
        package: Package::Core,
        breaks: "PREV and FIRST read each other's register",
        file: "src/exec/graph_table/shared.rs",
        original: "                    let element = if *first { bound.first(id) } else { bound.previous(id) };",
        replacement: "                    let element = if *first { bound.previous(id) } else { bound.first(id) };",
        integration: &["graph_table::iterations::prev_is_the_iteration_before_11_1"],
        unit: &[],
    },
    Mutation {
        name: "graph-segment-ignored",
        package: Package::Core,
        breaks: "a segment that fails FOR EACH SEGMENT's condition keeps the match",
        file: "src/exec/graph_table/values.rs",
        original: "                    return Ok(false);",
        replacement: "                    continue;",
        integration: &["graph_table::iterations::each_segment_11_1"],
        unit: &[],
    },
    Mutation {
        name: "graph-scn-unbounded-below",
        package: Package::Core,
        breaks: "an SCN below 0 is taken as an instant before 0001-01-01",
        file: "src/functions/scn.rs",
        original: "        let utc = i64::try_from(d.units).ok().filter(|u| (0..=LAST_INSTANT).contains(u));",
        replacement: "        let utc = i64::try_from(d.units).ok().filter(|u| (i64::MIN..=LAST_INSTANT).contains(u));",
        integration: &["graph_table::reads::read_as_of_an_scn_8"],
        unit: &[],
    },
    Mutation {
        name: "graph-distribution-always-continuous",
        package: Package::Core,
        breaks: "PERCENTILE_DISC of a group variable picks the value below the fraction, as PERCENTILE_CONT does",
        file: "src/exec/graph_table/values.rs",
        original: "    let Some(chosen) = picked(d.continuous, &fraction, n)? else { return Ok(Value::Null) };",
        replacement: "    let Some(chosen) = picked(true, &fraction, n)? else { return Ok(Value::Null) };",
        integration: &["graph_table::iterations::inverse_distribution_of_a_group_4_4"],
        unit: &[],
    },
];
