//! RULES.md R-PROC-1: no merge conflict is left half resolved. Git marks
//! the two sides of a conflict with lines of its own; one left in a Rust
//! file fails to compile, but one left in a Markdown document would be
//! committed, so every source and document is searched for them.

use std::path::Path;

use crate::error::{Error, Result};
use crate::files;

/// The lines Git writes around and between the sides of a conflict: the
/// first two begin a line, followed by a space and a name or by nothing;
/// the third is a line of its own. Built from parts, so this file holds
/// none of them.
const OURS: &str = concat!("<<<<<<", "<");
const THEIRS: &str = concat!(">>>>>>", ">");
const BETWEEN: &str = concat!("======", "=");

/// Checks that no Rust file or Markdown document holds a conflict marker.
pub fn check(root: &Path) -> Result<()> {
    let mut problems = Vec::new();
    let mut files = files::rust_files(root)?;
    files.extend(files::markdown_files(root)?);
    for file in &files {
        let found = markers(&files::read(root, file)?);
        problems.extend(found.into_iter().map(|line| format!("R-PROC-1: {file}:{line} holds a merge conflict marker")));
    }
    if problems.is_empty() {
        println!("merge markers: ok ({} files searched)", files.len());
        Ok(())
    } else {
        Err(Error::new(problems.join("\n")))
    }
}

/// Whether `line` is one Git writes for a conflict.
fn is_marker(line: &str) -> bool {
    let opens = |marker: &str| line.strip_prefix(marker).is_some_and(|rest| rest.is_empty() || rest.starts_with(' '));
    opens(OURS) || opens(THEIRS) || line == BETWEEN
}

/// The lines of `text`, numbered from 1, that are conflict markers.
fn markers(text: &str) -> Vec<usize> {
    (1..).zip(text.lines()).filter(|(_, line)| is_marker(line)).map(|(n, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-PROC-1 each of Git's three conflict markers is
    /// found, by its line number; a longer run of the same character, a
    /// marker not at a line's start, and Markdown's rules are not.
    #[test]
    fn conflict_markers_are_found() {
        let text = format!("a\n{OURS} HEAD\nb\n{BETWEEN}\nc\n{THEIRS} 1234 (wip)\n{OURS}\n");
        assert_eq!(markers(&text), vec![2, 4, 6, 7]);
        let clean = format!("Title\n========\n---\n x {OURS}\n{OURS}x\n{BETWEEN}=\n");
        assert_eq!(markers(&clean), Vec::<usize>::new());
    }

    /// Verifies: [TOOL] R-PROC-1 the check fails of a tree whose manual
    /// page or top-level document holds a marker, naming the file and line,
    /// and passes of the same tree without it.
    #[test]
    fn a_tree_with_a_marker_fails() {
        let root = std::env::temp_dir().join(format!("xtask-markers-{}", std::process::id()));
        let page = root.join(files::MANUAL_DIR).join("page.md");
        std::fs::create_dir_all(root.join(files::MANUAL_DIR)).unwrap();
        std::fs::write(root.join("README.md"), "# Read me\n").unwrap();
        std::fs::write(&page, format!("text\n{THEIRS} theirs\n")).unwrap();
        let error = check(&root).unwrap_err().to_string();
        assert!(error.contains("doc/page.md:2"), "{error}");
        std::fs::write(&page, "text\n").unwrap();
        assert!(check(&root).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
