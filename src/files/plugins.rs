//! \[DESIGN\] Where plug-in engines and index methods keep files of their
//! own (RULES.md D56): each a directory of its name. Given through `Host`
//! as the plug-in is registered, before the database may be opened from
//! its directory, so the place is shared and changed in place as it is:
//! in the database's `plugins/`, through its file manager, once it is kept
//! in a directory; in memory, of its memory budget, until then. A plug-in
//! asks for its directory when it makes a file, not once as it is
//! registered; its directory is made durably.
//!
//! A directory is kept as the plug-in declares it (`PluginFiles::declare`):
//! as written, the plug-in making what it writes durable itself, by
//! default; or shadowed (`disk/shadow.rs`), once kept in a directory: what
//! is written, made and removed in it reaches its files at the database's
//! checkpoints, held, sealed and committed beside the data file's
//! (`db/checkpoint.rs`), and recovered as the database opens, so after a
//! crash it is as the last checkpoint left it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::shadow::ShadowFileSystem;
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::memory::MemoryBudget;

/// \[DESIGN\] The directory the plug-ins' directories are in.
pub const PLUGINS: &str = "plugins";

/// \[DESIGN\] How a plug-in's directory keeps what is written to it
/// (RULES.md D56), as the plug-in declares it (`PluginFiles::declare`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// As written: the plug-in makes it durable itself, as it syncs. The
    /// default.
    AsWritten,
    /// Shadowed: what is written, made and removed in it reaches its files
    /// at the database's checkpoints, so after a crash it is as the last
    /// checkpoint left it. A plug-in's capture, written as its engine's
    /// `StorageEngine::checkpoint` is called, is durable by the time its
    /// `checkpoint_finish` is. Names ending `.shadow0` or `.shadow1` are
    /// refused in it. A database in memory keeps it as written.
    Shadowed,
}

/// \[DESIGN\] Where a database's plug-ins keep their files: a file system,
/// and the directory their directories are in.
pub struct PluginFiles {
    place: RwLock<Place>,
}

/// Where they are, and how each is kept.
struct Place {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// The shadowing of `fs`, once kept in a directory.
    shadows: Option<Arc<ShadowFileSystem>>,
    /// The directories declared, and how each is kept.
    declared: BTreeMap<String, Kept>,
    /// The directories given as written since the place was set.
    given: Vec<String>,
}

/// Refuses `name` unless it is letters, digits, `_` and `-`, so a
/// directory's name.
fn checked(name: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] a plug-in's directory is named by letters, digits, _ and -, not {name:?}")));
    }
    Ok(())
}

