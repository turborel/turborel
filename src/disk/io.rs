//! \[DESIGN\] File I/O for the storage foundation (RULES.md D30).
//!
//! Every read and write names its byte offset (positional I/O), so threads
//! share a file without sharing a cursor. How durable a write must be is a
//! `Durability`, chosen by the caller. Files are never memory-mapped: the
//! operating system would then decide when pages reach the disk, and the
//! components above must decide that (write-ahead logging, D25).
//!
//! The file system is an extension point (`FileSystem`, `DiskFile`), so a
//! component can be given another one: a test's, which injects torn writes,
//! full disks and I/O errors, or a platform's own. Built in: `OsFileSystem`,
//! over `std::fs`, on Unix and Windows, the platforms whose std offers
//! positional I/O. Only one platform primitive differs between them, the
//! single positional read or write; the loops around it are shared.
//!
//! On Unix, the files and directories it makes are its user's alone
//! (RULES.md D56): files 0600, directories 0700, less what the process's
//! umask takes away. On Windows they take what their directory gives.

use std::fs::{self, File, OpenOptions};
use std::io::{self as stdio, ErrorKind};
use std::path::{Path, PathBuf};

use crate::error::{Code, Result, SqlError};

/// How durable a write must be before `DiskFile::sync` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Not synced: the data reaches the disk when the operating system
    /// chooses, and a crash may lose it.
    None,
    /// The data, and the metadata needed to read it back (such as the
    /// file's length), are on the device: `fdatasync` on Linux.
    Data,
    /// The data and all metadata are on the device: `fsync`, or
    /// `FlushFileBuffers` on Windows. On Apple platforms both `Data` and
    /// `Full` use `F_FULLFSYNC`, which also flushes the drive's own cache.
    Full,
}

/// How to open a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Create it for reading and writing; it must not exist.
    CreateNew,
    /// Open it for reading and writing; it must exist.
    ReadWrite,
    /// Open it for reading only; it must exist.
    ReadOnly,
}

/// \[DESIGN\] A file system: an extension point (RULES.md D30).
///
/// Contract (RULES.md R-SAFE-3; checked for `OsFileSystem` by
/// `test/integration/disk.rs`):
/// * `open` honours `OpenMode`: `CreateNew` fails if the file exists, the
///   others if it does not.
/// * A file created or removed survives a crash only after `sync_dir` of
///   its directory returns.
/// * `lock` gives a lock no other holder of the file's lock has, in this
///   process or another, until its `FileLock` is dropped; a holder that
///   ends without dropping it, as a process that crashed, gives it up.
/// * Every failure is an error (SQLCODE -915), never a panic.
/// * Methods may be called from several threads at once (`Send + Sync`).
pub trait FileSystem: Send + Sync {
    /// Opens the file at `path`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be opened as `mode` says.
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>>;
    /// Removes the file at `path`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it does not exist or cannot be removed.
    fn remove(&self, path: &Path) -> Result<()>;
    /// Whether a file or directory exists at `path`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if that cannot be found out.
    fn exists(&self, path: &Path) -> Result<bool>;
    /// Removes the file at `path`, which nothing needs, as a spill file
    /// dropped: a failure is no caller's to handle, so it is told where the
    /// file system tells (the file manager's: the database's sink), or not
    /// at all. Default: removed, a failure dropped.
    fn discard(&self, path: &Path) {
        let _ = self.remove(path);
    }
    /// Creates the directory `path`, and any missing parents.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be created.
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    /// Creates the directory `path`, and any missing parents, as
    /// `create_dir_all` does, then syncs each it made in its parent, so a
    /// crash after leaves them (RULES.md D56).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if one cannot be created or synced.
    fn create_dir_durably(&self, path: &Path) -> Result<()> {
        let mut made = Vec::new();
        for dir in path.ancestors().filter(|d| !d.as_os_str().is_empty()) {
            if self.exists(dir)? {
                break;
            }
            made.push(dir);
        }
        self.create_dir_all(path)?;
        for dir in made {
            let parent = dir.parent().map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p });
            if let Some(parent) = parent {
                self.sync_dir(parent)?;
            }
        }
        Ok(())
    }
    /// The paths of the entries of directory `path`, sorted.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be read.
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    /// Makes the creation and removal of entries in directory `path`
    /// durable.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be synced.
    fn sync_dir(&self, path: &Path) -> Result<()>;
    /// \[DESIGN\] An exclusive lock on the file at `path`, made if it is
    /// not there, held until the lock is dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if another holds it, or it cannot be taken.
    fn lock(&self, path: &Path) -> Result<FileLock>;
}

