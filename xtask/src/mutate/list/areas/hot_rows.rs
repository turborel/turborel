//! The mutations of rows in demand (RULES.md R-TEST-9, D849, D850, D851,
//! D942): an increment through a delta table reserved, what its select
//! list shows guarded and given, and one that shows the sum held; an
//! increment by a whole key reserved by its point change, and what it
//! shows guarded; a completion told
//! at the end with no wait for what it rests on, and a rollback that waits
//! for it; a row whose writer ended taken by a newcomer, and the waiter
//! overtaken so queued first, the line behind it, its claim firm.

use super::super::{Mutation, Package};

/// Escrow through a delta table compared with the rows written and held.
const HOLDS: &str = "escrow_delta::an_increment_through_a_final_table_showing_other_columns_holds_no_row";

/// The mutations of rows in demand, in the order they run.
pub const HOT_ROWS: [Mutation; 11] = [
    Mutation {
        name: "escrow-delta-refused",
        package: Package::Core,
        breaks: "an increment through a delta table whose reader shows another column holds its row",
        file: "src/db/escrow.rs",
        original: "let shown = effects.delta.as_ref().map_or(Some(0), Collector::shown)?;",
        replacement: "let shown = effects.delta.as_ref().map_or(Some(0), |_| None)?;",
        integration: &[HOLDS],
        unit: &[],
    },
    Mutation {
        name: "escrow-delta-shows-sum",
        package: Package::Core,
        breaks: "an increment through a delta table that shows its column is reserved, the sum shown unapplied",
        file: "src/db/escrow.rs",
        original: "(shown & added == 0).then_some(guarded | shown)",
        replacement: "(shown & added == shown & added).then_some(guarded | shown)",
        integration: &[HOLDS],
        unit: &[],
    },
    Mutation {
        name: "escrow-delta-unguarded",
        package: Package::Core,
        breaks: "a reservation through a delta table leaves the columns its select list shows unguarded",
        file: "src/db/escrow.rs",
        original: "(shown & added == 0).then_some(guarded | shown)",
        replacement: "(shown & added == 0).then_some(guarded)",
        integration: &["escrow_delta::a_reservation_through_a_final_table_guards_what_it_shows"],
        unit: &[],
    },
    Mutation {
        name: "escrow-delta-row-dropped",
        package: Package::Core,
        breaks: "a delta table of an increment reserved is given no row",
        file: "src/db/write.rs",
        original: "return effects.reserved(&w.bound.table, new);",
        replacement: "return Ok(());",
        integration: &["escrow_delta::two_payments_into_one_row_both_apply"],
        unit: &[],
    },
    Mutation {
        name: "told-at-end-waits",
        package: Package::Core,
        breaks: "a completion told at the end waits for what it rests on, holding its row locks",
        file: "src/db/running.rs",
        original: "if self.told_at_end.get() && result.is_ok() {",
        replacement: "if self.told_at_end.get() && result.is_err() {",
        integration: &["told_at_end::a_completion_told_at_the_end_does_not_wait_for_the_sync"],
        unit: &[],
    },
    Mutation {
        name: "rollback-unsettled",
        package: Package::Core,
        breaks: "a rollback returns before what its transaction rests on is durable",
        file: "src/txn/end.rs",
        original: "        tx.replied(damage)",
        replacement: "        damage",
        integration: &["told_at_end::an_end_resting_on_a_failed_sync_is_in_doubt"],
        unit: &[],
    },
    Mutation {
        name: "row-kept-for-sleeper",
        package: Package::Core,
        breaks: "a row whose writer ended is kept for a first waiter not yet running",
        file: "src/txn/row_waits.rs",
        original: "(claim.firm || !claim.line.has_ended()).then_some(claim)",
        replacement: "Some(claim)",
        integration: &[],
        unit: &["txn::row_waits::tests::a_newcomer_takes_a_row_its_first_waiter_has_not_written"],
    },
    Mutation {
        name: "overtaken-claim-not-firm",
        package: Package::Core,
        breaks: "a waiter overtaken at its turn may be overtaken again",
        file: "src/txn/row_waits/overtaken.rs",
        original: "first: self.id, firm: true };",
        replacement: "first: self.id, firm: false };",
        integration: &[],
        unit: &["txn::row_waits::tests::a_waiter_overtaken_once_is_never_overtaken_again"],
    },
    Mutation {
        name: "overtaken-line-left",
        package: Package::Core,
        breaks: "the waiters behind one overtaken stay in the ended line, to wake each in turn",
        file: "src/txn/row_waits/overtaken.rs",
        original: "            move_line((&mut from, (&mut to, &next)), (table, id), writer, &mut waits_for);",
        replacement: "            let _ = (&mut from, &mut waits_for);",
        integration: &[],
        unit: &["txn::row_waits::tests::a_waiter_overtaken_once_is_never_overtaken_again"],
    },
    Mutation {
        name: "escrow-point-unreserved",
        package: Package::Core,
        breaks: "a point change shows its increment's row and reserves nothing, so the sum is never applied",
        file: "src/db/write/direct.rs",
        original: "    if !st.reserve_row(tx, (&w.bound.table, id), (&seen, &added, guarded))? {\n        return Ok(None);\n    }\n",
        replacement: "    if added.is_empty() && guarded == 0 {\n        return Ok(None);\n    }\n",
        integration: &["escrow_point::a_payment_by_a_key_gives_what_the_general_path_gives"],
        unit: &[],
    },
    Mutation {
        name: "escrow-point-unguarded",
        package: Package::Core,
        breaks: "a reservation a point change takes guards no column, so what it showed changes under it",
        file: "src/db/write/direct.rs",
        original: "    if !st.reserve_row(tx, (&w.bound.table, id), (&seen, &added, guarded))? {\n",
        replacement: "    if !st.reserve_row(tx, (&w.bound.table, id), (&seen, &added, guarded & 0))? {\n",
        integration: &["escrow_point::a_reservation_by_a_key_guards_what_it_shows"],
        unit: &[],
    },
];
