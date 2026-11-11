//! Project automation for turborel, run as `cargo xtask <command>`.
//!
//! Every check a change must pass lives here, in Rust, so it runs wherever
//! cargo runs and is reviewed, linted and tested like the rest of the code
//! (RULES.md R-TOOL-1).

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a command-line tool reports by printing (RULES.md R-DEV-3)")]

mod baseline;
mod bench;
mod citations;
mod cldr;
mod command;
mod coverage;
mod error;
mod file_length;
mod files;
mod json;
mod lint_audit;
mod mutate;
mod threads;
mod trace;
mod unicode;

use std::path::Path;
use std::process::ExitCode;

use command::{NO_ENV, RUSTDOCFLAGS_VAR};
use error::{Error, Result};

/// The commands, as typed after `cargo xtask`.
const VERIFY: &str = "verify";
const WITH_COVERAGE: &str = "--coverage";
const COVERAGE: &str = "coverage";
const TRACE: &str = "trace";
const FILE_LENGTH: &str = "file-length";
const LINT_AUDIT: &str = "lint-audit";
const WRITE: &str = "--write";
const INSTALL_HOOKS: &str = "install-hooks";
const BENCH: &str = "bench";
const MUTATE: &str = "mutate";
const SIMULATE: &str = "simulate";
const UNICODE: &str = "unicode";
const UNICODE_CHECK: &str = "unicode-check";
const CLDR: &str = "cldr";
const FROM: &str = "--from";
/// The support crate's check of the collation UNICODE against Unicode's own
/// test file, built optimized, run outside `verify` (RULES.md D60).
const UNICODE_CHECK_ARGS: [&str; 9] =
    ["run", "--quiet", "--release", "--package", "turborel-test-support", "--features", "database", "--bin", "unicode_check"];
/// The support crate's simulator sweep, built optimized, which `simulate`
/// runs with the arguments given (RULES.md D57, R-TEST-5).
const SIMULATE_ARGS: [&str; 9] =
    ["run", "--quiet", "--release", "--package", "turborel-test-support", "--features", "replication", "--bin", "simulate"];
/// The tracked directory of git hooks.
const HOOKS_DIR: &str = ".githooks";
/// Builds and runs every test target. Not `--all-targets`, which would run
/// the benchmarks too, unoptimized: they measure, they do not test (D6).
const TEST_ARGS: [&str; 8] = ["test", "--workspace", "--features", SIMULATOR, "--lib", "--bins", "--tests", "--quiet"];
/// The support crate's simulator, a feature of its own so the core's own
/// tests may use the crate without it: built and linted with the
/// workspace, its `simulate` binary among them (RULES.md D57).
const SIMULATOR: &str = "turborel-test-support/replication";
/// Makes warnings errors, for clippy and for rustdoc (R-DOC-3).
const DENY: &str = "-D";
const WARNINGS: &str = "warnings";
/// Git, and the setting that names the hooks directory.
const GIT: &str = "git";
const HOOKS_PATH_KEY: &str = "core.hooksPath";

