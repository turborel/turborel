//! The mutations of who takes a parallel run's morsels (RULES.md
//! R-TEST-9, D1246): the pool's workers taking as many of a run's morsels
//! at once as its degree, its reader's thread one more; a job served by
//! every worker of the pool; and a thread's next job numbered on from its
//! last, not served by the same workers; and of a grouping's morsels
//! handed whole (D1247): the statement thread's table never taken to the
//! partitions for its groups, and a morsel handed whole after morsels no
//! longer are; and of a run's lead past its window (D1262): a lead taken
//! whatever the bytes held ahead, and a result's bytes held after the
//! reader was given it; and of the lead of the runs over a grouping's
//! partitions (D1282): a lead that reaches every morsel, and a
//! statement's runs led by every partition.

use super::{Mutation, Package};

/// The file of a run of morsels.
const RUN: &str = "src/exec/parallel/run.rs";
/// The file of the pool of workers.
const POOL: &str = "src/threads/pool.rs";

/// The file of a grouping's waves.
const WAVES: &str = "src/exec/parallel/grouped/waves.rs";
/// The unit test of a grouping of many groups.
const MANY: &str = "exec::parallel::grouped::waves::tests::a_grouping_of_many_groups_stops_handing_morsels_whole";

/// The unit test of a run's lead.
const LED: &str = "exec::parallel::run::tests::a_run_leads_its_reader_while_what_it_holds_has_room";

pub const WORKERS: [Mutation; 9] = [
    Mutation {
        name: "a-lead-whatever-is-held-ahead",
        package: Package::Core,
        breaks: "a run leads its reader past its window though the bytes held ahead leave no room (RULES.md D1262)",
        file: RUN,
        original: "led.held.load(Ordering::Acquire) < (led.lead.room)()",
        replacement: "led.held.load(Ordering::Acquire) < usize::MAX",
        integration: &[],
        unit: &[LED],
    },
    Mutation {
        name: "a-result-given-stays-held-ahead",
        package: Package::Core,
        breaks: "a result's bytes stay counted as held ahead after the reader was given it (RULES.md D1262)",
        file: RUN,
        original: "led.held.fetch_sub(self.bytes_of(&result), Ordering::AcqRel);",
        replacement: "led.held.fetch_sub(0, Ordering::AcqRel);",
        integration: &[],
        unit: &[LED],
    },
    Mutation {
        name: "a-table-of-many-groups-stays-the-statements",
        package: Package::Core,
        breaks: "the statement thread's table stays its own past as many groups as a morsel has rows (RULES.md D1247)",
        file: WAVES,
        original: "        if table.len() > self.whole.1 {",
        replacement: "        if table.len() > usize::MAX {",
        integration: &[],
        unit: &[MANY],
    },
    Mutation {
        name: "a-morsel-is-handed-whole-after-they-stopped",
        package: Package::Core,
        breaks: "a morsel of few groups is handed whole though the grouping's morsels no longer are (RULES.md D1247)",
        file: WAVES,
        original: "    still && groups.saturating_mul(DIRECT_ROWS)",
        replacement: "    groups.saturating_mul(DIRECT_ROWS)",
        integration: &[],
        unit: &[MANY],
    },
    Mutation {
        name: "workers-take-the-readers-slot",
        package: Package::Core,
        breaks: "the workers run as many morsels of a run at once as its degree, its reader's thread busy beside them (RULES.md D1246)",
        file: RUN,
        original: "        self.degree.saturating_sub(1)",
        replacement: "        self.degree",
        integration: &[],
        unit: &["exec::parallel::run::tests::workers_take_one_morsel_fewer_than_the_degree"],
    },
    Mutation {
        name: "every-worker-serves-every-job",
        package: Package::Core,
        breaks: "a job is served by every worker of the pool, not by as many as it asks from its first (RULES.md D1246)",
        file: POOL,
        original: "is_some_and(|after| after < self.most)",
        replacement: "is_some()",
        integration: &[],
        unit: &["threads::pool::tests::a_job_is_served_by_its_workers_alone"],
    },
    Mutation {
        name: "a-threads-next-job-moves-on",
        package: Package::Core,
        breaks: "a thread's next job is numbered on past its last, so its runs change workers (RULES.md D1246)",
        file: POOL,
        original: "                return job.from;",
        replacement: "                others.push((job.owner, job.most));",
        integration: &[],
        unit: &["threads::pool::tests::a_threads_jobs_keep_their_workers"],
    },
    Mutation {
        name: "an-unmetered-lead-kept-to-the-window",
        package: Package::Core,
        breaks: "a run whose reader keeps every result takes every morsel past the one its reader waits for (RULES.md D1282)",
        file: RUN,
        original: "Lead { wide, size: |_| 0, room: Box::new(|| usize::MAX) }",
        replacement: "Lead { wide: wide.min(AHEAD_PER_THREAD), size: |_| 0, room: Box::new(|| usize::MAX) }",
        integration: &[],
        unit: &["exec::parallel::run::tests::a_run_led_by_every_morsel_takes_them_all_and_counts_no_byte"],
    },
    Mutation {
        name: "partitions-led-by-two-a-thread",
        package: Package::Core,
        breaks: "the runs that merge and finish a grouping's partitions lead their reader by every partition (RULES.md D1282)",
        file: WAVES,
        original: "parts: PARTS,",
        replacement: "parts: 2,",
        integration: &[],
        unit: &["exec::parallel::grouped::waves::tests::partitions_led_by_every_one_merge_and_spill_as_those_led_by_two"],
    },
];