/// \[DESIGN\] A lock a file system gave (`FileSystem::lock`), given up when
/// dropped.
pub struct FileLock(#[expect(dead_code, reason = "held for its drop, which gives the lock up")] Box<dyn Send + Sync>);

impl FileLock {
    /// The lock that `held`, dropped, gives up.
    pub fn new(held: impl Send + Sync + 'static) -> FileLock {
        FileLock(Box::new(held))
    }
}

impl std::fmt::Debug for FileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileLock")
    }
}

/// The error of a lock another holds.
pub fn locked(path: &Path) -> SqlError {
    SqlError::new(Code::Io, format!("[DESIGN] {} is locked: another process, or another database of this one, holds it", path.display()))
}

/// \[DESIGN\] An open file, read and written at byte offsets.
///
/// Contract (RULES.md R-SAFE-3, as for `FileSystem`):
/// * `write_all_at` past the end extends the file; bytes never written read
///   as zero.
/// * After `sync` with `Durability::Data` or `Full` returns, what was
///   written before it survives a crash.
/// * Every failure is an error (SQLCODE -915), never a panic.
/// * Methods may be called from several threads at once (`Send + Sync`);
///   reads and writes of disjoint ranges do not affect each other.
pub trait DiskFile: Send + Sync {
    /// Reads into `buf` from `offset` until it is full or the file ends, and
    /// returns how many bytes were read: fewer than `buf.len()` only at the
    /// end of the file.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the read fails.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize>;
    /// Writes all of `data` at `offset`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the write fails, for example because the disk is full.
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()>;
    /// The file's size in bytes.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be found out.
    fn size(&self) -> Result<u64>;
    /// Truncates or extends the file to `size` bytes; new bytes read as zero.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the size cannot be set.
    fn set_size(&self, size: u64) -> Result<()>;
    /// Makes what was written durable, as `durability` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the sync fails; what was written may then be lost.
    fn sync(&self, durability: Durability) -> Result<()>;
    /// Holds room for the file to grow to `size` bytes, where its bytes are
    /// kept in memory of a budget (RULES.md D56), so a write within it is
    /// not refused; a file on disk holds nothing. Default: nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget has no room for it.
    fn reserve(&self, size: u64) -> Result<()> {
        let _ = size;
        Ok(())
    }
    /// Fills `buf` from `offset`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the read fails or the file ends first.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        let read = self.read_at(buf, offset)?;
        if read < buf.len() {
            return Err(SqlError::new(
                Code::Io,
                format!("[DESIGN] the file ends {read} bytes after offset {offset}, before the {} bytes asked for", buf.len()),
            ));
        }
        Ok(())
    }
}

/// \[DESIGN\] A new file at `path` of `fs`, beginning with `header`, and,
/// if `durable`, synced with its directory, so a crash after leaves it
/// whole. One that fails once made is removed: it holds nothing, and may
/// then be made again (RULES.md D56).
///
/// # Errors
///
/// SQLCODE -915 if it cannot be made, written or synced.
pub(crate) fn begin_file(fs: &dyn FileSystem, path: &Path, header: &[u8], durable: bool) -> Result<Box<dyn DiskFile>> {
    let file = fs.open(path, OpenMode::CreateNew)?;
    let mut begun = file.write_all_at(header, 0);
    if durable {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        begun = begun.and_then(|()| file.sync(Durability::Full)).and_then(|()| dir.map_or(Ok(()), |d| fs.sync_dir(d)));
    }
    if let Err(e) = begun {
        drop(file);
        // A file not removed is told, and refused as half made where it is
        // opened, or cleared with its directory's scratch.
        fs.discard(path);
        return Err(e);
    }
    Ok(file)
}

/// The error for `what` failing on `path`.
fn failed(what: &str, path: &Path, e: &stdio::Error) -> SqlError {
    SqlError::new(Code::Io, format!("[DESIGN] could not {what} {}: {e}", path.display()))
}

/// The offset `done` bytes past `offset`; every byte a read or write touches
/// must have one.
fn advance(offset: u64, done: usize, path: &Path) -> Result<u64> {
    u64::try_from(done)
        .ok()
        .and_then(|d| offset.checked_add(d))
        .ok_or_else(|| SqlError::new(Code::Io, format!("[DESIGN] an offset in {} is beyond 2^64 bytes", path.display())))
}

/// \[DESIGN\] The operating system's file system, through `std::fs`.
#[cfg(any(unix, windows))]
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFileSystem;

