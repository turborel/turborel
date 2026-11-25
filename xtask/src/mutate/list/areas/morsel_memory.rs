//! The mutations of a morsel's memory (RULES.md R-TEST-9, D1150, D1151):
//! a morsel charged to its part's grant as it is gathered, one past it
//! read again as it is given, its rows made as they are read, and a
//! probe's pairs made a batch at a time.

use super::super::{Mutation, Package};

/// The test of joins past their grant run in parallel.
const PAST: &str = "fan_out::a_join_past_its_grant_answers_as_alone";

/// Mechanisms whose removal must fail the requirements of a morsel's memory.
pub const MORSEL_MEMORY: [Mutation; 4] = [
    Mutation {
        name: "morsel-charge-unbounded",
        package: Package::Core,
        breaks: "a morsel gathered is charged to its part's grant but never given up past it, so it is held whole",
        file: "src/exec/parallel/charged.rs",
        original: "        if self.limit.take(bytes) {",
        replacement: "        if self.limit.take(bytes) || bytes > 0 {",
        integration: &[],
        unit: &["exec::parallel::charged::tests::a_morsel_past_its_grant_is_given_up"],
    },
    Mutation {
        name: "morsel-past-dropped",
        package: Package::Core,
        breaks: "a stream drops the rows of a morsel past its grant, not reading it again",
        file: "src/exec/parallel/charged.rs",
        original: "            Taken::Past => Now::Alone((self.alone)(n)?),",
        replacement: "            Taken::Past => Now::Whole(Vec::new().into_iter()),",
        integration: &[PAST],
        unit: &[],
    },
    Mutation {
        name: "morsel-row-wrong-match",
        package: Package::Core,
        breaks: "a row read row by row is joined with the first built row for each of its matches",
        file: "src/exec/parallel/rows.rs",
        original: "joined.extend(table.shared.row_of(p));",
        replacement: "joined.extend(table.shared.row_of(p.min(0)));",
        integration: &[PAST],
        unit: &[],
    },
    Mutation {
        name: "probe-resumed-from-first",
        package: Package::Core,
        breaks: "a probe that stopped as a batch of pairs filled pairs its row with its key's rows from the first again",
        file: "src/exec/batch/joined/shared/probing.rs",
        original: "        if let Some(before) = self.seen.checked_sub(1)",
        replacement: "        if let Some(before) = self.seen.checked_sub(1).filter(|_| false)",
        integration: &[],
        unit: &["exec::batch::joined::shared::probing::tests::a_probe_gives_its_pairs_a_batch_at_a_time"],
    },
];
