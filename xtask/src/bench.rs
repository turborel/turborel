//! RULES.md R-BENCH-1: `cargo xtask bench` runs the benchmarks in `bench/`,
//! built optimized, keeps their results, and compares two runs. Those that
//! need the server and the client are examples of the server's crate on
//! the same harness (`SERVED`), as `network` is.
//!
//! Each benchmark appends its results to the file named by `OUT_VAR`
//! (`bench/harness.rs`), one line each: benchmark, operation, threads,
//! bytes, unit, then the median, lowest and highest rate per second, tab
//! separated. This tool starts the file with a header naming the toolchain
//! and the machine, since results compare only on the same ones. A run is
//! kept as `LAST`; `--save LABEL` keeps its results under `LABEL`,
//! replacing those of the benchmarks run and keeping the others, and
//! `--compare LABEL` shows each result's change from it, calling a change
//! real only when it is larger than the spread of the runs on either side,
//! and than `NOISE_FLOOR`. A benchmark may keep, beside a rate, a line of
//! what the work gave (`RESULT`); a comparison names the work whose result
//! is not the saved one's, and the comparison then fails. A suite of a
//! benchmark runs alone by its own name, with its own options
//! (`bench/suites.rs`).

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::thread;

use crate::command;
use crate::error::{Error, Result};

mod suites;

/// Where results are kept, relative to the workspace root.
const OUT_DIR: &str = "target/bench";
/// The results of the latest run.
const LAST: &str = "last";
/// The extension of a results file.
const EXTENSION: &str = "tsv";
/// The variable naming the results file, as `bench/harness.rs` reads it.
const OUT_VAR: &str = "TURBOREL_BENCH_OUT";
/// The options.
const SAVE: &str = "--save";
const COMPARE: &str = "--compare";
/// What begins the line of `rustc -vV` that names the host's target.
const HOST: &str = "host: ";
/// Marks a header line of a results file.
const HEADER: &str = "# ";
/// Per-operation latency metadata from the shared harness, kept with its benchmark.
const LATENCY: &str = "@latency\t";
/// What a measurement's work gave, kept with its benchmark: the benchmark,
/// the operation, the threads, then the result as the benchmark words it.
const RESULT: &str = "@result\t";
/// The smallest change called real: results move this much between runs
/// on one machine (a cooler or busier machine, other processes), more than
/// the spread within one run shows.
const NOISE_FLOOR: f64 = 0.05;

/// \[DESIGN\] The benchmarks that need the server and the client, which
/// the core crate's targets cannot depend on (RULES.md D3): examples of the
/// server's crate on the same harness, run optimized as a benchmark is.
const SERVED: [&str; 1] = ["network"];
/// The package they are examples of.
const SERVED_PACKAGE: &str = "turborel-server";

/// The arguments to cargo that run the served benchmark `name`.
fn served_args(name: &str) -> [&str; 7] {
    ["run", "--quiet", "--release", "-p", SERVED_PACKAGE, "--example", name]
}

/// The arguments to cargo that run the benchmark `name`.
fn bench_args(name: &str) -> Vec<&str> {
    if SERVED.contains(&name) { served_args(name).to_vec() } else { vec!["bench", "--quiet", "--bench", name] }
}

/// What to run, and what to do with the results.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options<'a> {
    /// One benchmark, or all.
    bench: Option<&'a str>,
    /// Keep the results under this label.
    save: Option<&'a str>,
    /// Compare the results with those kept under this label.
    compare: Option<&'a str>,
    /// A suite's options: the variable each is handed in, and its value.
    settings: Vec<(&'static str, &'a str)>,
    /// The processors a suite's run is kept to.
    cpus: Option<&'a str>,
}

