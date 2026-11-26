//! RULES.md R-BENCH-1: the suites of a benchmark that `cargo xtask bench`
//! runs alone by a name of their own, and the options each takes. A suite
//! is part of a benchmark's target, chosen by the variable the target
//! reads, so it needs no target of its own; its options reach it as
//! variables too, since `cargo bench` hands a target no arguments this
//! tool could name.

/// A suite: the name it is run by, the benchmark it is part of, and the
/// variable and value that choose it there.
#[derive(Debug, PartialEq, Eq)]
pub struct Suite {
    pub name: &'static str,
    pub bench: &'static str,
    pub chosen_by: (&'static str, &'static str),
}

/// The suites: `weblog`, the web log's queries of `bench/analytics/weblog.rs`.
const SUITES: [Suite; 1] = [Suite { name: "weblog", bench: "analytics", chosen_by: ("TURBOREL_BENCH_ANALYTICS_ONLY", "weblog") }];

/// The options of the suites, each with the variable its value is handed
/// in: the rows made, the queries run, the threads a query runs on, the
/// query run in a loop for a profiler and for how many seconds, the
/// queries whose plans are printed, and the directory the table is kept
/// in from one run to the next.
const SETTINGS: [(&str, &str); 7] = [
    ("--rows", "TURBOREL_BENCH_WEBLOG_ROWS"),
    ("--only", "TURBOREL_BENCH_WEBLOG_ONLY"),
    ("--threads", "TURBOREL_BENCH_WEBLOG_THREADS"),
    ("--profile", "TURBOREL_BENCH_WEBLOG_PROFILE"),
    ("--secs", "TURBOREL_BENCH_WEBLOG_SECS"),
    ("--plan", "TURBOREL_BENCH_WEBLOG_PLAN"),
    ("--dir", "TURBOREL_BENCH_WEBLOG_DIR"),
];

/// The suite run as `name`, if it names one.
pub fn named(name: &str) -> Option<&'static Suite> {
    SUITES.iter().find(|suite| suite.name == name)
}

/// The variable the value of `option` is handed in, if it is a suite's.
pub fn setting(option: &str) -> Option<&'static str> {
    SETTINGS.iter().find(|(name, _)| *name == option).map(|(_, variable)| *variable)
}

/// The suites' options, as the usage shows them.
pub fn usage() -> String {
    SETTINGS.iter().fold(String::new(), |usage, (name, _)| usage + " [" + name + " VALUE]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-BENCH-1 a suite is found by its name and names
    /// the benchmark it is part of; a benchmark's own name is no suite; a
    /// suite's option names its variable, and another option none.
    #[test]
    fn suites_and_their_options_are_found() {
        assert_eq!(named("weblog").map(|suite| suite.bench), Some("analytics"));
        assert_eq!(named("analytics"), None);
        assert_eq!(setting("--only"), Some("TURBOREL_BENCH_WEBLOG_ONLY"));
        assert_eq!(setting("--save"), None);
        assert!(usage().contains("[--profile VALUE]"));
    }
}
