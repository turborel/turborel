//! \[DESIGN\] What each file of a database is (RULES.md D56, D57): its kind,
//! known by its name and its directory's, as the database lays its
//! directory out (`db/files.rs`); its class of durability, which says
//! whether it holds state a crash keeps; and its owner. The durable files
//! are what an export of the database carries besides its log and its
//! catalog (`Database::export_manifest`), so a file of no kind known here
//! is refused there, rather than left behind.

use std::path::Path;

use super::plugins::PLUGINS;

/// \[DESIGN\] A file's class of durability (RULES.md D56).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    /// Synced as its owner says, kept across a crash: the catalog, the
    /// data file, the log, a plug-in's own files.
    Durable,
    /// What is written to it reaches its file at a commit: the data file's
    /// side files (`disk/shadow.rs`), whose batch not committed a crash
    /// throws away.
    DurableAtCommit,
    /// Never synced, begun anew at each open: the log of older row
    /// versions.
    Unsynced,
    /// Removed when dropped, cleared at each open: spill files.
    Scratch,
}

impl FileClass {
    /// Its name, as INFORMATION_SCHEMA.FILES shows it.
    pub fn name(self) -> &'static str {
        match self {
            FileClass::Durable => "DURABLE",
            FileClass::DurableAtCommit => "DURABLE AT COMMIT",
            FileClass::Unsynced => "UNSYNCED",
            FileClass::Scratch => "SCRATCH",
        }
    }
}

/// \[DESIGN\] Whose a file is (RULES.md D56).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOwner {
    /// The database: its catalog, its data file and its side files, and
    /// the log of older row versions of its built-in engines.
    Database,
    /// The database's log.
    Log,
    /// A spill area's.
    Spill,
    /// The plug-in, engine or index method, of this name: its own files,
    /// in a directory of its name (`files/plugins.rs`).
    Plugin(String),
    /// A server's own, noted (`Registry::note`).
    Server,
    /// None known: a file in the database's directory the database does
    /// not lay out.
    Unknown,
}

/// \[DESIGN\] A file's kind, by its name and its directory's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// The data file of the built-in engines and index methods.
    Data,
    /// One of the data file's side files.
    Shadow,
    /// One of the catalog's files.
    Catalog,
    /// A segment of the log.
    Log,
    /// A segment of the log of older row versions.
    Undo,
    /// A spill file.
    Spill,
    /// A plug-in's own file.
    Plugin,
    /// A server's own file, noted.
    Server,
    /// Another.
    Other,
}

impl FileKind {
    /// The kind of the file at `path`, by its name and its directory's: the
    /// data file and its side files, the catalog, a segment of the log or
    /// of older row versions, a spill file, a plug-in's own file (in a
    /// directory of `plugins/`, first, whatever its name), or another.
    pub fn of(path: &Path) -> FileKind {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let dir = path.parent().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or_default();
        if path.ancestors().skip(2).any(|a| a.file_name().is_some_and(|n| n == PLUGINS)) {
            FileKind::Plugin
        } else if name == "data" {
            FileKind::Data
        } else if name.starts_with("data.shadow") {
            FileKind::Shadow
        } else if name.starts_with("catalog.") {
            FileKind::Catalog
        } else if dir == "log" {
            FileKind::Log
        } else if dir == "undo" {
            FileKind::Undo
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("spill")) {
            FileKind::Spill
        } else {
            FileKind::Other
        }
    }

    /// Its name, as INFORMATION_SCHEMA.FILES shows it.
    pub fn name(self) -> &'static str {
        match self {
            FileKind::Data => "DATA",
            FileKind::Shadow => "SHADOW",
            FileKind::Catalog => "CATALOG",
            FileKind::Log => "LOG",
            FileKind::Undo => "UNDO",
            FileKind::Spill => "SPILL",
            FileKind::Plugin => "PLUGIN",
            FileKind::Server => "SERVER",
            FileKind::Other => "OTHER",
        }
    }

    /// The class of durability of a file of its kind; none for one not
    /// the database's: a server's, or one it does not lay out.
    pub fn class(self) -> Option<FileClass> {
        match self {
            FileKind::Data | FileKind::Catalog | FileKind::Log | FileKind::Plugin => Some(FileClass::Durable),
            FileKind::Shadow => Some(FileClass::DurableAtCommit),
            FileKind::Undo => Some(FileClass::Unsynced),
            FileKind::Spill => Some(FileClass::Scratch),
            FileKind::Server | FileKind::Other => None,
        }
    }

    /// The owner of the file at `path`, of this kind: of a plug-in's, the
    /// plug-in its directory is named for.
    pub fn owner(self, path: &Path) -> FileOwner {
        match self {
            FileKind::Data | FileKind::Shadow | FileKind::Catalog | FileKind::Undo => FileOwner::Database,
            FileKind::Log => FileOwner::Log,
            FileKind::Spill => FileOwner::Spill,
            FileKind::Plugin => plugin_of(path).map_or(FileOwner::Unknown, FileOwner::Plugin),
            FileKind::Server => FileOwner::Server,
            FileKind::Other => FileOwner::Unknown,
        }
    }
}

/// The plug-in whose directory, in `plugins/`, holds `path`.
fn plugin_of(path: &Path) -> Option<String> {
    let mut below: Option<&Path> = None;
    for at in path.ancestors().skip(1) {
        if at.file_name().is_some_and(|n| n == PLUGINS) {
            return below.and_then(Path::file_name).and_then(|n| n.to_str()).map(str::to_owned);
        }
        below = Some(at);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D56: each file's kind, by its name and
    /// its directory's; its class of durability; and its owner, a
    /// plug-in's the one its directory is named for.
    #[test]
    fn files_are_of_their_kinds_classes_and_owners() {
        let durable = Some(FileClass::Durable);
        let mine = || FileOwner::Plugin("mine".into());
        for (path, kind, class, owner) in [
            ("db/data", FileKind::Data, durable, FileOwner::Database),
            ("db/data.shadow1", FileKind::Shadow, Some(FileClass::DurableAtCommit), FileOwner::Database),
            ("db/catalog.0", FileKind::Catalog, durable, FileOwner::Database),
            ("db/log/segment-0000000001.log", FileKind::Log, durable, FileOwner::Log),
            ("db/undo/segment-0000000001.log", FileKind::Undo, Some(FileClass::Unsynced), FileOwner::Database),
            ("db/tmp/turborel-1-2-3.spill", FileKind::Spill, Some(FileClass::Scratch), FileOwner::Spill),
            ("db/lock", FileKind::Other, None, FileOwner::Unknown),
            ("db/plugins/mine/data", FileKind::Plugin, durable, mine()),
            ("db/plugins/mine/parts/0", FileKind::Plugin, durable, mine()),
        ] {
            let found = FileKind::of(Path::new(path));
            assert_eq!((found, found.class(), found.owner(Path::new(path))), (kind, class, owner), "{path}");
        }
        assert_eq!(FileKind::Server.class(), None);
        assert_eq!(FileKind::Server.owner(Path::new("/server.log")), FileOwner::Server);
        assert_eq!(FileKind::Plugin.owner(Path::new("loose")), FileOwner::Unknown, "in no plug-in's directory");
        let names: Vec<&str> =
            [FileClass::Durable, FileClass::DurableAtCommit, FileClass::Unsynced, FileClass::Scratch].map(FileClass::name).to_vec();
        assert_eq!(names, ["DURABLE", "DURABLE AT COMMIT", "UNSYNCED", "SCRATCH"]);
    }
}