/// Runs `cargo xtask bench` with the arguments after `bench`.
pub fn run(root: &Path, args: &[&str]) -> Result<()> {
    let options = options(args)?;
    let dir = root.join(OUT_DIR);
    fs::create_dir_all(&dir).map_err(|e| Error::new(format!("cannot create {}: {e}", dir.display())))?;
    let last = dir.join(results_file(LAST));
    write(&last, &header(root)?)?;
    let suite = options.bench.and_then(suites::named);
    if suite.is_none() && (!options.settings.is_empty() || options.cpus.is_some()) {
        return Err(usage());
    }
    let pinned = options.cpus.map(|cpus| host(root).map(|host| suites::pinned(&host, cpus))).transpose()?;
    let mut envs: Vec<(&str, &OsStr)> = vec![(OUT_VAR, last.as_os_str())];
    envs.extend(suite.map(|s| (s.chosen_by.0, OsStr::new(s.chosen_by.1))));
    envs.extend(options.settings.iter().map(|(variable, value)| (*variable, OsStr::new(value))));
    envs.extend(pinned.iter().map(|(variable, runner)| (variable.as_str(), OsStr::new(runner))));
    let runs: Vec<Vec<&str>> = match suite.map(|s| s.bench).or(options.bench) {
        Some(name) => vec![bench_args(name)],
        None => {
            std::iter::once(vec!["bench", "--quiet", "--benches"]).chain(SERVED.iter().map(|name| served_args(name).to_vec())).collect()
        }
    };
    for cargo_args in runs {
        command::cargo(root, &cargo_args, &envs)?;
    }
    if let Some(label) = options.save {
        let kept = dir.join(results_file(label));
        let before = if kept.exists() { read(&kept)? } else { String::new() };
        write(&kept, &merged(&before, &read(&last)?))?;
        println!("\nbench: results saved as {label}");
    }
    if let Some(label) = options.compare {
        let base = read(&dir.join(results_file(label)))?;
        let now = read(&last)?;
        println!("\n== compared with {label}");
        for line in comparison(&base, &now) {
            println!("{line}");
        }
        let unlike = differing(&base, &now).len();
        if unlike > 0 {
            return Err(Error::new(format!("bench: {unlike} results are not those saved as {label}, so the builds do not answer alike")));
        }
    }
    Ok(())
}

/// The options in `args`.
fn options<'a>(args: &[&'a str]) -> Result<Options<'a>> {
    let mut options = Options::default();
    let mut rest = args;
    while let Some((first, after)) = rest.split_first() {
        if let (Some(variable), [value, more @ ..]) = (suites::setting(first), after) {
            options.settings.push((variable, value));
            rest = more;
            continue;
        }
        rest = match (*first, after) {
            (SAVE, [label, more @ ..]) => {
                options.save = Some(label);
                more
            }
            (COMPARE, [label, more @ ..]) => {
                options.compare = Some(label);
                more
            }
            (suites::CPUS, [cpus, more @ ..]) => {
                options.cpus = Some(cpus);
                more
            }
            (name, more) if options.bench.is_none() && !name.starts_with('-') => {
                options.bench = Some(name);
                more
            }
            _ => return Err(usage()),
        };
    }
    Ok(options)
}

/// The error that shows how the tool is used.
fn usage() -> Error {
    let suites = suites::usage();
    Error::new(format!("usage: cargo xtask bench [NAME] [{SAVE} LABEL] [{COMPARE} LABEL]; of the suite weblog also{suites}"))
}

/// The target the toolchain builds for on this machine.
fn host(root: &Path) -> Result<String> {
    let told = command::tool_output(root, Path::new("rustc"), &["-vV"])?;
    let host = told.lines().find_map(|line| line.strip_prefix(HOST));
    host.map(str::to_owned).ok_or_else(|| Error::new("rustc -vV names no host".to_owned()))
}

/// The file name of the results kept as `label`.
fn results_file(label: &str) -> String {
    format!("{label}.{EXTENSION}")
}

