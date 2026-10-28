//! RULES.md R-DOC-5: code never cites the work list. An item there moves,
//! or goes, when its work is done, so a citation of it would soon point
//! nowhere; code cites the standards, RULES.md's decisions, or
//! CONFORMANCE.md instead.

use std::path::Path;

use crate::error::{Error, Result};
use crate::files;

/// The work list's file name, which no source file may mention. Built from
/// two parts, so this file does not mention it either.
const WORK_LIST: &str = concat!("TODO", ".md");

/// Checks that no source file mentions the work list.
pub fn check(root: &Path) -> Result<()> {
    let mut problems = Vec::new();
    for file in files::rust_files(root)? {
        problems.extend(mentions(&files::read(root, &file)?).into_iter().map(|line| format!("R-DOC-5: {file}:{line} cites {WORK_LIST}")));
    }
    if problems.is_empty() {
        println!("citations: ok (no source file cites {WORK_LIST})");
        Ok(())
    } else {
        Err(Error::new(problems.join("\n")))
    }
}

/// The lines of `text`, numbered from 1, that mention the work list.
fn mentions(text: &str) -> Vec<usize> {
    (1..).zip(text.lines()).filter(|(_, line)| line.contains(WORK_LIST)).map(|(n, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-DOC-5 every line that mentions the work list is
    /// reported, by its number, and a text that does not is clean.
    #[test]
    fn mentions_of_the_work_list_are_found() {
        let text = format!("// fine\n// see {WORK_LIST} P6\nlet x = 1;\n/// and {WORK_LIST} again\n");
        assert_eq!(mentions(&text), vec![2, 4]);
        assert_eq!(mentions("// RULES.md D36\n// CONFORMANCE.md"), Vec::<usize>::new());
    }
}
