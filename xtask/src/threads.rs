//! RULES.md R-SAFE-5: no thread is started but through the thread manager
//! (D55): in production code, a thread or a runtime of threads is made
//! only in the places that make them, each named. Tests may start their
//! own: a file of tests, and a `#[cfg(test)]` module, are not read.

use std::path::Path;

use crate::error::{Error, Result};
use crate::files;

/// What makes a thread, or a runtime of threads.
const STARTS: &[&str] = &["thread::spawn", "thread::Builder", "Builder::new_multi_thread", "Builder::new_current_thread", "Runtime::new("];

/// The places that may: the database's jobs' threads, the server's
/// runtime, and the command-line client's thread hearing Ctrl-C.
const MAKERS: &[&str] = &["src/threads/worker.rs", "server/src/runtime.rs", "cli/src/interrupt.rs"];

/// Checks that no production source but `MAKERS` starts a thread.
pub fn check(root: &Path) -> Result<()> {
    let mut problems = Vec::new();
    for file in files::rust_files(root)?.into_iter().filter(|f| production(f) && !MAKERS.contains(&f.as_str())) {
        let text = files::read(root, &file)?;
        problems
            .extend(starts(&text).into_iter().map(|line| format!("R-SAFE-5: {file}:{line} starts a thread outside the thread manager")));
    }
    if problems.is_empty() {
        println!("threads: ok (threads started only by {})", MAKERS.join(", "));
        Ok(())
    } else {
        Err(Error::new(problems.join("\n")))
    }
}

/// Whether `file` is production code: not of the tests, the benchmarks or
/// the tools, nor a file of tests.
fn production(file: &str) -> bool {
    let tests = file.starts_with("test/") || file.contains("/test/") || file.ends_with("/tests.rs") || file.contains("/tests/");
    !tests && !file.starts_with("bench/") && !file.starts_with("xtask/") && !file.starts_with("support/")
}

/// The lines of `text`, numbered from 1, that start a thread, but within
/// a `#[cfg(test)]` module.
fn starts(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let (mut in_tests, mut depth) = (false, 0_i64);
    let mut lines = (1..).zip(text.lines()).peekable();
    while let Some((n, line)) = lines.next() {
        if !in_tests && line.trim() == "#[cfg(test)]" && lines.peek().is_some_and(|(_, next)| next.trim_start().starts_with("mod ")) {
            in_tests = true;
            continue;
        }
        if in_tests {
            depth = depth.saturating_add(braces(line));
            in_tests = depth > 0;
            continue;
        }
        if STARTS.iter().any(|s| line.contains(s)) {
            found.push(n);
        }
    }
    found
}

/// How much `line` opens braces: those opened less those closed.
fn braces(line: &str) -> i64 {
    line.chars().fold(0_i64, |n, c| match c {
        '{' => n.saturating_add(1),
        '}' => n.saturating_sub(1),
        _ => n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-SAFE-5 a thread or runtime started is found, by
    /// its line; one within a `#[cfg(test)]` module is not, wherever the
    /// module is, the code after it read again.
    #[test]
    fn threads_started_are_found_outside_tests() {
        let text = "fn a() {\n    std::thread::spawn(f);\n}\n#[cfg(test)]\nmod tests {\n    fn t() {\n        thread::spawn(g);\n    }\n}\nfn b() {\n    Runtime::new()\n}\n";
        assert_eq!(starts(text), vec![2, 11]);
        assert_eq!(starts("fn c() { tokio::spawn(task); }\n"), Vec::<usize>::new(), "a task is no thread");
    }

    /// Verifies: [TOOL] R-SAFE-5 the tests, benchmarks, tools and the
    /// test support are not production code; the library's sources are.
    #[test]
    fn production_code_is_told_apart() {
        for file in [
            "test/integration/threads.rs",
            "server/test/main.rs",
            "src/engine/hash/tests.rs",
            "bench/scaling.rs",
            "xtask/src/main.rs",
            "support/src/server.rs",
        ] {
            assert!(!production(file), "{file}");
        }
        assert!(production("src/db/maintenance.rs") && production("server/src/server.rs"));
    }
}
