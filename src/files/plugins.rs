//! \[DESIGN\] Where plug-in engines and index methods keep files of their
//! own (RULES.md D56): each a directory of its name. Given through `Host`
//! as the plug-in is registered, before the database may be opened from
//! its directory, so the place is shared and changed in place as it is:
//! in the database's `plugins/`, through its file manager, once it is kept
//! in a directory; in memory, of its memory budget, until then. A plug-in
//! asks for its directory when it makes a file, not once as it is
//! registered. It makes what it writes durable itself; its directory is
//! made durably.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::error::{Code, Result, SqlError};
use crate::memory::MemoryBudget;

/// \[DESIGN\] The directory the plug-ins' directories are in.
pub const PLUGINS: &str = "plugins";

/// \[DESIGN\] Where a database's plug-ins keep their files: a file system,
/// and the directory their directories are in.
pub struct PluginFiles {
    place: RwLock<(Arc<dyn FileSystem>, PathBuf)>,
}

impl PluginFiles {
    /// In memory, reserved of `memory`, as a database in memory keeps them.
    pub(crate) fn in_memory(memory: Arc<MemoryBudget>) -> PluginFiles {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::within(memory, "a plug-in's file"));
        PluginFiles { place: RwLock::new((fs, Path::new("/").join(PLUGINS))) }
    }

    /// Keeps them in `dir` of `fs` from now on: the database's `plugins/`,
    /// as it is opened from its directory.
    pub(crate) fn set_place(&self, fs: Arc<dyn FileSystem>, dir: PathBuf) {
        *self.place.write().unwrap_or_else(PoisonError::into_inner) = (fs, dir);
    }

    /// The file system and directory of the files of the plug-in `name`,
    /// made if missing.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `name` is not letters, digits, `_` and `-`, so a
    /// directory's name; -915 if the directory cannot be made.
    pub fn directory(&self, name: &str) -> Result<(Arc<dyn FileSystem>, PathBuf)> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a plug-in's directory is named by letters, digits, _ and -, not {name:?}"),
            ));
        }
        let (fs, root) = self.place.read().unwrap_or_else(PoisonError::into_inner).clone();
        let dir = root.join(name);
        fs.create_dir_durably(&dir)?;
        Ok((fs, dir))
    }
}

impl std::fmt::Debug for PluginFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dir = self.place.read().unwrap_or_else(PoisonError::into_inner).1.clone();
        f.debug_struct("PluginFiles").field("dir", &dir).finish_non_exhaustive()
    }
}
