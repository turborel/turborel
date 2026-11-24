//! \[EXTENSION\] The backups a target holds, and a backup's chain
//! (RULES.md D161, D162): an incremental backup, then the one it follows,
//! and so on to a full one. A backup is in its target once its manifest
//! is, which it writes last.

use std::sync::Arc;

use crate::backup::manifest::Manifest;
use crate::backup::object::{ObjectIn, Reading};
use crate::backup::{BACKUPS, BackupKind, BackupTarget, MANIFEST, backup_prefix, damaged};
use crate::error::{Code, Result, SqlError};

/// \[EXTENSION\] The bound on a chain's length. A chain longer is refused
/// as damage, a cycle among the manifests' parents (RULES.md R-CF-6).
pub const MAX_CHAIN: usize = 10_000;

/// \[EXTENSION\] The ids of the backups `target` holds whole, in the order
/// they were taken.
///
/// # Errors
///
/// The target's.
pub fn backup_ids(target: &dyn BackupTarget) -> Result<Vec<String>> {
    let names = target.list(BACKUPS)?;
    let id = |n: &String| n.strip_prefix(BACKUPS)?.strip_suffix(MANIFEST)?.strip_suffix('/').map(str::to_owned);
    let mut ids: Vec<String> = names.iter().filter_map(id).filter(|i| !i.contains('/')).collect();
    ids.sort();
    Ok(ids)
}

/// \[EXTENSION\] The manifest of backup `id` of `target`, or of the newest
/// if `id` is empty.
///
/// # Errors
///
/// SQLCODE -910 if there is no such backup; -916 (SQLSTATE TBB01) if its
/// manifest is damaged, or names another id; the target's.
pub fn manifest_of(target: &Arc<dyn BackupTarget>, id: &str, reading: &Reading) -> Result<Manifest> {
    let id = if id.is_empty() {
        backup_ids(&**target)?
            .pop()
            .ok_or_else(|| SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D161: {} holds no backup", target.location())))?
    } else {
        id.to_owned()
    };
    let name = format!("{}{MANIFEST}", backup_prefix(&id));
    if !target.list(&name)?.contains(&name) {
        return Err(SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D161: {} holds no backup {id}", target.location())));
    }
    let manifest = Manifest::decode(&ObjectIn::only(target, &name, reading)?).map_err(|e| damaged(&name, &e.msg))?;
    if manifest.id != id {
        return Err(damaged(&name, &format!("it is the manifest of {}", manifest.id)));
    }
    Ok(manifest)
}

/// \[EXTENSION\] The chain of backup `id` (newest if empty) of `target`:
/// it, then each it follows, the last a full backup.
///
/// # Errors
///
/// As `manifest_of`, for each; SQLCODE -916 (SQLSTATE TBB01) if a parent
/// is missing, or the chain is longer than `MAX_CHAIN`.
pub fn chain_of(target: &Arc<dyn BackupTarget>, id: &str, reading: &Reading) -> Result<Vec<Manifest>> {
    let mut chain = vec![manifest_of(target, id, reading)?];
    while let Some(last) = chain.last().filter(|m| m.kind == BackupKind::Incremental) {
        if chain.len() >= MAX_CHAIN {
            return Err(damaged(&backup_prefix(&last.id), "its chain of parents does not end"));
        }
        let parent = last.parent.clone();
        let found = manifest_of(target, &parent, reading).map_err(|e| {
            if e.code == Code::Api { damaged(&backup_prefix(&parent), "a backup follows it, and it is missing") } else { e }
        })?;
        chain.push(found);
    }
    Ok(chain)
}
