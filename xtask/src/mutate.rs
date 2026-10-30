//! RULES.md R-TEST-9: `cargo xtask mutate` checks the tests by mutation.
//!
//! Each mutation of `MUTATIONS` (`mutate/list.rs`) breaks one mechanism of
//! the core: it replaces a snippet of one source file, found there exactly
//! once, and names the tests that must then fail. The command first runs
//! the named tests on the code as it is, where each must pass; then, one
//! mutation at a time, it writes the mutation, builds, runs its tests, and
//! writes the file back. It reports a mutation no test caught, a named test
//! that passed under its mutation, and a mutation that no longer applies
//! (its snippet changed) or does not build, and fails if there is any.
//!
//! The file is written back whatever happens: by `Applied::restore`, by
//! `Applied`'s `Drop` if the command fails or panics first, and, if the
//! process itself was stopped (Ctrl-C), at the start of the next run, from
//! the copy kept in `BACKUP_DIR` while a mutation is in place.

mod list;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::command::{self, Finished};
use crate::error::{Error, Result};

pub use list::MUTATIONS;

/// The package the mutations break, whose tests they run.
const PACKAGE: &str = "turborel";
/// The integration test binary of `PACKAGE`.
const INTEGRATION: &str = "integration";
/// Where the original of the file a mutation changed is kept while the
/// mutation is in place, relative to the workspace root.
const BACKUP_DIR: &str = "target/mutate";
/// In `BACKUP_DIR`: the path of the file changed, relative to the root...
const BACKUP_PATH: &str = "path";
/// ...and its original contents.
const BACKUP_CONTENTS: &str = "original";
/// How libtest reports a test, without `--quiet`: `test NAME ... ok`.
const TEST_LINE: &str = "test ";
const PASSED: &str = " ... ok";
const FAILED: &str = " ... FAILED";
/// The lines of a failed build shown.
const BUILD_LINES: usize = 20;

/// One mutation: `original`, found once in `file`, replaced by
/// `replacement`, breaks what `breaks` says, and each test of `integration`
/// (of the integration binary) and `unit` (of the library's unit tests),
/// full paths, must fail.
pub struct Mutation {
    pub name: &'static str,
    pub breaks: &'static str,
    pub file: &'static str,
    pub original: &'static str,
    pub replacement: &'static str,
    pub integration: &'static [&'static str],
    pub unit: &'static [&'static str],
}

/// The test binaries of `PACKAGE` a mutation names tests of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Integration,
    Unit,
}

impl Target {
    /// The arguments of `cargo test` that choose the binary.
    fn args(self) -> &'static [&'static str] {
        match self {
            Target::Integration => &["--test", INTEGRATION],
            Target::Unit => &["--lib"],
        }
    }
}

impl Mutation {
    /// Its tests of `target`.
    fn tests(&self, target: Target) -> &'static [&'static str] {
        match target {
            Target::Integration => self.integration,
            Target::Unit => self.unit,
        }
    }
}

/// Runs `cargo xtask mutate`: every mutation, or the one named.
pub fn run(root: &Path, args: &[&str]) -> Result<()> {
    restore_leftover(root)?;
    let chosen = chosen(&MUTATIONS, args)?;
    for m in &chosen {
        apply(&fs::read_to_string(root.join(m.file)).map_err(|e| Error::new(format!("cannot read {}: {e}", m.file)))?, m)?;
    }
    println!("mutate: the named tests, on the code as it is");
    let before = unmutated_problems(root, &chosen)?;
    if !before.is_empty() {
        return Err(Error::new(before.join("\n")));
    }
    let mut problems = Vec::new();
    for m in &chosen {
        println!("mutate: {} ({})", m.name, m.breaks);
        let found = check(root, m)?;
        println!("  {}", found.as_deref().unwrap_or("caught"));
        problems.extend(found.map(|p| format!("{}: {p}", m.name)));
    }
    if problems.is_empty() {
        println!("\nmutate: all {} mutations caught", chosen.len());
        Ok(())
    } else {
        Err(Error::new(format!("R-TEST-9: the tests missed mutations:\n{}", problems.join("\n"))))
    }
}

/// The mutations `args` choose: all, or the one named.
fn chosen<'a>(all: &'a [Mutation], args: &[&str]) -> Result<Vec<&'a Mutation>> {
    match args {
        [] => Ok(all.iter().collect()),
        [name] => all.iter().find(|m| m.name == *name).map(|m| vec![m]).ok_or_else(|| {
            Error::new(format!("no mutation {name}; the mutations: {}", all.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")))
        }),
        _ => Err(Error::new("usage: cargo xtask mutate [NAME]")),
    }
}

/// `source` with `m` applied; an error if its original is not found in it
/// exactly once.
fn apply(source: &str, m: &Mutation) -> Result<String> {
    match source.matches(m.original).count() {
        1 => Ok(source.replacen(m.original, m.replacement, 1)),
        0 => Err(Error::new(format!("{}: its original is no longer in {}; bring the mutation up to date", m.name, m.file))),
        n => Err(Error::new(format!("{}: its original is in {} {n} times; quote more of it", m.name, m.file))),
    }
}

/// What is wrong before any mutation: a named test that does not pass, or
/// is not there.
fn unmutated_problems(root: &Path, chosen: &[&Mutation]) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for target in [Target::Integration, Target::Unit] {
        let mut names: Vec<&str> = chosen.iter().flat_map(|m| m.tests(target)).copied().collect();
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            continue;
        }
        let seen = outcomes(&run_tests(root, target, &names)?.stdout);
        problems.extend(
            names.iter().filter(|n| seen.get(**n) != Some(&true)).map(|n| format!("{n} does not pass, or is not a test, unmutated")),
        );
    }
    Ok(problems)
}