/// The usage text, built from the command names so the two cannot drift apart.
fn usage() -> String {
    format!(
        "usage: cargo xtask <command>

commands:
  {VERIFY} [{WITH_COVERAGE}]  every check a change must pass (RULES.md R-PROC-1)
  {COVERAGE}             structural coverage report and floor check (R-COV)
  {TRACE} [FILTER]       requirement trace, test -> requirements (R-TRACE)
  {FILE_LENGTH}          source file length check (R-SIZE-2)
  {LINT_AUDIT} [{WRITE}]  lint baseline counts: check, or record a decrease (R-DEV-2)
  {INSTALL_HOOKS}        run `verify` before every commit (R-PROC-1)
  {BENCH} [NAME] [--save LABEL] [--compare LABEL]
                       the benchmarks in bench/, built optimized; keep or compare results (R-BENCH-1)
  {MUTATE} [NAME]        break each mechanism the list names, or one, and expect its tests to fail (R-TEST-9)
  {SIMULATE} [--seeds N] [--from S] [--plan NAME] [--databases]
                       sweeps of the replication simulator longer than `verify` runs (D57, R-TEST-5)
  {UNICODE} {FROM} DIR   the collation table, from Unicode's files in DIR (D60)
  {UNICODE_CHECK} {FROM} DIR
                       the collation UNICODE against Unicode's test file in DIR (D60)
  {CLDR} {FROM} CLDR UCD  the collations UNICODE_<LOCALE>, from CLDR's files in CLDR and Unicode's in UCD (D60)"
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatches one command line.
fn run(args: &[String]) -> Result<()> {
    let root = command::workspace_root();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [VERIFY] => verify(&root, false),
        [VERIFY, WITH_COVERAGE] => verify(&root, true),
        [COVERAGE] => coverage::run(&root),
        [TRACE] => trace::print(&root, ""),
        [TRACE, filter] => trace::print(&root, filter),
        [FILE_LENGTH] => file_length::check(&root),
        [LINT_AUDIT] => lint_audit::check(&root),
        [LINT_AUDIT, WRITE] => lint_audit::write(&root),
        [INSTALL_HOOKS] => install_hooks(&root),
        [BENCH, rest @ ..] => bench::run(&root, rest),
        [MUTATE, rest @ ..] => mutate::run(&root, rest),
        [SIMULATE, rest @ ..] => simulate(&root, rest),
        [UNICODE, FROM, dir] => unicode::generate(&root, Path::new(dir)),
        [CLDR, FROM, cldr, ucd] => cldr::generate(&root, Path::new(cldr), Path::new(ucd)),
        [UNICODE_CHECK, FROM, dir] => command::cargo(&root, &[UNICODE_CHECK_ARGS.as_slice(), &["--", dir]].concat(), NO_ENV),
        _ => Err(Error::new(usage())),
    }
}

/// Runs every check in order and stops at the first failure (RULES.md R-PROC-1).
fn verify(root: &Path, with_coverage: bool) -> Result<()> {
    step("format (R-STYLE-1)");
    command::cargo(root, &["fmt", "--all", "--check"], NO_ENV)?;
    step("lints (R-ERR, R-NUM, R-CF, R-STYLE, R-DOC)");
    command::cargo(root, &["clippy", "--workspace", "--features", SIMULATOR, "--all-targets", "--quiet", "--", DENY, WARNINGS], NO_ENV)?;
    step("lint baseline counts (R-DEV-2)");
    lint_audit::check(root)?;
    step("docs (R-DOC-3)");
    command::cargo(root, &["doc", "--workspace", "--no-deps", "--quiet"], &[(RUSTDOCFLAGS_VAR, format!("{DENY} {WARNINGS}"))])?;
    step("file length (R-SIZE-2)");
    file_length::check(root)?;
    step("traceability (R-TRACE-1)");
    trace::check(root)?;
    step("citations (R-DOC-5)");
    citations::check(root)?;
    step("threads (R-SAFE-5)");
    threads::check(root)?;
    step("tests: unit and integration (R-TEST)");
    command::cargo(root, &TEST_ARGS, NO_ENV)?;
    step("doc tests (R-TEST-1)");
    let mut doc_tests = vec!["test", "--doc", "--quiet"];
    for package in coverage::PACKAGES {
        doc_tests.extend(["--package", package]);
    }
    command::cargo(root, &doc_tests, NO_ENV)?;
    if with_coverage {
        step("coverage (R-COV)");
        coverage::run(root)?;
    }
    println!("\nverify: all checks passed");
    Ok(())
}

/// Runs the simulator's sweep with `rest` as its arguments (RULES.md D57,
/// R-TEST-5), built optimized.
fn simulate(root: &Path, rest: &[&str]) -> Result<()> {
    let mut args = SIMULATE_ARGS.to_vec();
    args.push("--");
    args.extend(rest);
    command::cargo(root, &args, NO_ENV)
}

/// Points git at the tracked hooks in `HOOKS_DIR`, so `verify` runs before
/// every commit of this clone (RULES.md R-PROC-1).
fn install_hooks(root: &Path) -> Result<()> {
    command::tool_output(root, Path::new(GIT), &["config", HOOKS_PATH_KEY, HOOKS_DIR])?;
    println!("git hooks: {HOOKS_DIR} installed; `cargo xtask verify` now runs before each commit");
    Ok(())
}

fn step(name: &str) {
    println!("\n== {name}");
}
