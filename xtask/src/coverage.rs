//! RULES.md R-COV: structural coverage of the product's crates (the
//! library, the wire protocol, the client, the server and its program, the
//! command-line client, and the LZ4 and AES-256-GCM plug-ins) by their
//! tests, measured with rustc's
//! source-based coverage (`-C instrument-coverage`) and the LLVM tools of the
//! pinned toolchain.
//!
//! What is measured is the code under each crate's `src`, named to
//! `llvm-cov` as the source directories, so the location of the checkout
//! does not matter; the test support crate is not the product, and is not
//! measured. The programs, `turboreld` and `turborel`, run by their tests,
//! write profiles too, as the test processes do.
//! Two limits of the stable toolchain apply. Doc tests are not run under
//! coverage, because collecting their profiles needs a nightly rustdoc flag.
//! Unit-test modules inside `src` count as covered code, because excluding them
//! needs the unstable `coverage` attribute. Branch coverage needs a nightly
//! compiler, and stable rustc has no MC/DC; see RULES.md D22.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::baseline::{COVERAGE_FLOOR_LINES, COVERAGE_FLOOR_REGIONS};
use crate::command::{self, RUSTFLAGS_VAR, TARGET_DIR_VAR};
use crate::error::{Error, Result};
use crate::files;

/// The packages whose coverage is measured.
pub const PACKAGES: [&str; 8] =
    ["turborel", "turborel-protocol", "turborel-client", "turborel-server", "turboreld", "turborel-cli", "turborel-lz4", "turborel-aes"];
/// The measured sources, relative to the workspace root.
const MEASURED_DIRS: [&str; 8] = [
    files::LIBRARY_DIR,
    files::PROTOCOL_DIR,
    files::CLIENT_DIR,
    files::SERVER_DIR,
    files::PROGRAM_DIR,
    files::CLI_DIR,
    files::LZ4_DIR,
    files::AES_DIR,
];
/// Everything coverage writes, relative to the workspace root.
const OUT_DIR: &str = "target/coverage";
const BUILD_DIR: &str = "build";
const PROFILE_DIR: &str = "profiles";
const HTML_DIR: &str = "html";
const HTML_INDEX: &str = "index.html";
const MERGED_PROFILE: &str = "merged.profdata";
const PROFILE_EXTENSION: &str = "profraw";
/// Environment of the instrumented build and test run.
const PROFILE_FILE_VAR: &str = "LLVM_PROFILE_FILE";
const INSTRUMENT_FLAGS: &str = "-C instrument-coverage";
/// The toolchain and its LLVM tools (component `llvm-tools`).
const RUSTC: &str = "rustc";
const HOST_PREFIX: &str = "host: ";
const LLVM_PROFDATA: &str = "llvm-profdata";
const LLVM_COV: &str = "llvm-cov";
/// Where the LLVM tools are, below the sysroot: `lib/rustlib/<host>/bin`.
const SYSROOT_LIB: &str = "lib";
const RUSTLIB: &str = "rustlib";
const TOOLS_BIN: &str = "bin";
/// The summary row of `llvm-cov report`, and its columns counted from 0.
const TOTAL_LABEL: &str = "TOTAL";
const REGION_COVER_COLUMN: usize = 3;
const LINE_COVER_COLUMN: usize = 9;
/// Where cargo's JSON messages name a built executable.
const EXECUTABLE_KEY: &str = "\"executable\":\"";

/// Builds the measured crates with coverage, runs their tests, writes an
/// HTML report and fails if coverage is below the floors in `baseline`.
pub fn run(root: &Path) -> Result<()> {
    let tools = LlvmTools::find(root)?;
    let out = root.join(OUT_DIR);
    let profiles = out.join(PROFILE_DIR);
    reset_dir(&profiles)?;
    // %p and %m (process id, binary signature) give each test process its own file.
    let envs = [
        (TARGET_DIR_VAR, out.join(BUILD_DIR).into_os_string()),
        (RUSTFLAGS_VAR, INSTRUMENT_FLAGS.into()),
        (PROFILE_FILE_VAR, profiles.join(format!("%p-%m.{PROFILE_EXTENSION}")).into_os_string()),
    ];
    command::cargo(root, &test_args(&["--quiet"]), &envs)?;
    let json = command::cargo_output(root, &test_args(&["--no-run", "--message-format=json"]), &envs)?;
    let merged = out.join(MERGED_PROFILE);
    tools.merge(root, &profile_files(&profiles)?, &merged)?;
    let report = tools.report(root, &merged, &executables(&json), &out.join(HTML_DIR))?;
    println!("{report}");
    check_floors(&report)
}

