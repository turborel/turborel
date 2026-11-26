//! The mutations of the memory budget's list of reservations (RULES.md
//! R-TEST-9, D38): the shards read back in shard order rather than the
//! order the reservations were made, and an entry left at the size it was
//! made with; and work grants sharing half of the whole budget, not of
//! what the buffer pools and other reservations leave (D683); and the
//! planner's grant taken as what is free now, not a statement's alone
//! (D1159).

use super::super::{Mutation, Package};

/// The file of the memory budget.
const BUDGET_FILE: &str = "src/memory.rs";
/// The unit test of the list's order and sizes.
const ORDER: &str = "memory::tests::the_list_keeps_creation_order_across_shards";

/// Mechanisms of the budget's sharded list whose removal must fail its tests.
pub const BUDGET: [Mutation; 6] = [
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
    Mutation {
        name: "grant-planned-as-free-now",
        package: Package::Core,
        breaks: "the planner costs spills at the grant free as it plans, so a plan changes with the grants other statements hold",
        file: "src/spill/grant.rs",
        original: "        ceiling.min(share >> 1_u32).max(WORK_FLOOR)",
        replacement: "        wanted_of(&self.memory, self.granted.load(Ordering::Acquire), ceiling).max(share.min(WORK_FLOOR))",
        integration: &[],
        unit: &["physical::cost::alone_tests::a_plan_is_costed_at_the_grant_of_a_statement_alone"],
    },
    Mutation {
        name: "planned-follows-growth",
        package: Package::Core,
        breaks: "a standing reservation planned at its bound is planned at what it holds, so the planner's grant changes with the writes",
        file: BUDGET_FILE,
        original: "            if self.bound.is_none() {",
        replacement: "            if self.bound.is_some() || self.bound.is_none() {",
        integration: &[],
        unit: &["physical::cost::alone_tests::a_plan_is_costed_at_the_grant_of_a_statement_alone"],
    },
    Mutation {
        name: "kept-planned-as-held",
        package: Package::Core,
        breaks: "the kept undo stands planned at what it keeps, so the planner's grant changes with the writes",
        file: "src/engine/rows/undo/kept.rs",
        original: "        reservation.stand_up_to(budget.limit().checked_div(KEPT_SHARE).unwrap_or(0));",
        replacement: "        reservation.stand();",
        integration: &[],
        unit: &["engine::rows::undo::tests::batches_written_are_kept_until_truncated"],
    },
];
