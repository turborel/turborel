//! The suites of a benchmark that `cargo xtask bench` runs alone by a name
//! of their own, and the options each takes (RULES.md R-BENCH-1). A suite
//! is part of a benchmark's target, chosen by the variable the target
//! reads, so it needs no target of its own. Its options reach it as
//! variables too, since `cargo bench` hands a target no arguments this
//! tool could name.

/// A suite, with the name it is run by, the benchmark it is part of, and
/// the variable and value that choose it there.
#[derive(Debug, PartialEq, Eq)]
pub struct Suite {
    pub name: &'static str,
    pub bench: &'static str,
    pub chosen_by: (&'static str, &'static str),
}

/// The suites, of which `weblog` is the web log's queries of
/// `bench/analytics/weblog.rs`.
const SUITES: [Suite; 1] = [Suite { name: "weblog", bench: "analytics", chosen_by: ("TURBOREL_BENCH_ANALYTICS_ONLY", "weblog") }];

/// The options of the suites, each with the variable its value is handed
/// in. They set the rows made, the queries run, the threads a query runs
/// on, the query run in a loop for a profiler and for how many seconds,
/// the queries whose plans are printed, and the directory the table is
/// kept in from one run to the next.
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

/// The option that names the processors a suite's run is kept to, as
/// `taskset` lists them, so other work on the machine's other processors
/// disturbs its times less.
pub const CPUS: &str = "--cpus";
/// The program that keeps a run to processors, and its option for them.
const PINNED_BY: &str = "taskset -c";

/// The variable through which cargo runs the benchmark's program by
/// another, for the target `host`, and the program that keeps it to the
/// processors `cpus`.
pub fn pinned(host: &str, cpus: &str) -> (String, String) {
    let target = host.to_uppercase().replace('-', "_");
    (format!("CARGO_TARGET_{target}_RUNNER"), format!("{PINNED_BY} {cpus}"))
}

/// The suites' options, as the usage shows them.
pub fn usage() -> String {
    SETTINGS.iter().fold(format!(" [{CPUS} LIST]"), |usage, (name, _)| usage + " [" + name + " VALUE]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-BENCH-1 a suite is found by its name and names
    /// the benchmark it is part of; a benchmark's own name is no suite; a
    /// suite's option names its variable, and another option none; a run
    /// is kept to processors by the runner of the host's target.
    #[test]
    fn suites_and_their_options_are_found() {
        assert_eq!(named("weblog").map(|suite| suite.bench), Some("analytics"));
        assert_eq!(named("analytics"), None);
        assert_eq!(setting("--only"), Some("TURBOREL_BENCH_WEBLOG_ONLY"));
        assert_eq!(setting("--save"), None);
        assert!(usage().contains("[--profile VALUE]") && usage().contains("[--cpus LIST]"));
        let (variable, runner) = pinned("x86_64-unknown-linux-gnu", "4-7,12-15");
        assert_eq!((variable.as_str(), runner.as_str()), ("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER", "taskset -c 4-7,12-15"));
    }
}
