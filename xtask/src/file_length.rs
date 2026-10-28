//! RULES.md R-SIZE-2: source files stay within `MAX_LINES` lines.

use std::path::Path;

use crate::baseline::FILE_LENGTH_BASELINE;
use crate::error::{Error, Result};
use crate::files;

/// Longest a Rust source file may be, in lines.
pub const MAX_LINES: usize = 600;

/// Checks every source file against its limit, and every baseline entry
/// against a file that still exists.
pub fn check(root: &Path) -> Result<()> {
    let sources = files::rust_files(root)?;
    let mut problems = Vec::new();
    for file in &sources {
        let lines = files::read(root, file)?.lines().count();
        if let Some(problem) = problem(file, lines, limit(file)) {
            problems.push(problem);
        }
    }
    for (file, _) in FILE_LENGTH_BASELINE.iter().filter(|(file, _)| !sources.iter().any(|s| s == file)) {
        problems.push(format!("R-SIZE-2: {file} no longer exists; delete its FILE_LENGTH_BASELINE entry"));
    }
    if problems.is_empty() {
        println!("file length: ok (limit {MAX_LINES} lines)");
        Ok(())
    } else {
        Err(Error::new(problems.join("\n")))
    }
}

/// The limit for `file`: its baseline entry, or `MAX_LINES`.
fn limit(file: &str) -> usize {
    FILE_LENGTH_BASELINE.iter().find(|(name, _)| *name == file).map_or(MAX_LINES, |(_, lines)| *lines)
}

/// What is wrong with `file`, `lines` long, under `limit`: too long, or a
/// baseline entry that no longer records its size. A baseline only shrinks, so
/// a file that got shorter lowers its entry, and one back within `MAX_LINES`
/// loses it (RULES.md R-DEV-2).
fn problem(file: &str, lines: usize, limit: usize) -> Option<String> {
    if lines > limit {
        Some(format!("R-SIZE-2: {file} has {lines} lines; the limit is {limit}"))
    } else if limit <= MAX_LINES {
        None
    } else if lines <= MAX_LINES {
        Some(format!("R-SIZE-2: {file} is within {MAX_LINES} lines now; delete its FILE_LENGTH_BASELINE entry"))
    } else if lines < limit {
        Some(format!("R-SIZE-2: {file} has shrunk to {lines} lines; lower its FILE_LENGTH_BASELINE entry to {lines}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVER: usize = 700;
    const BASELINE: usize = 800;

    /// Verifies: [TOOL] R-SIZE-2 a file over its limit is reported.
    #[test]
    fn too_long_is_reported() {
        assert!(problem("a.rs", MAX_LINES + 1, MAX_LINES).is_some());
        assert!(problem("a.rs", BASELINE + 1, BASELINE).is_some());
    }

    /// Verifies: [TOOL] R-SIZE-2, R-DEV-2 a baseline entry must record its file's
    /// size: one back within `MAX_LINES` is deleted, one above a shrunk file is
    /// lowered.
    ///
    /// Decisions in order: `limit <= MAX_LINES` (no entry), `lines <= MAX_LINES`,
    /// `lines < limit`. Each case below changes exactly one of them.
    #[test]
    fn stale_baseline_is_reported() {
        assert!(problem("a.rs", MAX_LINES, MAX_LINES).is_none()); // no entry
        assert!(problem("a.rs", MAX_LINES, BASELINE).is_some()); // back within MAX_LINES
        assert!(problem("a.rs", OVER, BASELINE).is_some()); // shrunk: lower the entry
        assert!(problem("a.rs", BASELINE, BASELINE).is_none()); // entry exact
    }
}
