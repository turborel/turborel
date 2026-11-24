//! RULES.md R-SAFE-5: no thread is started but through the thread manager
//! (D55): in production code, a thread or a runtime of threads is made
//! only in the places that make them, each named. A scoped thread is one
//! too, and so is an import that names a maker of them. Tests may start
//! their own: a file of tests, a `#[cfg(test)]` module, and a file that a
//! `#[cfg(test)] mod name;` declares, with its own submodules, are not
//! read (RULES.md D972).

use std::path::Path;

use crate::error::{Error, Result};
use crate::files;

/// What makes a thread, or a runtime of threads.
const STARTS: &[&str] = &[
    "thread::spawn",
    "thread::Builder",
    "thread::scope",
    "spawn_scoped(",
    "Builder::new_multi_thread",
    "Builder::new_current_thread",
    "Runtime::new(",
];

/// An import of `std::thread`'s items.
const THREAD_IMPORT: &str = "use std::thread::";
/// What an import may name that starts a thread by its bare name.
const STARTERS: &[&str] = &["spawn", "Builder", "scope"];
/// The attribute of a module of tests.
const TEST_ATTRIBUTE: &str = "#[cfg(test)]";
/// A module declared, its body in a file of its own.
const MODULE: &str = "mod ";
/// The end of a Rust source file's name.
const RUST_SUFFIX: &str = ".rs";
/// The ends of a crate root's path, whose submodules are beside it.
const CRATE_ROOTS: [&str; 2] = ["/lib", "/main"];

/// The places that may: the database's jobs' threads, the executor's
/// workers, a path search's scoped workers, the readers of plug-in processes' pipes, the graph
/// algorithms' scoped workers, the server's runtime, the command-line
/// client's thread hearing Ctrl-C, and the plug-in SDK's reader of a
/// plug-in's standard input, in the plug-in's own process.
const MAKERS: &[&str] = &[
    "src/threads/worker.rs",
    "src/threads/pool.rs",
    "src/threads/morsels.rs",
    "src/threads/pipes.rs",
    "src/graph/algo/parallel.rs",
    "server/src/runtime.rs",
    "cli/src/interrupt.rs",
    "plugin/src/input.rs",
];

/// Checks that no production source but `MAKERS` starts a thread.
pub fn check(root: &Path) -> Result<()> {
    let mut problems = Vec::new();
    let sources = files::rust_files(root)?;
    let mut tests = Vec::new();
    for file in sources.iter().filter(|f| production(f)) {
        tests.extend(test_modules(file, &files::read(root, file)?));
    }
    let read = |f: &&String| production(f) && !MAKERS.contains(&f.as_str()) && !tests.iter().any(|t| within(f, t));
    for file in sources.iter().filter(read) {
        let text = files::read(root, file)?;
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
        // A module of tests with its body here; one in a file of its own
        // (`test_modules`) leaves the next line read.
        let bodied = |next: &str| next.trim_start().starts_with(MODULE) && !next.trim_end().ends_with(';');
        if !in_tests && line.trim() == TEST_ATTRIBUTE && lines.peek().is_some_and(|(_, next)| bodied(next)) {
            in_tests = true;
            continue;
        }
        if in_tests {
            depth = depth.saturating_add(braces(line));
            in_tests = depth > 0;
            continue;
        }
        if STARTS.iter().any(|s| line.contains(s)) || imports_a_starter(line) {
            found.push(n);
        }
    }
    found
}

/// Whether `line` imports from `std::thread` an item that starts a thread
/// by its bare name, as `use std::thread::{Builder, scope};` does.
fn imports_a_starter(line: &str) -> bool {
    line.trim_start()
        .strip_prefix(THREAD_IMPORT)
        .is_some_and(|names| names.split(|c: char| !c.is_alphanumeric() && c != '_').any(|name| STARTERS.contains(&name)))
}

/// The files of the modules of tests that `text`, the file `file`,
/// declares by `#[cfg(test)]` and `mod name;`, their bodies in files of
/// their own: `name.rs` in the directory of `file`'s submodules.
fn test_modules(file: &str, text: &str) -> Vec<String> {
    let dir = file.strip_suffix(RUST_SUFFIX).unwrap_or(file);
    let dir = CRATE_ROOTS.iter().find_map(|root| dir.strip_suffix(root)).unwrap_or(dir);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines
        .windows(2)
        .filter(|pair| pair.first() == Some(&TEST_ATTRIBUTE))
        .filter_map(|pair| pair.get(1)?.strip_prefix(MODULE)?.strip_suffix(';'))
        .map(|name| format!("{dir}/{name}{RUST_SUFFIX}"))
        .collect()
}

/// Whether `file` is the module of tests `test`, or one of its submodules.
fn within(file: &str, test: &str) -> bool {
    file == test || test.strip_suffix(RUST_SUFFIX).is_some_and(|dir| file.starts_with(&format!("{dir}/")))
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

    /// Verifies: [TOOL] R-SAFE-5 RULES.md D972: a scoped thread is found,
    /// by `std::thread::scope`, by `spawn_scoped` and by an import naming
    /// `spawn`, `Builder` or `scope`; an import of other items is not.
    /// MC/DC of `imports_a_starter`: an import of `std::thread` naming a
    /// starter is found, one naming none is not, and a starter's name not
    /// in an import of `std::thread` is not.
    #[test]
    fn scoped_threads_and_imports_of_starters_are_found() {
        let text = "fn a() {\n    std::thread::scope(|s| s.spawn(f));\n    b.spawn_scoped(s, g);\n}\n";
        assert_eq!(starts(text), vec![2, 3]);
        for found in ["use std::thread::{Builder, scope};", "use std::thread::spawn;", "    use std::thread::{self, Builder};"] {
            assert!(imports_a_starter(found), "{found}");
        }
        for passed in ["use std::thread::{self, JoinHandle};", "use std::thread::ThreadId;", "use crate::spawn;", "let scope = 1;"] {
            assert!(!imports_a_starter(passed), "{passed}");
        }
    }

    /// Verifies: [TOOL] R-SAFE-5 RULES.md D972: a module of tests a file
    /// declares in a file of its own is found, beside the file or, of a
    /// crate's root, in its directory; that file and its submodules are of
    /// tests, others not. A module not of tests, or one with a body, is
    /// not found; the line after one declared so is read, as a regression
    /// test: it was passed over before the fix.
    #[test]
    fn files_of_modules_of_tests_are_told_apart() {
        let text = "mod kept;\n#[cfg(test)]\nmod costs;\nstd::thread::spawn(f);\n#[cfg(test)]\nmod tests {\n}\n";
        assert_eq!(test_modules("src/engine/rows/zones.rs", text), ["src/engine/rows/zones/costs.rs"]);
        assert_eq!(starts(text), vec![4], "the line after a module of a file of its own is read");
        assert_eq!(test_modules("src/lib.rs", "#[cfg(test)]\nmod tests;\n"), ["src/tests.rs"]);
        let costs = "src/engine/rows/zones/costs.rs";
        assert!(within(costs, costs) && within("src/engine/rows/zones/costs/more.rs", costs));
        assert!(!within("src/engine/rows/zones/kept.rs", costs) && !within("src/engine/rows/zones/costs_of.rs", costs));
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
