//! The mutations of the memory budget's list of reservations (RULES.md
//! R-TEST-9, D38): the shards read back in shard order rather than the
//! order the reservations were made, and an entry left at the size it was
//! made with; and work grants sharing half of the whole budget, not of
//! what the buffer pools and other reservations leave (D683).

use super::super::{Mutation, Package};

/// The file of the memory budget.
const BUDGET_FILE: &str = "src/memory.rs";
/// The unit test of the list's order and sizes.
const ORDER: &str = "memory::tests::the_list_keeps_creation_order_across_shards";

/// Mechanisms of the budget's sharded list whose removal must fail its tests.
pub const BUDGET: [Mutation; 3] = [
    Mutation {
        name: "budget-list-order",
        package: Package::Core,
        breaks: "the list of reservations follows its shards, not the order the reservations were made",
        file: BUDGET_FILE,
        original: "listed.sort_unstable_by_key(|&(id, _, _)| id);",
        replacement: "listed.truncate(listed.len());",
        integration: &[],
        unit: &[ORDER],
    },
    Mutation {
        name: "budget-entry-size",
        package: Package::Core,
        breaks: "a reservation's entry in the list keeps the size it was made with",
        file: BUDGET_FILE,
        original: "self.held.bytes.store(bytes, Ordering::Relaxed);",
        replacement: "let _unchanged = bytes;",
        integration: &[],
        unit: &[ORDER, "memory::tests::a_reservation_grows_and_shrinks"],
    },
    Mutation {
        name: "grant-share-of-whole-budget",
        package: Package::Core,
        breaks: "work grants keep half of the whole budget, so a buffer pool of more than half of it leaves every operator the floor",
        file: "src/spill/grant.rs",
        original: "let share = budget.limit().saturating_sub(budget.standing()) >> 1_u32;",
        replacement: "let share = budget.limit().saturating_sub(budget.reserved()).saturating_sub(budget.limit() >> 1_u32) + held;",
        integration: &["creation::a_pool_of_most_of_the_budget_leaves_a_hash_join_room"],
        unit: &["spill::grant::tests::grants_share_what_a_pool_leaves"],
    },
];
