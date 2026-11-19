//! \[EXTENSION\] The host kinds of files and connections (RULES.md D251,
//! D254): a file system, each operation by path, and a listener, whose
//! process keeps one connection while it is open.

use turborel::bytes::{Put, Reader};
use turborel::plugin::manifest::HostKind;
use turborel::plugin::payload::hosted as h;
use turborel::plugin::payload::put_strings;

use super::super::registry::Plugin;
use super::super::serve::Cx;
use super::super::{Answer, PluginError};
use super::HostObject;

/// How a file is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Created, for reading and writing; it must not be there.
    Create,
    /// For reading and writing; it must be there.
    Write,
    /// For reading only; it must be there.
    Read,
}

/// Kind 26, a file system. Each operation names its file by path.
/// Errors of `open`, `remove`, `read_dir` and `lock` are answers, as a
/// file not there is; any other stops the node.
pub trait Files: Send {
    /// The file `path` opened, as `mode` says.
    ///
    /// # Errors
    ///
    /// It cannot be.
    fn open(&mut self, path: &str, mode: Mode) -> Answer<()>;
    /// At most `max` bytes of `path` from `offset`: fewer only at its end.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn read(&mut self, path: &str, offset: u64, max: u32) -> Answer<Vec<u8>>;
    /// `bytes` written to `path` at `offset`.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn write(&mut self, path: &str, offset: u64, bytes: &[u8]) -> Answer<()>;
    /// The size of `path`.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn size(&mut self, path: &str) -> Answer<u64>;
    /// `path` made `size` bytes long.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn set_size(&mut self, path: &str, size: u64) -> Answer<()>;
    /// What was written to `path` made durable: 0 not at all, 1 its data,
    /// 2 its data and metadata.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn sync(&mut self, path: &str, durability: u8) -> Answer<()>;
    /// The file `path` removed.
    ///
    /// # Errors
    ///
    /// It cannot be.
    fn remove(&mut self, path: &str) -> Answer<()>;
    /// Whether something is at `path`.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn exists(&mut self, path: &str) -> Answer<bool>;
    /// The directory `path` and its missing parents made.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn create_dirs(&mut self, path: &str) -> Answer<()>;
    /// The paths of the directory's entries, sorted.
    ///
    /// # Errors
    ///
    /// It cannot be read.
    fn read_dir(&mut self, path: &str) -> Answer<Vec<String>>;
    /// The directory's entries made durable.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn sync_dir(&mut self, path: &str) -> Answer<()>;
    /// `path` locked against any other opener: a token of the lock.
    ///
    /// # Errors
    ///
    /// Another holds it.
    fn lock(&mut self, path: &str) -> Answer<u64>;
    /// The lock of `token` given up.
    fn unlock(&mut self, token: u64);
    /// `from` renamed `to`.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn rename(&mut self, from: &str, to: &str) -> Answer<()>;
    /// The directory `path` and all it holds removed.
    ///
    /// # Errors
    ///
    /// The file system's.
    fn remove_tree(&mut self, path: &str) -> Answer<()>;
}

/// Kind 28, a listener. Its process keeps one connection from `accept`
/// until it closes.
pub trait Accept: Send {
    /// Waits `wait_ms` at most for a client: where it is from, an
    /// address or `local`; none if none came.
    ///
    /// # Errors
    ///
    /// The listener's, which stops the node.
    fn accept(&mut self, wait_ms: u32) -> Answer<Option<String>>;
    /// `out` sent to the client, then what it sent, waiting `wait_ms` at
    /// most for some if `out` is empty; and whether it closed.
    ///
    /// # Errors
    ///
    /// As `accept`.
    fn relay(&mut self, wait_ms: u32, out: &[u8]) -> Answer<(bool, Vec<u8>)>;
}

/// An operation no kind has.
fn unknown(op: u8) -> PluginError {
    PluginError::external(&format!("no host operation {op}"))
}

/// A file system served.
struct Filed<T>(T);

impl<T: Files> Filed<T> {
    /// The answer of an operation of a file.
    fn of_file(&mut self, op: u8, path: &str, r: &mut Reader<'_>, out: &mut Vec<u8>) -> Answer<()> {
        match op {
            h::FILES_READ => out.put_bytes(&self.0.read(path, r.u64()?, r.u32()?)?),
            h::FILES_WRITE => {
                let offset = r.u64()?;
                self.0.write(path, offset, r.bytes()?)?;
            }
            h::FILES_SIZE => out.put_u64(self.0.size(path)?),
            h::FILES_SET_SIZE => self.0.set_size(path, r.u64()?)?,
            h::FILES_SYNC => self.0.sync(path, r.u8()?)?,
            other => return Err(unknown(other)),
        }
        Ok(())
    }

    /// The answer of an operation of a path.
    fn of_path(&mut self, op: u8, path: &str, r: &mut Reader<'_>, out: &mut Vec<u8>) -> Answer<()> {
        match op {
            h::FILES_OPEN => {
                let mode = match r.u8()? {
                    0 => Mode::Create,
                    1 => Mode::Write,
                    _ => Mode::Read,
                };
                self.0.open(path, mode)?;
            }
            h::FILES_REMOVE => self.0.remove(path)?,
            h::FILES_EXISTS => out.put_u8(u8::from(self.0.exists(path)?)),
            h::FILES_CREATE_DIRS => self.0.create_dirs(path)?,
            h::FILES_READ_DIR => put_strings(out, &self.0.read_dir(path)?),
            h::FILES_SYNC_DIR => self.0.sync_dir(path)?,
            h::FILES_LOCK => out.put_u64(self.0.lock(path)?),
            h::FILES_RENAME => self.0.rename(path, &r.str()?)?,
            h::FILES_REMOVE_TREE => self.0.remove_tree(path)?,
            other => return self.of_file(other, path, r, out),
        }
        Ok(())
    }
}

impl<T: Files> HostObject for Filed<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut r = Reader::received(payload, "host call");
        let mut out = Vec::new();
        if op == h::FILES_UNLOCK {
            self.0.unlock(r.u64()?);
            return Ok(out);
        }
        let path = r.str()?;
        self.of_path(op, &path, &mut r, &mut out)?;
        Ok(out)
    }
}

/// A listener served.
struct Listening<T>(T);

impl<T: Accept> HostObject for Listening<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut out = Vec::new();
        match op {
            h::LISTENER_ACCEPT => {
                let from = self.0.accept(Reader::received(payload, "host call").u32()?)?;
                out.put_u8(u8::from(from.is_some()));
                out.put_str(&from.unwrap_or_default());
            }
            h::LISTENER_RELAY => {
                let mut r = Reader::received(payload, "host call");
                let wait = r.u32()?;
                let (closed, back) = self.0.relay(wait, r.bytes()?)?;
                out.put_u8(u8::from(closed));
                out.put_bytes(&back);
            }
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

impl Plugin {
    /// It, serving `files` as HOST FILE SYSTEM `name`.
    #[must_use]
    pub fn host_files(self, name: &str, files: impl Files + 'static) -> Plugin {
        self.host(HostKind::FileSystem.code(), name, Filed(files))
    }
    /// It, serving `listener` as HOST LISTENER `name`.
    #[must_use]
    pub fn host_listener(self, name: &str, listener: impl Accept + 'static) -> Plugin {
        self.host(HostKind::Listener.code(), name, Listening(listener))
    }
}
