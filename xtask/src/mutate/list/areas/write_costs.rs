//! The mutations of the fixed costs cut from a write and its commit
//! (RULES.md R-TEST-9): an entry inserted into the room made for it in a
//! node, and the hints bounding a search found by binary search (D847).

use super::super::{Mutation, Package};

/// Every key placed by a search as the entries one by one place it.
const SEARCHED: &str = "engine::tree::node::tests::search::a_search_over_the_slots_places_keys_as_the_entries_do";
/// Entries inserted in place read back in key order.
const INSERTED: &str = "engine::tree::node::tests::search::entries_inserted_in_place_read_back_in_key_order";

/// Mechanisms of the cuts whose removal must fail their tests.
pub const WRITE_COSTS: [Mutation; 2] = [
    Mutation {
        name: "write-costs-hints-through",
        package: Package::Core,
        breaks: "the hints bounding a search stop before one equal to the key's head, so a key at that slot is not found",
        file: "src/engine/tree/node.rs",
        original: "let through = hints.partition_point(|&hint| u16::from_be_bytes(hint) <= head);",
        replacement: "let through = hints.partition_point(|&hint| u16::from_be_bytes(hint) < head);",
        integration: &[],
        unit: &[SEARCHED],
    },
    Mutation {
        name: "write-costs-slots-moved",
        package: Package::Core,
        breaks: "the slots from the one inserted on move past the entry but not past the new slot, which is then written over one of them",
        file: "src/engine/tree/node/build.rs",
        original: "content.copy_within(slot_from..end, slot_from.saturating_add(entry).saturating_add(SLOT));",
        replacement: "content.copy_within(slot_from..end, slot_from.saturating_add(entry));",
        integration: &[],
        unit: &[INSERTED],
    },
];
