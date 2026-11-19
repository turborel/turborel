//! The mutations of spatial joins past one probe per outer row (RULES.md
//! R-TEST-9, D138): spilled partitions that produce a pair in every cell
//! or are never chosen, and traversals of two trees that drop the outer
//! filter or a LEFT join's unpaired rows, record no outer read, take no
//! account of order, cost or the pairs the statistics estimate, or are
//! never offered a tree.

use super::{Mutation, Package};

const SPILLED: &str = "exec::join::indexed::adaptive::tests::outgrown_spatial_builds_are_partitioned_and_spilled";
const EQUAL: &str = "geometry::index::joins::paired::synchronized_spatial_joins_equal_unindexed_products";
const CHOSEN: &str = "geometry::index::joins::paired::synchronized_spatial_joins_are_chosen_by_cost_and_order";
const FEW: &str = "geometry::index::joins::paired::synchronized_spatial_joins_need_few_pairs_by_the_statistics";
const READS: &str = "geometry::index::joins::paired::synchronized_spatial_joins_record_serializable_reads";

/// The mutations of spatial joins, in the order they run.
pub const RTREE_JOINS: [Mutation; 9] = [
    Mutation {
        name: "partition-reference-ignored",
        package: Package::Core,
        breaks: "a pair of shapes spanning cells is produced in each cell both are in",
        file: "src/exec/join/indexed/adaptive/spilled/joined.rs",
        original: "if !m.grid.owns(self.at, *envelope, *bounds) {",
        replacement: "if false && !m.grid.owns(self.at, *envelope, *bounds) {",
        integration: &[],
        unit: &[SPILLED],
    },
    Mutation {
        name: "spilled-build-always-probes",
        package: Package::Core,
        breaks: "an outgrown build goes back to a probe for each outer row",
        file: "src/exec/join/indexed/adaptive/spilled.rs",
        original: "    if scan {\n",
        replacement: "    if scan && false {\n",
        integration: &[],
        unit: &[SPILLED],
    },
    Mutation {
        name: "traversal-outer-filter-dropped",
        package: Package::Core,
        breaks: "a traversal pairs outer rows its operand's filter is not true of",
        file: "src/exec/join/indexed/paired.rs",
        original: "            Some(filter) => eval_cond(filter, &row, runner)? == Truth::True,",
        replacement: "            Some(filter) => eval_cond(filter, &row, runner).is_ok(),",
        integration: &[EQUAL],
        unit: &[],
    },
    Mutation {
        name: "traversal-unpaired-rows-lost",
        package: Package::Core,
        breaks: "a LEFT or FULL traversal drops the outer rows no pair kept",
        file: "src/exec/join/indexed/paired.rs",
        original: "let kept = if join.nulls.is_none() { None } else { Some(Ids::new(db)?) };",
        replacement: "let kept = if join.nulls.is_some() { None } else { Some(Ids::new(db)?) };",
        integration: &[EQUAL],
        unit: &[],
    },
    Mutation {
        name: "traversal-outer-read-unrecorded",
        package: Package::Core,
        breaks: "a serializable traversal misses a new outer row that would pair",
        file: "src/exec/join/indexed/paired.rs",
        original: "        storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;",
        replacement: "        let _ = (&storage, Condition { columns: Vec::new(), probe: Probe::All });",
        integration: &[READS],
        unit: &[],
    },
    Mutation {
        name: "traversal-ignores-order",
        package: Package::Core,
        breaks: "an ordered join takes a traversal, whose pairs are in the trees' order",
        file: "src/physical/indexed/spatial.rs",
        original: "        let mut choice = normal?;\n        choice.spatial &= want.is_empty();\n        choice.paired = choice.paired.filter(|_| want.is_empty());",
        replacement: "        let mut choice = normal?;\n        choice.spatial &= want.is_empty();",
        integration: &[CHOSEN],
        unit: &[],
    },
    Mutation {
        name: "traversal-cost-ignored",
        package: Package::Core,
        breaks: "a join of one outer row descends both trees instead of probing once",
        file: "src/physical/indexed/spatial/paired.rs",
        original: "(self.traversal_cost(&read, choice)? < self.probes_cost(outer, &read, choice)?).then(",
        replacement: "(self.traversal_cost(&read, choice)? >= 0.0_f64).then(",
        integration: &[CHOSEN],
        unit: &[],
    },
    Mutation {
        name: "rtree-envelopes-withheld",
        package: Package::Core,
        breaks: "a geometry index gives no tree, so no join descends two",
        file: "src/index/rtree.rs",
        original: "        Some(Envelopes::new(self.tree.clone(), self.srid?))",
        replacement: "        Some(Envelopes::new(self.tree.clone(), self.srid?)).filter(|_| false)",
        integration: &[EQUAL],
        unit: &[],
    },
    Mutation {
        name: "traversal-pairs-unbounded",
        package: Package::Core,
        breaks: "a join the grids put at many pairs an outer row descends both trees",
        file: "src/physical/indexed/spatial/paired.rs",
        original: "        if each > PAIRED_MOST {",
        replacement: "        if each > PAIRED_MOST * 1000.0_f64 {",
        integration: &[FEW],
        unit: &[],
    },
];
