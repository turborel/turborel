//! The mutations of a morsel's memory (RULES.md R-TEST-9, D1150, D1151,
//! D1153 to D1155): a morsel charged to its part's grant as it is gathered, one
//! past it read on from where its thread left it, never read again, its
//! rows made as they are read, a
//! probe's pairs made a batch at a time, a stream's rows ahead of its
//! reader held to what its morsels scan, and a probe of streams stopped
//! at a batch of pairs made (D1154).

use super::super::{Mutation, Package};

/// The test of joins past their grant run in parallel.
const PAST: &str = "fan_out::a_join_past_its_grant_answers_as_alone";

/// Mechanisms whose removal must fail the requirements of a morsel's memory.
pub const MORSEL_MEMORY: [Mutation; 9] = [
    Mutation {
        name: "course-counts-ahead",
        package: Package::Core,
        breaks: "whether a part will pass its grant counts the morsels gathered ahead of its reader as those it has taken",
        file: "src/exec/parallel.rs",
        original: "        from.saturating_add(grown.saturating_mul(count)",
        replacement: "        from.saturating_add(grown.max(self.held()).saturating_mul(count)",
        integration: &[],
        unit: &["exec::parallel::tests::a_part_is_on_course_by_the_morsels_read"],
    },
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
        breaks: "a stream drops the rows of a morsel past its grant its thread did not gather",
        file: "src/exec/parallel/charged.rs",
        original: "            Gathered::Past(items, rest) => Now::Alone(Box::new(items.into_iter().map(Ok).chain(rest))),",
        replacement: "            Gathered::Past(items, _rest) => Now::Alone(Box::new(items.into_iter().map(Ok))),",
        integration: &[PAST],
        unit: &[],
    },
    Mutation {
        name: "morsel-past-read-lost",
        package: Package::Core,
        breaks: "the reader of a morsel past its grant is handed none of the rest of its thread's read",
        file: "src/exec/parallel/charged.rs",
        original: "    Ok((items, charged.inner))",
        replacement: "    Ok((items, { charged.inner.by_ref().for_each(drop); charged.inner }))",
        integration: &[],
        unit: &["exec::parallel::charged::tests::a_morsel_past_its_grant_is_given_up"],
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
    Mutation {
        name: "morsel-ahead-unbounded",
        package: Package::Core,
        breaks: "a stream's morsels ahead of its reader hold as many rows as the grant holds",
        file: "src/exec/parallel/charged.rs",
        original: "            if !ahead.take(rows) {",
        replacement: "            if !ahead.take(0) {",
        integration: &[],
        unit: &["exec::parallel::charged::tests::a_morsel_past_its_grant_is_given_up"],
    },
    Mutation {
        name: "stream-probe-unpaused",
        package: Package::Core,
        breaks: "a probe of streams makes every pair of a left batch before it gives one",
        file: "src/exec/batch/joined/probe.rs",
        original: "        if out.ready() {",
        replacement: "        if out.ready() && n == usize::MAX {",
        integration: &[],
        unit: &["exec::batch::joined::pairs::tests::a_probe_holds_the_pairs_of_a_row_at_most"],
    },
    Mutation {
        name: "outer-probe-unpaused",
        package: Package::Core,
        breaks: "a FULL join of streams makes every pair of a left batch before it gives one",
        file: "src/exec/batch/joined/outer.rs",
        original: "            if self.out.ready() {",
        replacement: "            if self.out.ready() && n == usize::MAX {",
        integration: &[],
        unit: &["exec::batch::joined::pairs::tests::a_probe_holds_the_pairs_of_a_row_at_most"],
    },
];
