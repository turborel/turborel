//! Finding the project's Rust source files.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// The library's sources, relative to the workspace root.
pub const LIBRARY_DIR: &str = "src";
/// The wire protocol's sources (RULES.md D42).
pub const PROTOCOL_DIR: &str = "protocol/src";
/// The client's sources (RULES.md D42).
pub const CLIENT_DIR: &str = "client/src";
/// The server's sources (RULES.md D42).
pub const SERVER_DIR: &str = "server/src";
/// The server program's sources (RULES.md D42).
pub const PROGRAM_DIR: &str = "turboreld/src";
/// The command-line client's sources (RULES.md D46).
pub const CLI_DIR: &str = "cli/src";
/// The LZ4 plug-in's sources (RULES.md D48).
pub const LZ4_DIR: &str = "lz4/src";
/// The AES-256-GCM plug-in's sources (RULES.md D26, D48).
pub const AES_DIR: &str = "aes/src";
/// Replication's sources (RULES.md D57).
pub const REPLICATION_DIR: &str = "replication/src";
/// Directories, relative to the workspace root, that hold the project's Rust code.
pub const SOURCE_DIRS: &[&str] = &[
    LIBRARY_DIR,
    "test",
    "bench",
    PROTOCOL_DIR,
    CLIENT_DIR,
    SERVER_DIR,
    "server/test",
    PROGRAM_DIR,
    "turboreld/test",
    CLI_DIR,
    "cli/test",
    LZ4_DIR,
    AES_DIR,
    REPLICATION_DIR,
    "replication/test",
    "support/src",
    "xtask/src",
];

/// The extension of a Rust source file.
pub const RUST_EXTENSION: &str = "rs";
/// Deepest directory nesting searched below a source directory (R-CF-6). The
/// project's deepest is 4 (`src/exec/batch/vector/ints/`); past this, the walk
/// fails rather than recursing without bound.
const MAX_DIR_DEPTH: usize = 8;

/// Every `.rs` file under `SOURCE_DIRS`, as sorted `/`-separated paths
/// relative to `root`, so results are the same on every platform.
pub fn rust_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for dir in SOURCE_DIRS {
        collect(root, &root.join(dir), 0, &mut files)?;
    }
    files.sort();
    Ok(files)
}

/// Adds the `.rs` files under `dir`, `depth` levels below a source directory.
/// Symbolic links are not followed: a link to a parent would list files twice,
/// and a loop of links would never end.
fn collect(root: &Path, dir: &Path, depth: usize, files: &mut Vec<String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    if depth > MAX_DIR_DEPTH {
        return Err(Error::new(format!("{} is nested deeper than {MAX_DIR_DEPTH} directories", dir.display())));
    }
    let listing_error = |e: std::io::Error| Error::new(format!("cannot list {}: {e}", dir.display()));
    for entry in fs::read_dir(dir).map_err(listing_error)? {
        let entry = entry.map_err(listing_error)?;
        let kind = entry.file_type().map_err(listing_error)?;
        let path = entry.path();
        if kind.is_dir() {
            collect(root, &path, depth.saturating_add(1), files)?;
        } else if kind.is_file() && path.extension().is_some_and(|e| e == RUST_EXTENSION) {
            files.push(relative(root, &path));
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` between components.
fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The contents of `file`, relative to `root`.
pub fn read(root: &Path, file: &str) -> Result<String> {
    fs::read_to_string(root.join(file)).map_err(|e| Error::new(format!("cannot read {file}: {e}")))
}