/// The header of a results file: the toolchain and the machine.
fn header(root: &Path) -> Result<String> {
    let rustc = command::tool_output(root, Path::new("rustc"), &["-V"])?;
    let cores = thread::available_parallelism().map_or(0, std::num::NonZero::get);
    Ok(format!("{HEADER}{}\n{HEADER}{} {}, {cores} threads\n", rustc.trim(), std::env::consts::OS, std::env::consts::ARCH))
}

fn write(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))
}

/// One result of a results file.
#[derive(Debug, PartialEq)]
struct Row<'a> {
    /// The benchmark, operation, threads and bytes: what identifies it.
    key: [&'a str; 4],
    unit: &'a str,
    median: f64,
    low: f64,
    high: f64,
}

impl Row<'_> {
    /// Half the range of its runs, as a fraction of the median.
    fn noise(&self) -> f64 {
        (self.high - self.low) / 2.0_f64 / self.median
    }
}

/// The results in `text`; lines that are not results are skipped.
fn rows(text: &str) -> Vec<Row<'_>> {
    text.lines()
        .filter(|line| !line.starts_with(HEADER))
        .filter_map(|line| match line.split('\t').collect::<Vec<_>>().as_slice() {
            [bench, operation, threads, bytes, unit, median, low, high] => Some(Row {
                key: [bench, operation, threads, bytes],
                unit,
                median: median.parse().ok()?,
                low: low.parse().ok()?,
                high: high.parse().ok()?,
            }),
            _ => None,
        })
        .collect()
}

/// The results kept as a label, `before`, with those of the benchmarks run
/// now replaced by `now`'s: so saving after running one benchmark keeps the
/// others. The header is `now`'s.
fn merged(before: &str, now: &str) -> String {
    let run: Vec<&str> = rows(now).iter().map(|r| r.key[0]).collect();
    let header = now.lines().filter(|l| l.starts_with(HEADER));
    let kept = before.lines().filter(|l| benchmark_of(l).is_some_and(|name| !run.contains(&name)));
    let results = now.lines().filter(|l| !l.starts_with(HEADER));
    header.chain(kept).chain(results).flat_map(|l| [l, "\n"]).collect()
}

/// The benchmark owning a throughput row or latency metadata row.
fn benchmark_of(line: &str) -> Option<&str> {
    if line.starts_with(LATENCY) || line.starts_with(RESULT) {
        return line.split('\t').nth(1);
    }
    rows(line).first().map(|row| row.key[0])
}

/// Each result of `now` against the same one in `base`: the change in its
/// median, and whether it is beyond the noise of both.
fn comparison(base: &str, now: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for header in base.lines().chain(now.lines()).filter(|l| l.starts_with(HEADER)) {
        if !lines.iter().any(|l| l == header) {
            lines.push(header.to_owned());
        }
    }
    let base_rows = rows(base);
    for row in rows(now) {
        let [bench, operation, threads, bytes] = row.key;
        let threads = if threads == "1" { "1 thread".to_owned() } else { format!("{threads} threads") };
        let what = format!("{bench}: {operation} ({threads}{})", if bytes == "0" { String::new() } else { format!(", {bytes} B") });
        let Some(before) = base_rows.iter().find(|b| b.key == row.key) else {
            lines.push(format!("{what:<60} new: {:.0} {}/s", row.median, row.unit));
            continue;
        };
        let change = row.median / before.median - 1.0_f64;
        let verdict = if change.abs() <= row.noise().max(before.noise()).max(NOISE_FLOOR) {
            "within noise"
        } else if change > 0.0_f64 {
            "faster"
        } else {
            "slower"
        };
        lines.push(format!("{what:<60} {:>+7.1}%  {verdict}", change * 100.0_f64));
    }
    lines.extend(differing(base, now));
    lines
}

/// What each measurement's work gave, of the lines of `text` that keep
/// it: the benchmark, operation and threads, then the result.
fn results(text: &str) -> Vec<([&str; 3], &str)> {
    text.lines().filter_map(|line| line.strip_prefix(RESULT)).filter_map(result).collect()
}

