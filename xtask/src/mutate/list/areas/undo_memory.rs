//! The mutations of the undo store's batches kept in memory (RULES.md
//! R-TEST-9, D745, D746): a batch written kept, the bound of the budget's
//! share, the budget's refusal, a batch gone past the bound let go, a
//! truncation forgetting what memory holds, and a refused writer told the
//! row's writer with no older version read (D747).

use super::super::{Mutation, Package};

/// The undo store's file.
const UNDO: &str = "src/engine/rows/undo.rs";
/// The bound's file.
const KEPT: &str = "src/engine/rows/undo/kept.rs";

/// Mechanisms whose removal must fail the requirements of batches kept.
pub const UNDO_MEMORY: [Mutation; 6] = [
    Mutation {
        name: "undo-kept-none",
        package: Package::Core,
        breaks: "a batch written is let go of memory whatever the bound, so every older version is read from the log",
        file: UNDO,
        original: "kept.then_some(record)",
        replacement: "kept.then_some(record).filter(|_| false)",
        integration: &[],
        unit: &["engine::rows::undo::tests::batches_written_are_kept_until_truncated"],
    },
    Mutation {
        name: "undo-kept-unbounded",
        package: Package::Core,
        breaks: "the batches kept pass the budget's share",
        file: KEPT,
        original: "while after(self) > self.bound() || !self.reserved(after(self)) {",
        replacement: "while !self.reserved(after(self)) {",
        integration: &[],
        unit: &["engine::rows::undo::tests::batches_past_the_bound_are_read_from_the_log"],
    },
    Mutation {
        name: "undo-kept-budget-ignored",
        package: Package::Core,
        breaks: "a batch is kept though the budget refuses its bytes",
        file: KEPT,
        original: "reservation.bytes() >= bytes || reservation.resize(bytes).is_ok()",
        replacement: "reservation.bytes() >= bytes || reservation.resize(bytes).is_ok() || bytes > 0",
        integration: &[],
        unit: &["engine::rows::undo::tests::a_budget_refusing_keeps_no_batch"],
    },
    Mutation {
        name: "undo-evicted-kept",
        package: Package::Core,
        breaks: "a batch past the bound stays in memory, uncounted",
        file: UNDO,
        original: "evicted.into_iter().filter_map(|n| self.resident.remove(n)).collect()",
        replacement: "evicted.into_iter().filter_map(|_| None).collect()",
        integration: &[],
        unit: &["engine::rows::undo::tests::batches_past_the_bound_are_read_from_the_log"],
    },
    Mutation {
        name: "undo-truncate-keeps-memory",
        package: Package::Core,
        breaks: "a truncation leaves the batches it forgets in memory",
        file: UNDO,
        original: "(batches.first..needed).filter_map(|n| self.resident.remove(n)).collect()",
        replacement: "(batches.first..needed).filter_map(|_| None).collect()",
        integration: &[],
        unit: &[
            "engine::rows::undo::tests::batches_written_are_kept_until_truncated",
            "engine::rows::undo::tests::batches_go_once_nothing_in_them_is_pinned",
        ],
    },
    Mutation {
        name: "undo-waiter-told-no-writer",
        package: Package::Core,
        breaks: "a writer refused is not told the open transaction that changed the row, so it never waits for it",
        file: "src/engine/rows/mvcc.rs",
        original: "Maker::Open(by) => (Some(by).filter(|by| *by != tx), 0),",
        replacement: "Maker::Open(_) => (None, 0),",
        integration: &["row_waits::a_second_writer_waits_and_writes_the_committed_row_4_28"],
        unit: &[],
    },
];
