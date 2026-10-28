//! Running cargo and other tools as child processes.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// No extra environment variables.
pub const NO_ENV: &[(&str, &str)] = &[];

/// The workspace root: the directory above this crate.
pub fn workspace_root() -> PathBuf {
    let xtask_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir.parent().unwrap_or(xtask_dir).to_path_buf()
}

// Cargo's environment variables.
/// Set by cargo for the programs it runs: the cargo executable itself.
const CARGO_VAR: &str = "CARGO";
/// Where a build puts its output.
pub const TARGET_DIR_VAR: &str = "CARGO_TARGET_DIR";
/// Flags for every rustc a build runs.
pub const RUSTFLAGS_VAR: &str = "RUSTFLAGS";
/// Flags for every rustdoc a build runs.
pub const RUSTDOCFLAGS_VAR: &str = "RUSTDOCFLAGS";
/// The cargo on the path, when this tool was not started by cargo.
const CARGO_ON_PATH: &str = "cargo";

/// The cargo that started this tool, so nested builds use the same toolchain.
/// Read as an `OsString`, so a path that is not UTF-8 is still used.
fn cargo_program() -> OsString {
    env::var_os(CARGO_VAR).unwrap_or_else(|| CARGO_ON_PATH.into())
}

/// Runs cargo in `root`, sharing the terminal; fails if cargo fails.
pub fn cargo<K: AsRef<OsStr>, V: AsRef<OsStr>>(root: &Path, args: &[&str], envs: &[(K, V)]) -> Result<()> {
    let mut command = Command::new(cargo_program());
    command.args(args).current_dir(root);
    for (key, value) in envs {
        command.env(key, value);
    }
    let status = command.status().map_err(|e| Error::new(format!("cannot run cargo: {e}")))?;
    if status.success() { Ok(()) } else { Err(Error::new(format!("`cargo {}` failed ({status})", args.join(" ")))) }
}

/// Runs cargo in `root` and returns its standard output; fails if cargo fails.
pub fn cargo_output<K: AsRef<OsStr>, V: AsRef<OsStr>>(root: &Path, args: &[&str], envs: &[(K, V)]) -> Result<String> {
    let mut command = Command::new(cargo_program());
    command.args(args).current_dir(root);
    for (key, value) in envs {
        command.env(key, value);
    }
    output(&mut command, CARGO_ON_PATH)
}

/// Runs `program` in `root` and returns its standard output; fails if it fails.
pub fn tool_output<S: AsRef<OsStr>>(root: &Path, program: &Path, args: &[S]) -> Result<String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    output(&mut command, &program.display().to_string())
}

fn output(command: &mut Command, name: &str) -> Result<String> {
    let out = command.stderr(Stdio::inherit()).output().map_err(|e| Error::new(format!("cannot run {name}: {e}")))?;
    if !out.status.success() {
        return Err(Error::new(format!("{name} failed ({})", out.status)));
    }
    String::from_utf8(out.stdout).map_err(|_| Error::new(format!("{name} printed text that is not UTF-8")))
}
