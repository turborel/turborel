//! What a run of real databases in the simulator did.

use super::super::Report;

/// What a run of real databases did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatabaseReport {
    /// What replication did.
    pub replication: Report,
    /// Transactions the primaries' databases committed.
    pub committed: u64,
    /// Commits left in doubt as their primary stopped serving or crashed.
    pub in_doubt: u64,
    /// Transactions open on a primary's database as it stopped or crashed.
    pub interrupted: u64,
    /// Databases promoted in a view after the first.
    pub promoted: u64,
    /// Checkpoints taken by following databases.
    pub following_checkpoints: u64,
    /// Databases opened again from their files after a crash.
    pub reopened: u64,
    /// Reads served by backups, each holding its snapshot a while.
    pub backup_reads: u64,
    /// Reconfigurations asked of a primary's database that were done.
    pub reconfigured: u64,
    /// Backups compared with the primary at the end.
    pub compared: usize,
    /// \[EXTENSION\] Nodes stopped by a replication plug-in's death
    /// (`Database::stop_node`, RULES.md D270).
    pub node_stops: u64,
}