#[cfg(any(unix, windows))]
impl FileSystem for OsFileSystem {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let mut options = owners();
        match mode {
            OpenMode::CreateNew => options.read(true).write(true).create_new(true),
            OpenMode::ReadWrite => options.read(true).write(true),
            OpenMode::ReadOnly => options.read(true),
        };
        let file = options.open(path).map_err(|e| failed("open", path, &e))?;
        Ok(Box::new(OsFile { file, path: path.to_path_buf() }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).map_err(|e| failed("remove", path, &e))
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        path.try_exists().map_err(|e| failed("look for", path, &e))
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        owners_directories().create(path).map_err(|e| failed("create the directory", path, &e))
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let read = |e: stdio::Error| failed("read the directory", path, &e);
        let mut paths = fs::read_dir(path).map_err(read)?.map(|entry| entry.map(|e| e.path()).map_err(read)).collect::<Result<Vec<_>>>()?;
        paths.sort();
        Ok(paths)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        sync_directory(path)
    }
    /// \[DESIGN\] `File::try_lock`: `flock` on Unix, `LockFileEx` on
    /// Windows; the system gives it up when the process ends.
    fn lock(&self, path: &Path) -> Result<FileLock> {
        let file = owners().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| failed("open", path, &e))?;
        match file.try_lock() {
            Ok(()) => Ok(FileLock::new(file)),
            Err(fs::TryLockError::WouldBlock) => Err(locked(path)),
            Err(fs::TryLockError::Error(e)) => Err(failed("lock", path, &e)),
        }
    }
}

/// Options that make a file its user's alone: 0600.
#[cfg(unix)]
fn owners() -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}

/// On Windows a file takes what its directory gives.
#[cfg(windows)]
fn owners() -> OpenOptions {
    OpenOptions::new()
}

/// A maker of directories, and of those they are in, each its user's
/// alone: 0700. A directory that exists is left as it is.
#[cfg(unix)]
fn owners_directories() -> fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
}

/// On Windows a directory takes what its parent gives.
#[cfg(windows)]
fn owners_directories() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    builder
}

/// On Unix a directory is synced like a file.
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path).and_then(|dir| dir.sync_all()).map_err(|e| failed("sync the directory", path, &e))
}

/// On Windows std cannot open a directory to sync it; NTFS journals the
/// metadata of creating and removing files instead. The directory must exist.
#[cfg(windows)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::metadata(path).map(|_| ()).map_err(|e| failed("sync the directory", path, &e))
}

/// A file of `OsFileSystem`, and its path, for messages.
#[cfg(any(unix, windows))]
struct OsFile {
    file: File,
    path: PathBuf,
}

/// One positional read: the platform primitive.
#[cfg(unix)]
fn read_once(file: &File, buf: &mut [u8], offset: u64) -> stdio::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

/// One positional write: the platform primitive.
#[cfg(unix)]
fn write_once(file: &File, data: &[u8], offset: u64) -> stdio::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, data, offset)
}

/// One positional read. It moves the file's cursor too, which no code here
/// uses.
#[cfg(windows)]
fn read_once(file: &File, buf: &mut [u8], offset: u64) -> stdio::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

/// One positional write, as `read_once`.
#[cfg(windows)]
fn write_once(file: &File, data: &[u8], offset: u64) -> stdio::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, data, offset)
}

/// Runs `op` until the operating system does not interrupt it.
#[cfg(any(unix, windows))]
fn retry<T>(mut op: impl FnMut() -> stdio::Result<T>) -> stdio::Result<T> {
    loop {
        match op() {
            Ok(done) => return Ok(done),
            Err(e) => {
                if e.kind() != ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(any(unix, windows))]
impl DiskFile for OsFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        advance(offset, buf.len(), &self.path)?;
        let mut done = 0;
        while let Some(rest) = buf.get_mut(done..).filter(|rest| !rest.is_empty()) {
            let at = advance(offset, done, &self.path)?;
            let read = retry(|| read_once(&self.file, rest, at)).map_err(|e| failed("read", &self.path, &e))?;
            if read == 0 {
                break;
            }
            done = done.saturating_add(read);
        }
        Ok(done)
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        // Refused before anything is written, so a write past 2^64 bytes changes nothing.
        advance(offset, data.len(), &self.path)?;
        let mut done = 0;
        while let Some(rest) = data.get(done..).filter(|rest| !rest.is_empty()) {
            let at = advance(offset, done, &self.path)?;
            let written = retry(|| write_once(&self.file, rest, at)).map_err(|e| failed("write", &self.path, &e))?;
            if written == 0 {
                return Err(failed("write", &self.path, &stdio::Error::from(ErrorKind::WriteZero)));
            }
            done = done.saturating_add(written);
        }
        Ok(())
    }
    fn size(&self) -> Result<u64> {
        self.file.metadata().map(|m| m.len()).map_err(|e| failed("find the size of", &self.path, &e))
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.file.set_len(size).map_err(|e| failed("set the size of", &self.path, &e))
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        let synced = match durability {
            Durability::None => Ok(()),
            Durability::Data => self.file.sync_data(),
            Durability::Full => self.file.sync_all(),
        };
        synced.map_err(|e| failed("sync", &self.path, &e))
    }
}
