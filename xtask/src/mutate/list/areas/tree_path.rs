//! The mutations of a B-link tree probe's path (RULES.md R-TEST-9, D839):
//! the search over a node's slots, inner copies kept with their headers
//! parsed, writers that find their leaf from its parent and change it
//! through the frame they own, paths kept in place, and scans' buffers
//! and bounds.

use super::super::{Mutation, Package};

/// The concurrent writers of the tree's paths, splitting and merging.
const THREADS: &str = "tree_paths::writers_through_owned_frames_split_and_merge_under_readers";

/// Mechanisms of a probe's path whose removal must fail their tests.
pub const TREE_PATH: [Mutation; 7] = [
    Mutation {
        name: "tree-path-slot-head",
        package: Package::Core,
        breaks: "a node's search reads its slots' heads in the wrong byte order, so heads no longer order as their keys",
        file: "src/engine/tree/node.rs",
        original: "u16::from_be_bytes([h0, h1])",
        replacement: "u16::from_le_bytes([h0, h1])",
        integration: &[],
        unit: &["engine::tree::node::tests::search::a_search_over_the_slots_places_keys_as_the_entries_do"],
    },
    Mutation {
        name: "tree-path-copy-parsed",
        package: Package::Core,
        breaks: "a copy put in another's place keeps the header parsed of the page it replaced",
        file: "src/engine/tree/inner.rs",
        original: "(old.file, old.id, old.stamp, old.parsed, old.used) = (file, id, stamp, parsed, true);",
        replacement: "(old.file, old.id, old.stamp, old.used) = (file, id, stamp, true);",
        integration: &[],
        unit: &["engine::tree::tests::copies::copies_read_since_the_hand_passed_are_kept"],
    },
    Mutation {
        name: "tree-path-leaf-from-parent",
        package: Package::Core,
        breaks: "a writer locks the parent it found its leaf in rather than the leaf, and writes the entry there",
        file: "src/engine/tree/write.rs",
        original: "                path.stack.push(parent);\n                child\n",
        replacement: "                path.stack.push(parent);\n                parent\n",
        integration: &[THREADS, "trees::trees_grow_levels_and_keep_order"],
        unit: &[],
    },
    Mutation {
        name: "tree-path-owned-write-clean",
        package: Package::Core,
        breaks: "a write through the frame a writer owns leaves the page clean, so it is dropped unwritten when its frame is wanted",
        file: "src/disk/buffer/owned.rs",
        original: "        Ok(self.held_for_writing(frame))",
        replacement: "        Ok(PageMut { content: frame.latched(), _pin: Pin(frame) })",
        integration: &[THREADS, "tree_paths::trees_changed_through_owned_frames_recover_after_checkpoints_and_a_crash"],
        unit: &[],
    },
    Mutation {
        name: "tree-path-levels-top",
        package: Package::Core,
        breaks: "a path taller than it keeps writes its lowest node over the one before, not giving up its top",
        file: "src/engine/tree/levels.rs",
        original: "            self.nodes.rotate_left(1);\n",
        replacement: "",
        integration: &[],
        unit: &["engine::tree::levels::tests::a_path_keeps_its_lowest_levels"],
    },
    Mutation {
        name: "tree-path-ready-uncleared",
        package: Package::Core,
        breaks: "buffers a scan gives back keep its entries, so the thread's next scan gives them again",
        file: "src/engine/tree/scan/ready.rs",
        original: "        self.clear();\n        SPARE.with",
        replacement: "        SPARE.with",
        integration: &[],
        unit: &["engine::tree::scan::ready::tests::buffers_given_back_are_reused"],
    },
    Mutation {
        name: "tree-path-low-forward",
        package: Package::Core,
        breaks: "a backward scan no longer keeps the key below its leaf on the way down, so it stops after one leaf",
        file: "src/engine/tree/scan.rs",
        original: "let track_low = matches!(self.direction, Direction::Backward);",
        replacement: "let track_low = matches!(self.direction, Direction::Forward);",
        integration: &["trees::scans_honour_bounds_in_both_directions"],
        unit: &[],
    },
];
