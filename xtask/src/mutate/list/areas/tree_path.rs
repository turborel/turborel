//! The mutations of a B-link tree probe's path (RULES.md R-TEST-9, D840,
//! D852, D853, D854, D855):
//! the search over a node's slots, inner copies kept with their headers
//! parsed, writers that find their leaf from its parent and change it
//! through the frame they own, under one latch, from the leaf noted for a
//! row, their page pinned through its hint, paths kept in place, and
//! scans' buffers and bounds.

use super::super::{Mutation, Package};

/// The concurrent writers of the tree's paths, splitting and merging.
const THREADS: &str = "tree_paths::writers_through_owned_frames_split_and_merge_under_readers";

/// Mechanisms of a probe's path whose removal must fail their tests.
pub const TREE_PATH: [Mutation; 12] = [
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
    Mutation {
        name: "tree-here-covers",
        package: Package::Core,
        breaks: "a writer searches the leaf it owns though the key moved right of it with a split, and finds no entry there",
        file: "src/engine/tree/here.rs",
        original: "    if node.deleted || !node.covers(key) {",
        replacement: "    if node.deleted {",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_replace_starts_at_the_leaf_a_read_left"],
    },
    Mutation {
        name: "tree-here-compares",
        package: Package::Core,
        breaks: "a replace of a value expected compares only its length, so it writes over a value changed since it was read",
        file: "src/engine/tree/here.rs",
        original: "            if node.value(at)? == expected {",
        replacement: "            if node.value(at)?.len() == expected.len() {",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_replace_starts_at_the_leaf_a_read_left"],
    },
    Mutation {
        name: "tree-noted-for-write",
        package: Package::Core,
        breaks: "a row read for a write no longer starts at the leaf noted for it, and descends from the root",
        file: "src/engine/tree/write.rs",
        original: "        if let Some((leaf, value)) = self.from_noted(key, |leaf| value_of(leaf, key))? {",
        replacement: "        if let Some((leaf, value)) = None::<(u64, Option<Vec<u8>>)> {",
        integration: &[],
        unit: &["engine::tree::tests::hinted::a_read_for_a_write_starts_at_the_leaf_noted"],
    },
    Mutation {
        name: "pool-taken-pinned",
        package: Package::Core,
        breaks: "a frame latched to be taken from its page is taken though pinned under that latch, through its hint",
        file: "src/disk/buffer/hits.rs",
        original: "        if self.pins.load(Ordering::Acquire) > 0 {\n            return Err(Kept::Pinned);\n        }\n",
        replacement: "",
        integration: &[],
        unit: &["disk::buffer::hits::tests::a_page_pinned_through_its_hint_keeps_its_frame"],
    },
    Mutation {
        name: "pool-pin-hinted-page",
        package: Package::Core,
        breaks: "a hold pins the frame its hint names though the frame holds another page now",
        file: "src/disk/buffer/hits.rs",
        original: "            if frame.loading.load(Ordering::Acquire) || frame.key() != Some(key) {\n                return None;\n            }\n            frame.pins",
        replacement: "            if frame.loading.load(Ordering::Acquire) {\n                return None;\n            }\n            frame.pins",
        integration: &[],
        unit: &["disk::buffer::hits::tests::a_page_pinned_through_its_hint_keeps_its_frame"],
    },
];
