//! RULES.md R-DEV-2: the lint baselines only shrink, counted per file and lint.
//!
//! A file's `#![cfg_attr(not(lint_audit), expect(...))]` baseline hides the
//! lints it lists, so on its own it would also hide new violations of those
//! lints. This check builds with `--cfg lint_audit`, which turns the baselines
//! off, counts every violation per file and lint, and compares the counts with
//! `xtask/lint-baseline.txt`: a count may fall, never rise.
//!
//! With no baseline in any source file and none recorded, the audit build
//! would be the lint build again, so it is skipped: the lints of `verify`
//! already deny every violation.
//!
//! Counts are kept per baseline owner: a module in `src/<name>/` is covered by
//! the baseline of `src/<name>.rs`, so moving code within a module tree changes
//! no count.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::command::{self, RUSTFLAGS_VAR, TARGET_DIR_VAR};
use crate::error::{Error, Result};
use crate::files::{self, LIBRARY_DIR, RUST_EXTENSION};
use crate::json::{self, Json};

/// The recorded counts, relative to the workspace root.
const BASELINE_FILE: &str = "xtask/lint-baseline.txt";
/// A separate build directory, so the audit build does not invalidate the normal one.
const TARGET_DIR: &str = "target/lint-audit";
/// Turns the baselines off.
const AUDIT_FLAGS: &str = "--cfg lint_audit";
/// How a file-level baseline starts (RULES.md R-DEV-2).
const BASELINE_ATTRIBUTE: &str = "#![cfg_attr(not(lint_audit)";
// Cargo's `--message-format=json` messages: the fields read, and the kind of
// message that carries a diagnostic.
const REASON_KEY: &str = "reason";
const COMPILER_MESSAGE: &str = "compiler-message";
const MESSAGE_KEY: &str = "message";
const CODE_KEY: &str = "code";
const SPANS_KEY: &str = "spans";
const IS_PRIMARY_KEY: &str = "is_primary";
const FILE_NAME_KEY: &str = "file_name";
const LINE_START_KEY: &str = "line_start";
const COLUMN_START_KEY: &str = "column_start";
const HEADER: &str = "# Lint violations the baselines still hide, per file and lint (RULES.md R-DEV-2).
# Counts may only fall. Update with `cargo xtask lint-audit --write` after removing violations.
";

/// Counts per (file, lint).
type Counts = BTreeMap<(String, String), usize>;

/// Fails if any count rose above, or fell below, its recorded value.
pub fn check(root: &Path) -> Result<()> {
    let recorded = recorded(root)?;
    if recorded.is_empty() && !any_baseline(root)? {
        println!("lint baseline: none in the sources, so the lints deny every violation");
        return Ok(());
    }
    let problems = compare(&recorded, &current(root)?);
    if problems.is_empty() {
        println!("lint baseline: ok");
        Ok(())
    } else {
        Err(Error::new(problems.join("\n")))
    }
}

