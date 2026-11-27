//! The mutations of a filter passing over what its scan judged exactly
//! (RULES.md R-TEST-9, D1266): which conjuncts a scan says it judged
//! exactly of a run, what it says of the rows gathered of several runs
//! and of a run given whole, that its word goes with one batch and no
//! other, which filter hears it, and which conjunct each told place is.

use super::super::{Mutation, Package};

const CODED: &str = "src/engine/columns/judged/coded.rs";
const JUDGED: &str = "src/engine/columns/judged.rs";
const JUDGE: &str = "src/exec/batch/judge.rs";
const SAID: &str = "src/exec/batch/vetted.rs";
const PIPELINE: &str = "src/exec/parallel/pipeline.rs";

/// The test of what a LIKE's judging says of each run.
const LIKE_SAID: &str = "engine::columns::judged::coded::tests::a_like_is_said_judged_exactly_only_of_a_run_judged_to_its_end";
/// The test of a pipeline's rows with its filter hearing and deaf.
const DEAF: &str = "exec::parallel::batched::vetting::a_pipeline_gives_the_same_rows_with_its_filter_hearing_and_deaf";
/// The test of which step hears a branch's scan.
const OWN: &str = "exec::parallel::batched::vetting::the_filter_that_hears_is_the_scans_own";
/// The test of what a filter judges of a batch its scan vouches for.
const PASSED: &str = "exec::batch::judge::vetted::tests::a_conjunct_is_passed_over_only_of_a_batch_the_scan_judged_it_of";
/// The test of each conjunct's told place.
const PLACES: &str = "exec::batch::judge::vetted::tests::each_conjunct_judged_knows_its_told_place";
/// The test of a conjunct the scan judged of a batch's first rows.
const REST: &str = "exec::batch::judge::vetted::tests::a_conjunct_judged_of_the_first_rows_is_judged_of_the_rest";

