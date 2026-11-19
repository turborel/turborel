//! \[EXTENSION\] The statements of backups (RULES.md D160, D164, D166):
//! BACKUP DATABASE, CREATE RESTORE POINT and RESCAN BACKUP TARGET, each
//! run by the database's owner alone, outside a transaction. A backup run
//! so is cancelled as any statement is (`Session::canceller`), between two
//! chunks or records; what it wrote is removed, and the history records it
//! cancelled. A backup's id, once it is done, is in
//! INFORMATION_SCHEMA.BACKUPS.

use super::BackupOptions;
use crate::ast::UserStatement;
use crate::db::{Completion, Session};
use crate::error::{At, Result};

impl Session {
    /// Runs `stmt` if it is a statement of backups: its completion, or none
    /// if it is another of the owner's statements.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 in a transaction; -104 if the session's identifier is
    /// not the owner's; -920 if cancelled; those of `Database::backup`,
    /// `create_restore_point`, `rescan_target`.
    pub(in crate::db) fn backup_statement(&self, stmt: &UserStatement) -> Result<Option<Completion>> {
        match stmt {
            UserStatement::Backup { location, incremental, label } => {
                self.as_owner("BACKUP DATABASE")?;
                let options = BackupOptions { location: location.clone(), incremental: incremental.clone(), label: label.clone() };
                self.db.backup_stopping(&options, Some(&self.canceller()))?;
            }
            UserStatement::RestorePoint { name: (name, at) } => {
                self.as_owner("CREATE RESTORE POINT")?;
                self.db.create_restore_point(name).or_at(*at)?;
            }
            UserStatement::Rescan { location } => {
                self.as_owner("RESCAN BACKUP TARGET")?;
                self.db.rescan_target(location)?;
            }
            UserStatement::Create { .. }
            | UserStatement::Alter { .. }
            | UserStatement::RoleCreator { .. }
            | UserStatement::Drop { .. }
            | UserStatement::Owner { .. }
            | UserStatement::Members { .. } => return Ok(None),
        }
        Ok(Some(Completion::Success))
    }
}
