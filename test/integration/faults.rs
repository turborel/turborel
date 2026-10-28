//! A file system that injects faults (RULES.md D30): the real file system,
//! with operations that fail, writes that are torn, or writes that are
//! slow, when a rule says so.
//! For the tests of the storage foundation's failure paths: what each
//! component does when an open, read, write, sync or removal fails.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use turborel::disk::io::{DiskFile, Durability, FileSystem, OpenMode, OsFileSystem};
use turborel::error::{Code, Result, SqlError};

/// The operations a rule can match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Open,
    Read,
    Write,
    Size,
    SetSize,
    Sync,
    Remove,
    ReadDir,
    SyncDir,
}

/// What a matched operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// It fails, doing nothing.
    Fail,
    /// A write writes the first half of its bytes, then fails.
    Tear,
    /// A write waits this long, then succeeds: to hold a thread inside one
    /// while another runs.
    Delay(Duration),
}

/// When an operation is made to fail.
#[derive(Debug, Clone)]
pub struct Rule {
    op: Op,
    /// Only on paths containing this.
    path: Option<String>,
    /// Only reads and writes at or past this byte offset, and before `below`.
    from_offset: u64,
    below: u64,
    /// Matching operations let through before the first fault.
    skip: usize,
    /// Faults to inject; `None` for every match from then on.
    times: Option<usize>,
    effect: Effect,
}

impl Rule {
    /// Every `op` fails.
    pub fn fail(op: Op) -> Rule {
        Rule { op, path: None, from_offset: 0, below: u64::MAX, skip: 0, times: None, effect: Effect::Fail }
    }

    /// Every write is torn.
    pub fn tear() -> Rule {
        Rule { effect: Effect::Tear, ..Rule::fail(Op::Write) }
    }

    /// Every write waits `pause`, then succeeds.
    pub fn delay(pause: Duration) -> Rule {
        Rule { effect: Effect::Delay(pause), ..Rule::fail(Op::Write) }
    }

    /// Only on paths containing `part`, its directories parted by `/` on
    /// every platform.
    pub fn on(mut self, part: &str) -> Rule {
        self.path = Some(part.to_owned());
        self
    }

    /// Only reads and writes at or past byte `offset`.
    pub fn at_or_past(mut self, offset: u64) -> Rule {
        self.from_offset = offset;
        self
    }

    /// Only reads and writes before byte `offset`.
    pub fn below(mut self, offset: u64) -> Rule {
        self.below = offset;
        self
    }

    /// After letting `n` matching operations through.
    pub fn after(mut self, n: usize) -> Rule {
        self.skip = n;
        self
    }

    /// `n` times only.
    pub fn times(mut self, n: usize) -> Rule {
        self.times = Some(n);
        self
    }
}

/// The rules in force, and the faults injected so far.
#[derive(Default)]
pub struct Faults {
    rules: Mutex<Vec<Rule>>,
    injected: AtomicUsize,
}

impl Faults {
    pub fn new() -> Arc<Faults> {
        Arc::new(Faults::default())
    }

    /// Adds `rule`.
    pub fn inject(&self, rule: Rule) {
        self.rules.lock().unwrap().push(rule);
    }

    /// Removes every rule.
    pub fn clear(&self) {
        self.rules.lock().unwrap().clear();
    }

    /// Faults injected so far.
    pub fn injected(&self) -> usize {
        self.injected.load(Ordering::SeqCst)
    }

    /// The effect for `op` on `path` at `offset`, if a rule fires: the rule
    /// counts the match, and is used up when its faults are.
    fn fires(&self, op: Op, path: &Path, offset: u64) -> Option<Effect> {
        let mut rules = self.rules.lock().unwrap();
        // Windows parts directories by `\`: a rule's `/` matches either.
        let shown = path.to_string_lossy().replace('\\', "/");
        let index = rules.iter().position(|r| {
            r.op == op && (r.from_offset..r.below).contains(&offset) && r.path.as_deref().is_none_or(|p| shown.contains(p))
        })?;
        let rule = &mut rules[index];
        if rule.skip > 0 {
            rule.skip -= 1;
            return None;
        }
        let effect = rule.effect;
        match &mut rule.times {
            Some(1) => {
                rules.remove(index);
            }
            Some(n) => *n -= 1,
            None => {}
        }
        self.injected.fetch_add(1, Ordering::SeqCst);
        Some(effect)
    }
}

/// The error a fault returns.
fn injected(op: Op, path: &Path) -> SqlError {
    SqlError::new(Code::Io, format!("injected: {op:?} of {} failed", path.display()))
}

/// The real file system, with `faults`.
pub struct FaultyFs {
    pub faults: Arc<Faults>,
}

impl FaultyFs {
    /// A faulty file system and its faults, to inject into.
    pub fn with_faults() -> (Arc<dyn FileSystem>, Arc<Faults>) {
        let faults = Faults::new();
        (Arc::new(FaultyFs { faults: Arc::clone(&faults) }), faults)
    }

    fn check(&self, op: Op, path: &Path) -> Result<()> {
        match self.faults.fires(op, path, 0) {
            Some(_) => Err(injected(op, path)),
            None => Ok(()),
        }
    }
}

impl FileSystem for FaultyFs {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        self.check(Op::Open, path)?;
        Ok(Box::new(FaultyFile { inner: OsFileSystem.open(path, mode)?, path: path.to_path_buf(), faults: Arc::clone(&self.faults) }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.check(Op::Remove, path)?;
        OsFileSystem.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        OsFileSystem.exists(path)
    }
    fn lock(&self, path: &Path) -> Result<turborel::disk::io::FileLock> {
        OsFileSystem.lock(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        OsFileSystem.create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.check(Op::ReadDir, path)?;
        OsFileSystem.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.check(Op::SyncDir, path)?;
        OsFileSystem.sync_dir(path)
    }
}

/// A file of a `FaultyFs`.
struct FaultyFile {
    inner: Box<dyn DiskFile>,
    path: PathBuf,
    faults: Arc<Faults>,
}

impl FaultyFile {
    fn check(&self, op: Op) -> Result<()> {
        match self.faults.fires(op, &self.path, 0) {
            Some(_) => Err(injected(op, &self.path)),
            None => Ok(()),
        }
    }
}

impl DiskFile for FaultyFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        match self.faults.fires(Op::Read, &self.path, offset) {
            Some(_) => Err(injected(Op::Read, &self.path)),
            None => self.inner.read_at(buf, offset),
        }
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        match self.faults.fires(Op::Write, &self.path, offset) {
            Some(Effect::Tear) => {
                self.inner.write_all_at(&data[..data.len().div_euclid(2)], offset)?;
                Err(injected(Op::Write, &self.path))
            }
            Some(Effect::Fail) => Err(injected(Op::Write, &self.path)),
            Some(Effect::Delay(pause)) => {
                std::thread::sleep(pause);
                self.inner.write_all_at(data, offset)
            }
            None => self.inner.write_all_at(data, offset),
        }
    }
    fn size(&self) -> Result<u64> {
        self.check(Op::Size)?;
        self.inner.size()
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.check(Op::SetSize)?;
        self.inner.set_size(size)
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        self.check(Op::Sync)?;
        self.inner.sync(durability)
    }
}