/// `cargo test` of each measured package's library and tests, with `more`.
pub fn test_args<'a>(more: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["test"];
    for package in PACKAGES {
        args.extend(["--package", package]);
    }
    args.extend(["--lib", "--bins", "--tests"]);
    args.extend(more);
    args
}

/// The two LLVM programs coverage needs, from the `llvm-tools` component.
struct LlvmTools {
    profdata: PathBuf,
    cov: PathBuf,
}

impl LlvmTools {
    fn find(root: &Path) -> Result<Self> {
        let rustc = Path::new(RUSTC);
        let sysroot = command::tool_output(root, rustc, &["--print", "sysroot"])?;
        let version = command::tool_output(root, rustc, &["-vV"])?;
        let host = version.lines().find_map(|l| l.strip_prefix(HOST_PREFIX)).ok_or_else(|| Error::new("rustc -vV names no host"))?;
        let bin = Path::new(sysroot.trim()).join(SYSROOT_LIB).join(RUSTLIB).join(host).join(TOOLS_BIN);
        let tool = |name: &str| bin.join(format!("{name}{}", env::consts::EXE_SUFFIX));
        let tools = LlvmTools { profdata: tool(LLVM_PROFDATA), cov: tool(LLVM_COV) };
        if tools.profdata.is_file() && tools.cov.is_file() {
            Ok(tools)
        } else {
            Err(Error::new("llvm-tools missing: rustup component add llvm-tools"))
        }
    }

    fn merge(&self, root: &Path, profiles: &[PathBuf], merged: &Path) -> Result<()> {
        let mut args = vec!["merge".into(), "-sparse".into(), "-o".into(), merged.as_os_str().to_owned()];
        args.extend(profiles.iter().map(|p| p.as_os_str().to_owned()));
        command::tool_output(root, &self.profdata, &args).map(drop)
    }

    /// Writes the HTML report and returns the text summary of the code under
    /// `MEASURED_DIRS`. `llvm-cov` takes the first executable as its positional
    /// binary, any others as `-object`, and then the sources to report on.
    fn report(&self, root: &Path, merged: &Path, objects: &[String], html: &Path) -> Result<String> {
        let (first, others) = objects.split_first().ok_or_else(|| Error::new("cargo built no executables"))?;
        let mut common = vec![format!("-instr-profile={}", merged.display()), first.clone()];
        for object in others {
            common.extend(["-object".to_owned(), object.clone()]);
        }
        common.extend(MEASURED_DIRS.iter().map(|dir| root.join(dir).display().to_string()));
        let show = [vec!["show".to_owned(), "-format=html".to_owned(), format!("-output-dir={}", html.display())], common.clone()].concat();
        command::tool_output(root, &self.cov, &show)?;
        println!("report: {}", html.join(HTML_INDEX).display());
        command::tool_output(root, &self.cov, &[vec!["report".to_owned()], common].concat())
    }
}

fn reset_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| Error::new(format!("cannot remove {}: {e}", dir.display())))?;
    }
    fs::create_dir_all(dir).map_err(|e| Error::new(format!("cannot create {}: {e}", dir.display())))
}

fn profile_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let listing_error = |e: std::io::Error| Error::new(format!("cannot list {}: {e}", dir.display()));
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(listing_error)? {
        let path = entry.map_err(listing_error)?.path();
        if path.extension().is_some_and(|e| e == PROFILE_EXTENSION) {
            files.push(path);
        }
    }
    if files.is_empty() { Err(Error::new("the tests wrote no coverage profiles")) } else { Ok(files) }
}