/// The fields of a line that keeps a result, its mark taken off.
fn result(line: &str) -> Option<([&str; 3], &str)> {
    let mut fields = line.splitn(4, '\t');
    Some(([fields.next()?, fields.next()?, fields.next()?], fields.next()?))
}

/// A line for each measurement of `now` whose work gave another result
/// than the same one's in `base`: both results, as their benchmark words
/// them.
fn differing(base: &str, now: &str) -> Vec<String> {
    let before = results(base);
    let mut lines = Vec::new();
    for (key, result) in results(now) {
        let saved = before.iter().find(|(k, _)| *k == key).map(|(_, r)| *r);
        if let Some(saved) = saved.filter(|saved| *saved != result) {
            let [bench, operation, _] = key;
            lines.push(format!("{bench}: {operation} gave another result: {saved} before, {result} now"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "# rustc 1.0\n# os arch, 8 threads\npages\twrite\t1\t4096\tpages\t100.0\t95.0\t105.0\n\
                        pages\tread\t1\t4096\tpages\t200.0\t199.0\t201.0\n";

    /// Verifies: [TOOL] R-BENCH-1 the options name one benchmark, a label
    /// to save as and one to compare with, in any order; others are refused.
    #[test]
    fn options_are_read() {
        assert_eq!(options(&[]).ok(), Some(Options::default()));
        let all = options(&["pages", "--save", "a", "--compare", "b"]).ok();
        assert_eq!(all, Some(Options { bench: Some("pages"), save: Some("a"), compare: Some("b"), ..Options::default() }));
        let compared = Options { bench: Some("pages"), compare: Some("b"), ..Options::default() };
        assert_eq!(options(&["--compare", "b", "pages"]).ok(), Some(compared));
        assert!(options(&["--save"]).is_err());
        assert!(options(&["pages", "tree"]).is_err());
        assert!(options(&["--fast"]).is_err());
    }

    /// Verifies: [TOOL] R-BENCH-1 a benchmark of the core crate runs as
    /// its bench target; one that needs the server, as the server's example,
    /// optimized (RULES.md D236).
    #[test]
    fn served_benchmarks_run_as_the_servers_examples() {
        assert_eq!(bench_args("pages"), ["bench", "--quiet", "--bench", "pages"]);
        assert_eq!(bench_args("network"), ["run", "--quiet", "--release", "-p", "turborel-server", "--example", "network"]);
    }

    /// Verifies: [TOOL] R-BENCH-1 a results file is read line by line,
    /// headers and malformed lines skipped.
    #[test]
    fn results_are_read() {
        let text = format!("{BASE}garbage\npages\tx\t1\t0\tpages\tnot a number\t1\t1\n");
        let read = rows(&text);
        assert_eq!(read.len(), 2);
        assert_eq!(read.first().map(|r| (r.key, r.median)), Some((["pages", "write", "1", "4096"], 100.0_f64)));
    }

    /// Verifies: [TOOL] R-BENCH-1 saving the results of some benchmarks
    /// replaces those benchmarks' results under the label, keeps the
    /// others', and takes the new header.
    #[test]
    fn saving_replaces_only_the_benchmarks_run() {
        let now = "# rustc 2.0\ntree\tget\t1\t0\tentries\t5.0\t4.0\t6.0\npages\twrite\t1\t4096\tpages\t110.0\t100.0\t120.0\n";
        let pages_only = "# rustc 2.0\npages\twrite\t1\t4096\tpages\t110.0\t100.0\t120.0\n";
        let saved = merged(&merged("", now), pages_only);
        assert_eq!(saved.lines().filter(|l| l.starts_with(HEADER)).collect::<Vec<_>>(), ["# rustc 2.0"]);
        assert_eq!(rows(&saved).len(), 2, "{saved}");
        let replaced = merged(BASE, pages_only);
        assert!(!replaced.contains("read"), "pages was run again: its old results go");
        assert!(replaced.starts_with("# rustc 2.0\n"), "{replaced}");
    }

    /// Verifies: [TOOL] R-BENCH-1 a change is called faster or slower only
    /// beyond the noise of both runs and the noise floor (a 4% change with
    /// tight runs is not); a result not in the base is new; the headers of
    /// both are shown.
    #[test]
    fn changes_beyond_the_noise_are_called() {
        let now = "# rustc 2.0\n# os arch, 8 threads\npages\twrite\t1\t4096\tpages\t104.0\t103.5\t104.5\n\
                   pages\tread\t1\t4096\tpages\t150.0\t149.0\t151.0\npages\tscan\t4\t0\tpages\t9.0\t8.0\t10.0\n";
        let lines = comparison(BASE, now);
        assert_eq!(lines.iter().filter(|l| l.starts_with(HEADER)).count(), 3, "each header once: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("write") && l.contains("within noise")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("read") && l.contains("-25.0%") && l.contains("slower")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("scan (4 threads)") && l.contains("new")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("write (1 thread, 4096 B)")), "{lines:?}");
    }
    /// Verifies: [TOOL] R-BENCH-1: saving one benchmark replaces its
    /// latency samples' summary and preserves another benchmark's.
    /// Metadata is neither a throughput row nor a machine header.
    #[test]
    fn latency_metadata_follows_its_benchmark() {
        let before = "a\tx\t1\t0\top\t1\t1\t1\n@latency\ta\tx\t1\t5\t10\t20\t30\tus\nb\ty\t1\t0\top\t1\t1\t1\n@latency\tb\ty\t1\t5\t10\t20\t30\tus\n";
        let now = "a\tx\t1\t0\top\t2\t2\t2\n@latency\ta\tx\t1\t5\t5\t10\t15\tus\n";
        let kept = merged(before, now);
        assert_eq!(kept.matches("@latency\ta").count(), 1);
        assert!(kept.contains("@latency\tb\ty"));
        assert!(kept.contains("\t5\t5\t10\t15\tus"));
        assert_eq!(rows(&kept).len(), 2);
        assert!(comparison(before, now).iter().all(|line| !line.contains("@latency")));
    }

    /// Verifies: [TOOL] R-BENCH-1 a suite's options are read with their
    /// values, each to the variable it is handed in; a comparison names
    /// the work whose result is not the saved one's, and no other; saving
    /// keeps the results of the benchmarks not run.
    #[test]
    fn a_suites_options_and_results_are_read() {
        let read = options(&["weblog", "--only", "q02,q29", "--save", "a", "--threads", "8", "--cpus", "4-7"]).ok();
        let settings = vec![("TURBOREL_BENCH_WEBLOG_ONLY", "q02,q29"), ("TURBOREL_BENCH_WEBLOG_THREADS", "8")];
        assert_eq!(read, Some(Options { bench: Some("weblog"), save: Some("a"), compare: None, settings, cpus: Some("4-7") }));
        assert!(options(&["weblog", "--only"]).is_err());
        let before = "w\tq01\t8\t0\tqueries\t1\t1\t1\n@result\tw\tq01\t8\t1 rows, checksum 0a\n@result\tw\tq02\t8\t1 rows, checksum 0b\n";
        let now = "w\tq01\t8\t0\tqueries\t1\t1\t1\n@result\tw\tq01\t8\t1 rows, checksum 0a\n@result\tw\tq02\t8\t2 rows, checksum 0c\n";
        let lines = comparison(before, now);
        assert_eq!(lines.iter().filter(|l| l.contains("another result")).count(), 1, "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("w: q02") && l.contains("1 rows, checksum 0b before, 2 rows, checksum 0c now")),
            "{lines:?}"
        );
        let kept = merged("@result\tother\tx\t1\t3 rows\nother\tx\t1\t0\tqueries\t1\t1\t1\n", now);
        assert!(kept.contains("@result\tother") && kept.contains("@result\tw\tq02\t8\t2 rows"), "{kept}");
    }
}