/// Mechanisms whose removal must fail the requirements of a filter
/// passing over what its scan judged exactly.
pub const VETTED: [Mutation; 17] = [
    Mutation {
        name: "vetted-like-stopped-early-said-exact",
        package: Package::Core,
        breaks: "a LIKE whose judging stopped part way through a run is said to be judged exactly, so rows it never judged are kept",
        file: CODED,
        original: "        (true, Some(place)) => Reach::Before(place),",
        replacement: "        (true, Some(_)) => Reach::Every,",
        integration: &[],
        unit: &[LIKE_SAID, DEAF],
    },
    Mutation {
        name: "vetted-like-judged-past-where-it-stopped",
        package: Package::Core,
        breaks: "a LIKE whose judging stopped is said to be judged of rows past the one it stopped at, which it kept unjudged",
        file: CODED,
        original: "    let unjudged = chunks.as_slice().first().and_then(|(_, these)| these.first().copied());",
        replacement: "    let unjudged = chunks.as_slice().first().and_then(|(_, these)| these.last().copied());",
        integration: &[],
        unit: &[LIKE_SAID],
    },
    Mutation {
        name: "vetted-rest-of-a-run-not-judged",
        package: Package::Core,
        breaks: "the rows a scan kept unjudged by a conjunct it judged of a run's first rows are kept by the filter unjudged too",
        file: "src/exec/batch/judge/vetted.rs",
        original: "        settled.extend(self.kept(batch, Some(&rest), runner, runner.kernels())?);",
        replacement: "        settled.extend(self.kept(batch, Some(&[]), runner, runner.kernels())?);\n        settled.extend(rest);",
        integration: &[],
        unit: &[REST],
    },
    Mutation {
        name: "vetted-chunk-gave-way-said-exact",
        package: Package::Core,
        breaks: "a LIKE is said to be judged exactly of a run one of whose chunks gave way, its rows kept unjudged",
        file: CODED,
        original: "    judged.is_some()\n}",
        replacement: "    judged.is_some() || !these.is_empty()\n}",
        integration: &[],
        unit: &["engine::columns::judged::coded::tests::a_like_gives_way_of_padded_strings_and_malformed_codes"],
    },
    Mutation {
        name: "vetted-sketch-said-exact",
        package: Package::Core,
        breaks: "a comparison a sketch's codes answered for is said to be judged exactly",
        file: CODED,
        original: "    Ok(Some((out, if sketch.is_none() { Reach::Every } else { Reach::Part })))",
        replacement: "    Ok(Some((out, if sketch.is_none() || !out.is_empty() { Reach::Every } else { Reach::Part })))",
        integration: &[],
        unit: &["engine::columns::judged::sketched::tests::a_run_judged_by_a_sketch_is_not_said_judged_exactly"],
    },
    Mutation {
        name: "vetted-sort-bound-said-exact",
        package: Package::Core,
        breaks: "a sort's bound, which tightens as the scan runs, is said to be judged exactly",
        file: JUDGED,
        original: "        if matches!(self.conjunct, Conjunct::Dynamic { .. }) { 0 } else { Vetted::bit(self.at) }",
        replacement: "        if matches!(self.conjunct, Conjunct::Spans { .. }) { 0 } else { Vetted::bit(self.at) }",
        integration: &[],
        unit: &["engine::columns::judged::coded::tests::a_sorts_bound_is_never_said_judged_exactly"],
    },
    Mutation {
        name: "vetted-decoded-conjunct-not-said",
        package: Package::Core,
        breaks: "a conjunct judged of its column decoded is not said to be judged exactly, so the filter judges it again",
        file: JUDGED,
        original: "                reached.note(bit, Reach::Every);",
        replacement: "                reached.note(bit, Reach::Part);",
        integration: &[],
        unit: &[LIKE_SAID],
    },
    Mutation {
        name: "vetted-gathered-rows-of-any-run",
        package: Package::Core,
        breaks: "the rows gathered of several runs are said to be judged exactly by a conjunct only one of the runs judged so",
        file: "src/engine/rows/frozen/batches/pending.rs",
        original: "        self.exact = if first { exact } else { self.exact & exact };",
        replacement: "        self.exact = if first { exact } else { self.exact | exact };",
        integration: &[],
        unit: &["engine::rows::frozen::batches::pending::tests::only_the_conjuncts_every_run_judged_exactly_are_kept"],
    },
    Mutation {
        name: "vetted-whole-run-said-of-every-row",
        package: Package::Core,
        breaks: "a run given whole is said to be judged exactly of every row of it, not of the rows the scan kept",
        file: "src/engine/rows/frozen/batches/run.rs",
        original: "rows: (judged.kept.len() < rows).then_some(judged.kept), exact, partly });",
        replacement: "rows: (judged.kept.len() > rows).then_some(judged.kept), exact, partly });",
        integration: &[],
        unit: &[DEAF],
    },
    Mutation {
        name: "vetted-word-kept-for-the-next-batch",
        package: Package::Core,
        breaks: "what a scan said of a batch of frozen rows is said again of the batch of the tree's rows after it, whose rows it never judged",
        file: "src/engine/rows/frozen/batches/vetted.rs",
        original: "        Some(batch.map(|b| (b, self.0.vetted.take())))",
        replacement: "        Some(batch.map(|b| (b, self.0.vetted.clone())))",
        integration: &[],
        unit: &[DEAF],
    },
    Mutation {
        name: "vetted-heard-of-a-batch-not-the-last",
        package: Package::Core,
        breaks: "a filter hears the scan's word of a batch that is not the one the scan gave last",
        file: SAID,
        original: "        if self.parted.load(Ordering::Relaxed) {\n            return Heard::default();",
        replacement: "        if self.parted.load(Ordering::Relaxed) && judged == 0 {\n            return Heard::default();",
        integration: &[],
        unit: &["exec::batch::vetted::tests::nothing_is_heard_once_a_batch_is_not_the_scans_last"],
    },
    Mutation {
        name: "vetted-conjunct-judged-again",
        package: Package::Core,
        breaks: "a conjunct the scan judged exactly of a batch is judged of it again",
        file: JUDGE,
        original: "            if self.told.get(n).is_some_and(|&told| heard.passes(told)) {",
        replacement: "            if self.told.get(n).is_some_and(|&told| heard.passes(told) && n > self.judged.len()) {",
        integration: &[],
        unit: &[PASSED],
    },
    Mutation {
        name: "vetted-conjunct-passed-over-unsaid",
        package: Package::Core,
        breaks: "a conjunct told the scan is passed over whether or not the scan says it judged it of the batch",
        file: JUDGE,
        original: "            if self.told.get(n).is_some_and(|&told| heard.passes(told)) {",
        replacement: "            if self.told.get(n).is_some_and(|&told| told != 0) {",
        integration: &[],
        unit: &[PASSED],
    },
    Mutation {
        name: "vetted-like-takes-its-bounds-place",
        package: Package::Core,
        breaks: "a conjunct is paired with the place before its own told conjunct, as a LIKE with a bound of its prefix",
        file: "src/logical/pushed.rs",
        original: "    own.push((cond, itself.as_ref().map(|_| found.len())));",
        replacement: "    own.push((cond, itself.as_ref().map(|_| found.len().saturating_sub(1))));",
        integration: &[],
        unit: &["logical::pushed::tests::each_conjunct_knows_the_place_of_the_one_told_that_is_it", PLACES],
    },
    Mutation {
        name: "vetted-filter-of-a-scan-not-told",
        package: Package::Core,
        breaks: "the first filter of a branch hears its scan though the scan was not told that filter's conjuncts",
        file: PIPELINE,
        original: "        (self.vets && matches!(self.steps.get(first), Some(Step::Filter(_)))).then_some(first)",
        replacement: "        (self.vets | matches!(self.steps.get(first), Some(Step::Filter(_)))).then_some(first)",
        integration: &[],
        unit: &[OWN],
    },
    Mutation {
        name: "vetted-filter-past-another-step",
        package: Package::Core,
        breaks: "a filter hears its branch's scan across a step that changes the batches before it",
        file: PIPELINE,
        original: "        let first = self.steps.iter().position(|step| !matches!(step, Step::Count(_)))?;",
        replacement: "        let first = self.steps.iter().position(|step| matches!(step, Step::Filter(_)))?;",
        integration: &[],
        unit: &[OWN],
    },
    Mutation {
        name: "vetted-place-past-the-word-wraps",
        package: Package::Core,
        breaks: "a conjunct told past the last place a word holds shares the bit of an earlier one",
        file: "src/storage/batch/vetted.rs",
        original: "        u32::try_from(at).ok().and_then(|at| 1_u64.checked_shl(at)).unwrap_or(0)",
        replacement: "        u32::try_from(at).ok().map_or(0, |at| 1_u64.wrapping_shl(at))",
        integration: &[],
        unit: &["storage::batch::vetted::tests::a_place_past_the_word_has_no_bit"],
    },
];