/// Whether any source file has a file-level baseline.
fn any_baseline(root: &Path) -> Result<bool> {
    for file in files::rust_files(root)? {
        if has_baseline(&files::read(root, &file)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `text` has a line that starts a file-level baseline.
fn has_baseline(text: &str) -> bool {
    text.lines().any(|line| line.trim_start().starts_with(BASELINE_ATTRIBUTE))
}

/// Records the current counts, if none rose.
pub fn write(root: &Path) -> Result<()> {
    let (recorded, current) = (recorded(root)?, current(root)?);
    let rises: Vec<String> = compare(&recorded, &current).into_iter().filter(|p| p.contains(RISE)).collect();
    if !rises.is_empty() {
        return Err(Error::new(rises.join("\n")));
    }
    let mut text = String::from(HEADER);
    for ((file, lint), n) in &current {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "{file} {lint} {n}");
    }
    let path = root.join(BASELINE_FILE);
    fs::write(&path, text).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("lint baseline: {} entries written", current.len());
    Ok(())
}

/// Marks a problem that `write` must refuse.
const RISE: &str = "more than its baseline";

/// What differs between `recorded` and `current`.
fn compare(recorded: &Counts, current: &Counts) -> Vec<String> {
    let keys: BTreeSet<&(String, String)> = recorded.keys().chain(current.keys()).collect();
    let mut problems = Vec::new();
    for key in keys {
        let (was, now) = (recorded.get(key).copied().unwrap_or(0), current.get(key).copied().unwrap_or(0));
        let (file, lint) = key;
        if now > was {
            problems.push(format!("R-DEV-2: {file} has {now} {lint}, {RISE} of {was}"));
        } else if now < was {
            problems.push(format!("R-DEV-2: {file} has {now} {lint}, below its baseline of {was}: run `cargo xtask lint-audit --write`"));
        }
    }
    problems
}

fn recorded(root: &Path) -> Result<Counts> {
    let path = root.join(BASELINE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    parse_baseline(&text)
}

/// Reads lines `file lint count`; `#` starts a comment line.
fn parse_baseline(text: &str) -> Result<Counts> {
    let mut counts = Counts::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [file, lint, count] = fields.as_slice() else {
            return Err(Error::new(format!("{BASELINE_FILE}: expected `file lint count`, found {line:?}")));
        };
        let count = count.parse().map_err(|_| Error::new(format!("{BASELINE_FILE}: bad count in {line:?}")))?;
        counts.insert(((*file).to_owned(), (*lint).to_owned()), count);
    }
    Ok(counts)
}

/// Builds with the baselines off and counts each violation once.
fn current(root: &Path) -> Result<Counts> {
    let target_dir = root.join(TARGET_DIR);
    let envs = [(TARGET_DIR_VAR, target_dir.as_os_str()), (RUSTFLAGS_VAR, OsStr::new(AUDIT_FLAGS))];
    let output = command::cargo_output(root, &["clippy", "--workspace", "--all-targets", "--quiet", "--message-format=json"], &envs)?;
    let mut seen = BTreeSet::new();
    for line in output.lines().filter(|l| l.starts_with('{')) {
        if let Some(violation) = violation(&json::parse(line)?) {
            seen.insert(violation);
        }
    }
    let mut counts = Counts::new();
    for (file, lint, _, _) in seen {
        let count = counts.entry((owner(&file), lint)).or_insert(0);
        *count = count.saturating_add(1);
    }
    Ok(counts)
}

/// The file whose baseline covers `file`: `src/<name>.rs` for any file under
/// `src/<name>/`, otherwise the file itself.
fn owner(file: &str) -> String {
    match file.strip_prefix(LIBRARY_DIR).and_then(|rest| rest.strip_prefix('/')).and_then(|rest| rest.split_once('/')) {
        Some((module, _)) => format!("{LIBRARY_DIR}/{module}.{RUST_EXTENSION}"),
        None => file.to_owned(),
    }
}

/// A lint violation in a cargo message: (file, lint, line, column) of its
/// primary span. The same violation is reported once per target that compiles
/// the file, so the position tells repeats apart.
fn violation(message: &Json) -> Option<(String, String, String, String)> {
    if message.get(REASON_KEY).and_then(Json::text) != Some(COMPILER_MESSAGE) {
        return None;
    }
    let diagnostic = message.get(MESSAGE_KEY)?;
    let lint = diagnostic.get(CODE_KEY)?.get(CODE_KEY)?.text()?;
    let span = diagnostic.get(SPANS_KEY)?.items().iter().find(|s| s.get(IS_PRIMARY_KEY) == Some(&Json::Bool(true)))?;
    let position = |field: &str| match span.get(field) {
        Some(Json::Number(n)) => n.clone(),
        Some(_) | None => String::new(),
    };
    Some((span.get(FILE_NAME_KEY)?.text()?.to_owned(), lint.to_owned(), position(LINE_START_KEY), position(COLUMN_START_KEY)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(entries: &[(&str, &str, usize)]) -> Counts {
        entries.iter().map(|(f, l, n)| (((*f).to_owned(), (*l).to_owned()), *n)).collect()
    }

    /// Verifies: [TOOL] R-DEV-2 a count that rose, a new lint, and a count that
    /// fell are each reported; equal counts pass.
    #[test]
    fn counts_may_only_fall_and_must_be_recorded() {
        let recorded = counts(&[("a.rs", "x", 2), ("a.rs", "y", 1), ("b.rs", "x", 3)]);
        let current = counts(&[("a.rs", "x", 3), ("a.rs", "y", 1), ("b.rs", "x", 1), ("c.rs", "z", 1)]);
        let problems = compare(&recorded, &current);
        assert_eq!(problems.iter().filter(|p| p.contains(RISE)).count(), 2, "{problems:?}"); // a.rs x, c.rs z
        assert_eq!(problems.len(), 3, "{problems:?}"); // and b.rs x fell
        assert_eq!(compare(&recorded, &recorded), Vec::<String>::new());
    }

    /// Verifies: [TOOL] R-DEV-2 a module's violations count against the file
    /// whose baseline covers it, so moving code within a module tree is neutral.
    #[test]
    fn violations_count_against_the_baseline_owner() {
        assert_eq!(owner("src/binder/set_functions.rs"), "src/binder.rs");
        assert_eq!(owner("src/db/cursor/open.rs"), "src/db.rs");
        assert_eq!(owner("src/value.rs"), "src/value.rs");
        assert_eq!(owner("xtask/src/json.rs"), "xtask/src/json.rs");
    }

    /// Verifies: [TOOL] R-DEV-2: a file has a baseline only if a line starts
    /// with the baseline attribute; one named in a comment or a string does
    /// not count.
    #[test]
    fn baselines_are_found_only_as_attributes() {
        assert!(has_baseline("//! doc\n#![cfg_attr(not(lint_audit), expect(clippy::panic))]\nfn f() {}"));
        assert!(!has_baseline("//! `#![cfg_attr(not(lint_audit), expect(...))]` hides lints\nfn f() {}"));
        assert!(!has_baseline("const A: &str = \"#![cfg_attr(not(lint_audit)\";"));
    }

    /// Verifies: [TOOL] R-DEV-2 the baseline file's format, and its errors.
    #[test]
    fn baseline_file_format() {
        let parsed = parse_baseline("# comment\n\nsrc/a.rs clippy::panic 2\n").unwrap();
        assert_eq!(parsed, counts(&[("src/a.rs", "clippy::panic", 2)]));
        assert!(parse_baseline("src/a.rs clippy::panic").is_err());
        assert!(parse_baseline("src/a.rs clippy::panic two").is_err());
    }

    /// Verifies: [TOOL] R-DEV-2 a violation is read from the diagnostic's own
    /// code and primary span, not from its children.
    #[test]
    fn violations_come_from_the_primary_span() {
        let line = r#"{"reason":"compiler-message","message":{"children":[{"code":null,"spans":[{"file_name":"other.rs","is_primary":true}]}],"spans":[{"file_name":"src/a.rs","is_primary":false,"line_start":1,"column_start":1},{"file_name":"src/a.rs","is_primary":true,"line_start":7,"column_start":3}],"code":{"code":"clippy::panic"}}}"#;
        let found = violation(&json::parse(line).unwrap());
        assert_eq!(found, Some(("src/a.rs".into(), "clippy::panic".into(), "7".into(), "3".into())));
        assert_eq!(violation(&json::parse(r#"{"reason":"build-finished","success":true}"#).unwrap()), None);
    }
}