/// The test and program executables in cargo's JSON messages.
fn executables(json: &str) -> Vec<String> {
    json.lines().filter_map(|line| line.split_once(EXECUTABLE_KEY)).filter_map(|(_, rest)| json_string(rest)).collect()
}

/// The JSON string whose text starts `rest` (just after its opening quote),
/// unescaped; `None` if it does not end. Paths use the escapes `\\` and `\"`.
fn json_string(rest: &str) -> Option<String> {
    let mut text = String::new();
    let mut escaped = false;
    for c in rest.chars() {
        if escaped {
            text.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(text);
        } else {
            text.push(c);
        }
    }
    None
}

/// Region and line coverage, in percent, from the `TOTAL` row of a report.
fn totals(report: &str) -> Result<(f64, f64)> {
    let row: Vec<&str> =
        report.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|f| f.first() == Some(&TOTAL_LABEL)).unwrap_or_default();
    let percent = |column: usize| row.get(column).and_then(|f| f.strip_suffix('%')).and_then(|f| f.parse::<f64>().ok());
    match (percent(REGION_COVER_COLUMN), percent(LINE_COVER_COLUMN)) {
        (Some(regions), Some(lines)) => Ok((regions, lines)),
        _ => Err(Error::new("llvm-cov report has no readable TOTAL row")),
    }
}

/// Whether coverage fell below either floor (R-COV-2).
fn below_floor(regions: f64, lines: f64, floor_regions: f64, floor_lines: f64) -> bool {
    regions < floor_regions || lines < floor_lines
}

fn check_floors(report: &str) -> Result<()> {
    let (regions, lines) = totals(report)?;
    println!("coverage: regions {regions}% (floor {COVERAGE_FLOOR_REGIONS}%), lines {lines}% (floor {COVERAGE_FLOOR_LINES}%)");
    if below_floor(regions, lines, COVERAGE_FLOOR_REGIONS, COVERAGE_FLOOR_LINES) {
        return Err(Error::new("R-COV-2: coverage fell below its floor in xtask/src/baseline.rs"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "Filename Regions Missed Cover Functions Missed Executed Lines Missed Cover
TOTALS.rs 10 1 90.00% 2 0 100.00% 20 2 90.00%
TOTAL 40 10 75.00% 8 2 75.00% 80 4 95.00%
";

    /// Verifies: [TOOL] R-COV-2 the totals are read from the row labelled exactly
    /// TOTAL, by column; a file whose name starts with TOTAL is not that row.
    #[test]
    fn totals_reads_region_and_line_cover() {
        assert_eq!(totals(REPORT).ok(), Some((75.0_f64, 95.0_f64)));
        assert!(totals("no table").is_err());
    }

    /// Verifies: [TOOL] R-COV-2 coverage fails when either measure is below its floor.
    ///
    /// Decision `regions < floor || lines < floor`. MC/DC: (F F) passes;
    /// (T F) and (F T) each fail alone.
    #[test]
    fn either_measure_below_its_floor_fails() {
        assert!(!below_floor(92.0, 94.0, 91.0, 93.0)); // F F
        assert!(below_floor(90.0, 94.0, 91.0, 93.0)); // T F
        assert!(below_floor(92.0, 92.0, 91.0, 93.0)); // F T
    }

    /// Verifies: [TOOL] R-COV-1 every executable cargo built is measured, with
    /// JSON escapes in paths undone.
    #[test]
    fn executables_from_cargo_json() {
        let json = "{\"executable\":\"/t/a\",\"fresh\":true}\n{\"executable\":null}\n{\"executable\":\"C:\\\\t\\\\b.exe\"}\n{\"executable\":\"/t/q\\\"x\"}";
        assert_eq!(executables(json), vec!["/t/a".to_owned(), "C:\\t\\b.exe".to_owned(), "/t/q\"x".to_owned()]);
        assert_eq!(json_string("unterminated"), None);
    }
}
