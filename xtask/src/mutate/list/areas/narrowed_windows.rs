//! The mutations of a window pruned and narrowed (RULES.md R-TEST-9,
//! D1245): its input pruned to the columns it and those above read, a
//! call's columns read though nothing above reads the call, its rows
//! narrowed to those columns whether under a projection or a scan, its
//! calls' columns numbered after them, a window read in an order merged
//! back in it, and a run of morsels dropped letting go of what it holds
//! at once.

use super::super::{Mutation, Package};

/// The file of the pruning's walk.
const PRUNED: &str = "src/physical/blind/pruned.rs";

/// The file of a window's pruning.
const WINDOWED: &str = "src/physical/blind/pruned/windowed.rs";

/// The file of a window's narrowing.
const NARROWED: &str = "src/physical/blind/pruned/narrowed.rs";

/// The file of a window's plan.
const ORDERED: &str = "src/physical/ordered.rs";

/// The file of a run of morsels.
const RUN: &str = "src/exec/parallel/run.rs";

/// The unit test of a run dropped by its reader.
const DROPPED: &str = "exec::parallel::run::tests::a_run_dropped_lets_go_of_its_work_and_its_results_at_once";

/// The unit test of a window's scan pruned and narrowed.
const SCAN: &str = "physical::blind::pruned::tests::a_windows_scan_reads_only_what_it_and_those_above_read";

/// The unit test of a call's column read.
const CALL: &str = "physical::blind::pruned::tests::a_column_a_windows_call_alone_reads_is_read";

/// The test of narrowed windows against whole ones.
const WHOLE: &str = "narrowed_windows::a_narrowed_window_gives_the_rows_of_the_whole_one";

/// The test of a call read by none, and of an order asked.
const RAISED: &str = "narrowed_windows::a_call_read_by_none_still_raises_and_an_order_asked_is_kept";

/// The test of windows split by row past the grant.
const SPILLED: &str = "spilled_windows::partitions_past_the_grant_give_their_results_in_order";

/// Mechanisms whose removal must fail the requirements of a window pruned
/// and narrowed.
pub const NARROWED_WINDOWS: [Mutation; 6] = [
    Mutation {
        name: "windows-read-whole",
        package: Package::Core,
        breaks: "a window's input reads the columns it and those above read alone (D1245)",
        file: PRUNED,
        original: "Plan::Window { .. } => self.window_pruned(plan, sizes.kid(0), needed),",
        replacement: "Plan::Window { .. } => self.read_whole(plan),",
        integration: &[WHOLE],
        unit: &[SCAN],
    },
    Mutation {
        name: "windows-call-columns-unread",
        package: Package::Core,
        breaks: "a column a window's call reads is read though nothing above reads the call (D1245)",
        file: WINDOWED,
        original: "reads.extend(of_calls.into_iter().filter(|&c| c < width));",
        replacement: "drop(of_calls);",
        integration: &[RAISED],
        unit: &[CALL],
    },
    Mutation {
        name: "windows-scan-not-narrowed",
        package: Package::Core,
        breaks: "a window over a scan of some of its table's columns carries those alone (D1245)",
        file: NARROWED,
        original: "(live, self.sized(below).width?)",
        replacement: "(live.clone(), live.len())",
        integration: &[WHOLE],
        unit: &[SCAN],
    },
    Mutation {
        name: "windows-calls-unshifted",
        package: Package::Core,
        breaks: "a window's calls' columns are numbered after the columns it carries (D1245)",
        file: NARROWED,
        original: "c.saturating_sub(width).saturating_add(live.len())",
        replacement: "c",
        integration: &[WHOLE],
        unit: &[SCAN],
    },
    Mutation {
        name: "windows-order-dropped",
        package: Package::Core,
        breaks: "a window run in parallel merges its partitions back in its order where one is asked (D1245)",
        file: ORDERED,
        original: "Plan::Window { window, detail, est, ordered: wanted }",
        replacement: "Plan::Window { window, detail, est, ordered: false }",
        integration: &[RAISED, SPILLED],
        unit: &[],
    },
    Mutation {
        name: "morsels-drop-leaves-work",
        package: Package::Core,
        breaks: "a run dropped by its reader lets go of its work and its results before the drop returns (D1245)",
        file: RUN,
        original: "let work = self.run.work.write().unwrap_or_else(PoisonError::into_inner).take();",
        replacement: "let work: Option<Work<T>> = None;",
        integration: &[],
        unit: &[DROPPED],
    },
];
