//! The database of `turboreld` opened and settled (RULES.md D36, D57):
//! opened from its directory as itself, or as the replica its settings
//! name, its node started with the server; and its settings given at
//! start set once the server is made, with the owner `--owner` names,
//! on a replica once it serves as primary. Split from lib.rs (RULES.md
//! R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use tokio::runtime::Runtime;
use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel_server::replication::{self, Replication};

use crate::{Config, Store, config_file, users};

/// Sets the settings `late`, the server's own declared by `Server::new`
/// among them, once the server is made, its verifiers told
/// SERVER.PASSWORD_ITERATIONS as given here, as every plug-in is told its
/// settings (RULES.md D36); and makes the owner `--owner` names. On a
/// replica `node`, which logs them, both once it serves as primary, on
/// `runtime` (RULES.md D57).
///
/// # Errors
///
/// A setting refused; `--owner` given for a database kept in a directory
/// that is not replicated.
pub(crate) fn settled(
    config: &Config,
    db: &Database,
    (node, runtime): (Option<&Replication>, Option<&Runtime>),
    late: &[&config_file::Given],
) -> Result<(), String> {
    if let (Store::Dir(_), Some(node), Some(runtime)) = (&config.store, node, runtime) {
        let (deferred, owner) = (late.iter().map(|g| (*g).clone()).collect::<Vec<_>>(), config.owner.clone());
        users::once_serving(
            db,
            node,
            runtime,
            ("the settings given at start", move |primary| {
                deferred.iter().try_for_each(|g| g.set(primary))?;
                owner.as_ref().map_or(Ok(()), |owner| users::make_owner_unless_owned(primary, owner))
            }),
        );
        return Ok(());
    }
    late.iter().try_for_each(|g| g.set(db))?;
    if let Some(owner) = &config.owner {
        if config.store != Store::Memory {
            return Err(
                "--owner makes the owner of a database kept nowhere, or of a replicated one: in a directory, user add NAME --owner".into(),
            );
        }
        users::make_owner(db, owner)?;
    }
    Ok(())
}

/// Opens `db` from `dir`: as the replica its settings name, if they name
/// one (RULES.md D57), its node started on `runtime`, which a server has
/// and a command on the directory has not; else as itself.
///
/// # Errors
///
/// Why it could not be opened, for the operator: a replicated database
/// is managed through its primary, not by a command on its directory.
pub(crate) fn opened(db: &Database, dir: &Path, runtime: Option<&Runtime>) -> Result<Option<Replication>, String> {
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    let failed = |e: turborel::error::SqlError| format!("cannot open {}: {e}", dir.display());
    let Some(runtime) = runtime else {
        if replication::replicated(db) {
            return Err("the database is replicated: it is managed through its primary, by SQL, not by a command on its directory".into());
        }
        db.open_files(&fs, dir, &OpenOptions::default()).map_err(failed)?;
        return Ok(None);
    };
    let Some(prepared) = replication::prepare(db, dir).map_err(failed)? else {
        db.open_files(&fs, dir, &OpenOptions::default()).map_err(failed)?;
        return Ok(None);
    };
    let holds = db.open_following(&fs, dir, &OpenOptions::default()).map_err(failed)?;
    let node = prepared.start(db, runtime.handle(), holds).map_err(|e| format!("cannot start the replica: {e}"))?;
    Ok(Some(node))
}
