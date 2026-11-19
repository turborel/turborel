//! Mutations of R-tree packing and reclamation (RULES.md R-TEST-9, D139):
//! leaves filled past their fill, duplicate counts lost, the writers'
//! entries dropped by a load, sparse leaves kept, merged leaves emptied
//! under a scan, retired pages freed while scans hold them, and split
//! stamps repeated.

use super::{Mutation, Package};

const PACKED: &str = "engine::rtree::tests::pack::packed_and_inserted_trees_answer_alike";
const PAUSED: &str = "engine::rtree::tests::reclaim::a_paused_scan_survives_reclamation";

/// The mutations of packing and reclamation, in the order they run.
pub const RTREE_PACK: [Mutation; 7] = [
    Mutation {
        name: "rtree-pack-fills-full",
        package: Package::Core,
        breaks: "packed nodes are filled to capacity, so the first inserts split them",
        file: "src/engine/rtree/pack.rs",
        original: "const FILL: (usize, usize) = (9, 10);",
        replacement: "const FILL: (usize, usize) = (10, 10);",
        integration: &[],
        unit: &[PACKED],
    },
    Mutation {
        name: "rtree-pack-drops-counts",
        package: Package::Core,
        breaks: "an entry loaded twice keeps one reference, so one remove takes it out",
        file: "src/engine/rtree/pack.rs",
        original: "entry.value = entry.value.saturating_add(added);",
        replacement: "entry.value = entry.value.max(added);",
        integration: &[],
        unit: &["engine::rtree::tests::pack::duplicate_loaded_entries_are_counted"],
    },
    Mutation {
        name: "rtree-load-drops-writers",
        package: Package::Core,
        breaks: "a load replaces the tree without the entries writers made meanwhile",
        file: "src/engine/rtree/pack.rs",
        original: "            if node.level == 0 {\n                node.entries",
        replacement: "            if node.level == u8::MAX {\n                node.entries",
        integration: &[],
        unit: &["engine::rtree::tests::pack::a_load_keeps_the_writers_entries"],
    },
    Mutation {
        name: "rtree-sparse-leaves-kept",
        package: Package::Core,
        breaks: "removes never take out or merge a leaf, so deleted space is never reused",
        file: "src/engine/rtree/reclaim.rs",
        original: "if count >= self.minimum_fill() {",
        replacement: "if count >= self.minimum_fill().min(0) {",
        integration: &[],
        unit: &["engine::rtree::tests::reclaim::emptied_and_sparse_leaves_are_taken_out"],
    },
    Mutation {
        name: "rtree-merge-moves-entries",
        package: Package::Core,
        breaks: "a merge empties the old leaves, so a scan holding them misses their entries",
        file: "src/engine/rtree/reclaim.rs",
        original: "            held.node.retired = true;\n",
        replacement: "            held.node.retired = true;\n            held.node.entries.clear();\n",
        integration: &[],
        unit: &[PAUSED],
    },
    Mutation {
        name: "rtree-retired-freed-at-once",
        package: Package::Core,
        breaks: "retired pages are freed while a scan may still read them",
        file: "src/engine/rtree/reclaim.rs",
        original: "for page in self.inner.epochs.freeable() {",
        replacement: "for page in self.inner.epochs.all_retired() {",
        integration: &[],
        unit: &[PAUSED],
    },
    Mutation {
        name: "rtree-stamps-repeat",
        package: Package::Core,
        breaks: "every split reuses one stamp, so a scan stops before entries a split moved",
        file: "src/engine/rtree.rs",
        original: "meta.next = stamp.checked_add(1)",
        replacement: "meta.next = stamp.checked_add(0)",
        integration: &[],
        unit: &["engine::rtree::tests::reclaim::split_stamps_never_repeat"],
    },
];
