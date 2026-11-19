//! \[EXTENSION\] A host plug-in's file system (RULES.md D251): the files
//! a database is opened from, kept wherever the program keeps them.
//!
//! Each operation names its file by path, so any process of the program
//! answers it, and a read or write of a page is one call. A lock is the
//! program's to keep across its processes, given up when the database
//! drops it. A write or sync that fails stops the node, as a failed
//! device write would leave the database unable to say what is durable.

use std::path::{Path, PathBuf};

use crate::bytes::Put;
use crate::disk::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::plugin::bridge::{answer, byte, number};
use crate::plugin::hosted::HostLink;
use crate::plugin::payload::hosted::{
    FILES_CREATE_DIRS, FILES_EXISTS, FILES_LOCK, FILES_OPEN, FILES_READ, FILES_READ_DIR, FILES_REMOVE, FILES_REMOVE_TREE, FILES_RENAME,
    FILES_SET_SIZE, FILES_SIZE, FILES_SYNC, FILES_SYNC_DIR, FILES_UNLOCK, FILES_WRITE,
};

/// A payload of `path`.
fn of_path(path: &Path) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.put_str(&path.to_string_lossy());
    payload
}

/// A host plug-in's file system.
#[derive(Debug, Clone)]
pub struct PluginFiles {
    link: HostLink,
}

impl PluginFiles {
    /// The file system of `link`.
    pub fn new(link: HostLink) -> PluginFiles {
        PluginFiles { link }
    }
}

impl FileSystem for PluginFiles {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let mut payload = of_path(path);
        payload.put_u8(match mode {
            OpenMode::CreateNew => 0,
            OpenMode::ReadWrite => 1,
            OpenMode::ReadOnly => 2,
        });
        // That a file is not there, or is there already, is an answer.
        self.link.ask(FILES_OPEN, payload).map_err(io)?;
        Ok(Box::new(PluginFile { link: self.link.clone(), path: path.to_path_buf() }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.link.ask(FILES_REMOVE, of_path(path)).map(drop).map_err(io)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        let got = self.link.call(FILES_EXISTS, of_path(path))?;
        Ok(answer(&got, byte)? == 1)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.link.call(FILES_CREATE_DIRS, of_path(path)).map(drop)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let got = self.link.ask(FILES_READ_DIR, of_path(path)).map_err(io)?;
        let names = answer(&got, crate::plugin::payload::strings)?;
        Ok(names.into_iter().map(PathBuf::from).collect())
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.link.call(FILES_SYNC_DIR, of_path(path)).map(drop)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let mut payload = of_path(from);
        payload.put_str(&to.to_string_lossy());
        self.link.call(FILES_RENAME, payload).map(drop)
    }
    fn remove_tree(&self, path: &Path) -> Result<()> {
        self.link.call(FILES_REMOVE_TREE, of_path(path)).map(drop)
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        let got = self.link.ask(FILES_LOCK, of_path(path)).map_err(io)?;
        let token = answer(&got, number)?;
        Ok(FileLock::new(Held { link: self.link.clone(), token }))
    }
}

/// A lock a host file system holds, given up as it drops.
struct Held {
    link: HostLink,
    token: u64,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut payload = Vec::new();
        payload.put_u64(self.token);
        // Given up or not, the lock is the database's no more; a program
        // that cannot answer has stopped the node.
        drop(self.link.call(FILES_UNLOCK, payload));
    }
}

/// A file of a host file system, by path.
struct PluginFile {
    link: HostLink,
    path: PathBuf,
}

impl PluginFile {
    /// A payload of its path.
    fn payload(&self) -> Vec<u8> {
        of_path(&self.path)
    }
}

impl DiskFile for PluginFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        let mut payload = self.payload();
        payload.put_u64(offset);
        payload.put_u32(u32::try_from(buf.len()).unwrap_or(u32::MAX));
        let got = self.link.call(FILES_READ, payload)?;
        let bytes = answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?;
        let read = bytes.len().min(buf.len());
        let (into, _) = buf.split_at_mut(read);
        into.copy_from_slice(bytes.get(..read).unwrap_or_default());
        Ok(read)
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        let mut payload = self.payload();
        payload.put_u64(offset);
        payload.put_bytes(data);
        self.link.call(FILES_WRITE, payload).map(drop)
    }
    fn size(&self) -> Result<u64> {
        let got = self.link.call(FILES_SIZE, self.payload())?;
        answer(&got, number)
    }
    fn set_size(&self, size: u64) -> Result<()> {
        let mut payload = self.payload();
        payload.put_u64(size);
        self.link.call(FILES_SET_SIZE, payload).map(drop)
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        let mut payload = self.payload();
        payload.put_u8(match durability {
            Durability::None => 0,
            Durability::Data => 1,
            Durability::Full => 2,
        });
        self.link.call(FILES_SYNC, payload).map(drop)
    }
}

/// A program's refusal of a file operation, as the file system's error.
fn io(e: SqlError) -> SqlError {
    SqlError { code: Code::Io, ..e }
}