/// Applies `m`, builds and runs its tests, and writes the file back: what
/// is wrong, or `None` if every named test failed.
fn check(root: &Path, m: &Mutation) -> Result<Option<String>> {
    let applied = Applied::new(root, m)?;
    let targets: Vec<Target> = [Target::Integration, Target::Unit].into_iter().filter(|t| !m.tests(*t).is_empty()).collect();
    let mut build_args = vec!["test", "--package", PACKAGE, "--no-run"];
    build_args.extend(targets.iter().flat_map(|t| t.args()));
    let built = command::cargo_finished(root, &build_args)?;
    let mut seen = HashMap::new();
    if built.success {
        for target in &targets {
            seen.extend(outcomes(&run_tests(root, *target, m.tests(*target))?.stdout));
        }
    }
    applied.restore()?;
    if !built.success {
        return Ok(Some(format!("does not build:\n{}", tail(&built.stderr, BUILD_LINES))));
    }
    Ok(verdict(m, &seen))
}

/// Runs the tests `names` of `target`, exactly those.
fn run_tests(root: &Path, target: Target, names: &[&str]) -> Result<Finished> {
    let mut args = vec!["test", "--package", PACKAGE];
    args.extend(target.args());
    args.extend(["--", "--exact"]);
    args.extend(names);
    command::cargo_finished(root, &args)
}

/// Each test libtest reported in `stdout`, and whether it passed.
fn outcomes(stdout: &str) -> HashMap<String, bool> {
    let mut found = HashMap::new();
    for line in stdout.lines().filter_map(|l| l.strip_prefix(TEST_LINE)) {
        if let Some(name) = line.strip_suffix(PASSED) {
            found.insert(name.to_owned(), true);
        } else if let Some(name) = line.strip_suffix(FAILED) {
            found.insert(name.to_owned(), false);
        }
    }
    found
}

/// What is wrong with `m`'s tests, given their `outcomes` under it: none
/// failed (it survived), or some did not; `None` if all failed.
fn verdict(m: &Mutation, outcomes: &HashMap<String, bool>) -> Option<String> {
    let named: Vec<&str> = m.integration.iter().chain(m.unit).copied().collect();
    let not_failed: Vec<&str> = named.iter().copied().filter(|n| outcomes.get(*n) != Some(&false)).collect();
    if not_failed.is_empty() {
        None
    } else if not_failed.len() == named.len() {
        Some("survived: no named test failed".to_owned())
    } else {
        Some(format!("these named tests did not fail: {}", not_failed.join(", ")))
    }
}

/// The last `n` lines of `text`.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines.get(lines.len().saturating_sub(n)..).unwrap_or_default().join("\n")
}

/// A mutation in place: its file's original contents, kept in memory and
/// in `BACKUP_DIR` until they are written back.
struct Applied {
    path: PathBuf,
    original: String,
    backup: PathBuf,
    restored: bool,
}

impl Applied {
    /// Keeps `m`'s file's contents, then writes it mutated.
    fn new(root: &Path, m: &Mutation) -> Result<Applied> {
        let path = root.join(m.file);
        let original = fs::read_to_string(&path).map_err(|e| Error::new(format!("cannot read {}: {e}", m.file)))?;
        let mutated = apply(&original, m)?;
        let backup = root.join(BACKUP_DIR);
        fs::create_dir_all(&backup).map_err(|e| Error::new(format!("cannot create {}: {e}", backup.display())))?;
        write(&backup.join(BACKUP_CONTENTS), &original)?;
        write(&backup.join(BACKUP_PATH), m.file)?;
        let applied = Applied { path, original, backup, restored: false };
        write(&applied.path, &mutated)?;
        Ok(applied)
    }

    /// Writes the original back, then forgets the copy.
    fn restore(mut self) -> Result<()> {
        self.write_back()
    }

    fn write_back(&mut self) -> Result<()> {
        write(&self.path, &self.original)?;
        self.restored = true;
        forget_backup(&self.backup)
    }
}

impl Drop for Applied {
    /// A mutation left in place by a failure or a panic is undone here.
    fn drop(&mut self) {
        if !self.restored && self.write_back().is_err() {
            eprintln!("mutate: could not write {} back; its original is kept in {}", self.path.display(), self.backup.display());
        }
    }
}

/// Writes back a file a run stopped from outside left mutated, from the
/// copy in `BACKUP_DIR`.
fn restore_leftover(root: &Path) -> Result<()> {
    let backup = root.join(BACKUP_DIR);
    let Ok(file) = fs::read_to_string(backup.join(BACKUP_PATH)) else { return Ok(()) };
    let original = fs::read_to_string(backup.join(BACKUP_CONTENTS))
        .map_err(|e| Error::new(format!("{} names {file}, but its original cannot be read: {e}", backup.display())))?;
    write(&root.join(&file), &original)?;
    println!("mutate: wrote back {file}, left mutated by a run that was stopped");
    forget_backup(&backup)
}

/// Removes the copy of a file written back: its path first, so a copy
/// without its path is never taken for a file to write back.
fn forget_backup(backup: &Path) -> Result<()> {
    for name in [BACKUP_PATH, BACKUP_CONTENTS] {
        fs::remove_file(backup.join(name)).map_err(|e| Error::new(format!("cannot remove {}: {e}", backup.join(name).display())))?;
    }
    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests;
