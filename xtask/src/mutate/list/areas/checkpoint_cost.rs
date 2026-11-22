//! The mutations of what a checkpoint costs and tells (RULES.md R-TEST-9,
//! D810, D811): the pages it wrote and the whole it took left untold, and
//! the runs a commit closes its side file and writes its file in, made one
//! granule each, uncapped, or joined across a gap.

use super::super::{Mutation, Package};

/// The file timing a checkpoint.
const TAKING: &str = "src/db/checkpoint/taking.rs";
/// The file closing and writing a commit's runs.
const SHADOW_FILE: &str = "src/disk/shadow/file.rs";
/// The integration test of what a checkpoint tells.
const TOOK: &str = "diagnostics::a_checkpoint_tells_what_it_took";
/// The unit test of the runs.
const RUNS: &str = "disk::shadow::tests::held_granules_close_the_side_file_in_runs";
/// The decision of a run's next granule.
const EXTENDS: &str = "    next == Some(number) && bytes.len() < RUN_BYTES";

/// Mechanisms of a checkpoint's cost whose removal must fail their tests.
pub const CHECKPOINT_COST: [Mutation; 5] = [
    Mutation {
        name: "ckpt-pages-untold",
        package: Package::Core,
        breaks: "a checkpoint tells no page written, whatever it wrote",
        file: TAKING,
        original: "        took.pages = pages_written(&files).saturating_sub(written);",
        replacement: "        took.pages = written.saturating_sub(written);",
        integration: &[TOOK],
        unit: &[],
    },
    Mutation {
        name: "ckpt-whole-untold",
        package: Package::Core,
        breaks: "a checkpoint tells a whole shorter than its steps",
        file: TAKING,
        original: "        took.micros = micros(began);",
        replacement: "        took.micros = 0;",
        integration: &[TOOK],
        unit: &[],
    },
    Mutation {
        name: "runs-one-each",
        package: Package::Core,
        breaks: "every granule is a run of its own, one op and one write each",
        file: SHADOW_FILE,
        original: EXTENDS,
        replacement: "    next == Some(number) && bytes.len() < RUN_BYTES && false",
        integration: &[],
        unit: &[RUNS],
    },
    Mutation {
        name: "runs-uncapped",
        package: Package::Core,
        breaks: "a run grows past RUN_BYTES",
        file: SHADOW_FILE,
        original: EXTENDS,
        replacement: "    next == Some(number)",
        integration: &[],
        unit: &[RUNS],
    },
    Mutation {
        name: "runs-across-gaps",
        package: Package::Core,
        breaks: "a granule past a gap joins the run before it, so it lands at the wrong offset",
        file: SHADOW_FILE,
        original: EXTENDS,
        replacement: "    next.is_some() && bytes.len() < RUN_BYTES",
        integration: &[],
        unit: &[RUNS],
    },
];