impl PluginFiles {
    /// In memory, reserved of `memory`, as a database in memory keeps them.
    pub(crate) fn in_memory(memory: Arc<MemoryBudget>) -> PluginFiles {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::within(memory, "a plug-in's file"));
        let place = Place { fs, dir: Path::new("/").join(PLUGINS), shadows: None, declared: BTreeMap::new(), given: Vec::new() };
        PluginFiles { place: RwLock::new(place) }
    }

    fn read(&self) -> RwLockReadGuard<'_, Place> {
        self.place.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Place> {
        self.place.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps them in `dir` of `fs` from now on: the database's `plugins/`,
    /// as it is opened from its directory, before its tables are; the
    /// side files of shadowed directories checksummed with `hash`, each
    /// such directory recovered first.
    ///
    /// # Errors
    ///
    /// As `ShadowFileSystem::recover`.
    pub(crate) fn set_place(&self, fs: Arc<dyn FileSystem>, dir: PathBuf, hash: Arc<dyn HashFunction>) -> Result<()> {
        let shadows = Arc::new(ShadowFileSystem::new(Arc::clone(&fs), hash));
        let mut place = self.write();
        for (name, _) in place.declared.iter().filter(|(_, kept)| **kept == Kept::Shadowed) {
            shadows.recover(&dir.join(name))?;
        }
        place.fs = fs;
        place.dir = dir;
        place.shadows = Some(shadows);
        place.given.clear();
        Ok(())
    }

    /// Declares how the directory of the plug-in `name` keeps what is
    /// written to it; one declared shadowed after the database was opened
    /// is recovered first. A plug-in declares it as it is registered
    /// (`StorageEngine::attach`, `IndexMethod::attach`), before it asks for
    /// its directory.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `name` is no directory's, was declared otherwise
    /// before, or was given as written before it was declared shadowed;
    /// as `ShadowFileSystem::recover`.
    pub fn declare(&self, name: &str, kept: Kept) -> Result<()> {
        checked(name)?;
        let mut place = self.write();
        let refused = |why: String| -> Result<()> {
            Err(SqlError::new(Code::Api, format!("[DESIGN] RULES.md D56: plug-in {name}'s directory {why}")))
        };
        if let Some(before) = place.declared.get(name).filter(|before| **before != kept) {
            return refused(format!("was declared {before:?}, not {kept:?}"));
        }
        if kept == Kept::Shadowed {
            if place.given.iter().any(|n| n == name) {
                return refused("was given as written before it was declared shadowed".to_owned());
            }
            if let Some(shadows) = &place.shadows {
                shadows.recover(&place.dir.join(name))?;
            }
        }
        place.declared.insert(name.to_owned(), kept);
        Ok(())
    }

    /// The file system and directory of the files of the plug-in `name`,
    /// made if missing: the shadowing file system, if it is declared
    /// shadowed and kept in a directory.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `name` is not letters, digits, `_` and `-`, so a
    /// directory's name; -915 if the directory cannot be made.
    pub fn directory(&self, name: &str) -> Result<(Arc<dyn FileSystem>, PathBuf)> {
        checked(name)?;
        let (fs, dir) = {
            let mut place = self.write();
            let shadowed = place.declared.get(name) == Some(&Kept::Shadowed);
            let fs: Arc<dyn FileSystem> = match place.shadows.as_ref().filter(|_| shadowed) {
                Some(shadows) => Arc::<ShadowFileSystem>::clone(shadows),
                None => Arc::clone(&place.fs),
            };
            if !shadowed && !place.given.iter().any(|n| n == name) {
                place.given.push(name.to_owned());
            }
            (fs, place.dir.join(name))
        };
        fs.create_dir_durably(&dir)?;
        Ok((fs, dir))
    }

    /// The shadowing of the shadowed directories, once kept in a directory.
    fn shadows(&self) -> Option<Arc<ShadowFileSystem>> {
        self.read().shadows.clone()
    }

    /// Holds what the shadowed directories' files write in memory, as a
    /// checkpoint captures (`ShadowFileSystem::hold`).
    ///
    /// # Errors
    ///
    /// As `ShadowFileSystem::hold`.
    pub(crate) fn hold(&self) -> Result<()> {
        self.shadows().map_or(Ok(()), |s| s.hold())
    }

    /// Gives a hold up, for a capture given up (`ShadowFileSystem::release`).
    ///
    /// # Errors
    ///
    /// As `ShadowFileSystem::release`.
    pub(crate) fn release(&self) -> Result<()> {
        self.shadows().map_or(Ok(()), |s| s.release())
    }

    /// Seals what the shadowed directories' files wrote, for the
    /// checkpoint's commit (`ShadowFileSystem::seal`).
    ///
    /// # Errors
    ///
    /// As `ShadowFileSystem::seal`.
    pub(crate) fn seal(&self) -> Result<()> {
        self.shadows().map_or(Ok(()), |s| s.seal())
    }

    /// Commits what was sealed (`ShadowFileSystem::commit_sealed`).
    ///
    /// # Errors
    ///
    /// As `ShadowFileSystem::commit_sealed`.
    pub(crate) fn commit_sealed(&self) -> Result<()> {
        self.shadows().map_or(Ok(()), |s| s.commit_sealed())
    }
}

impl std::fmt::Debug for PluginFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let place = self.read();
        f.debug_struct("PluginFiles").field("dir", &place.dir).field("declared", &place.declared).finish_non_exhaustive()
    }
}
