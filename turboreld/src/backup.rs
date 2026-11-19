//! \[EXTENSION\] `turboreld --dir PATH [--keys DIR] --restore TARGET ...`
//! and `turboreld [--keys DIR] --verify-backup TARGET ...` (RULES.md D161,
//! D164): a backup restored into `--dir`, a directory not there yet,
//! recovered to the point asked for; or a backup checked without restoring
//! it. Each runs with no server, through a database given the plug-ins the
//! program ships, so a backup of an encrypted database is read with the
//! keys of `--keys`.

use std::io::Write;
use std::path::PathBuf;

use turborel::backup::seed::RecoveryTarget;
use turborel::db::Database;
use turborel::db::backup::RestoreOptions;
use turborel::log::Lsn;
use turborel::value::datetime::{Moment, parse, utc_of_unix};

use crate::{Config, Store, register};

/// \[EXTENSION\] The microseconds in a minute, of a timestamp's zone.
const MICROS_PER_MINUTE: i64 = 60_000_000;

/// What `--restore` or `--verify-backup` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupCommand {
    /// A backup restored.
    Restore {
        /// The backup's target.
        location: String,
        /// The backup's id, or empty for the newest.
        backup: String,
        /// The archive, if any.
        archive: Option<String>,
        /// Where recovery stops.
        target: RecoveryTarget,
    },
    /// A backup verified.
    Verify {
        /// The backup's target.
        location: String,
        /// The backup's id, or empty for the newest.
        backup: String,
    },
}

/// The instant `text` writes, a timestamp of UTC unless it writes its
/// zone, as microseconds from 1970-01-01 00:00:00 UTC.
///
/// # Errors
///
/// It writes no timestamp.
pub fn instant(text: &str) -> Result<i64, String> {
    let written =
        parse(Moment::Timestamp, text).ok_or_else(|| format!("--until-time takes a timestamp, YYYY-MM-DD HH:MM:SS, not {text}"))?;
    let zone = i64::from(written.zone.unwrap_or(0)).saturating_mul(MICROS_PER_MINUTE);
    Ok(written.local.saturating_sub(zone).saturating_sub(utc_of_unix(0)))
}

/// What follows `--restore` or `--verify-backup`, `option`, in `rest`.
///
/// # Errors
///
/// What is wrong with it, for the operator.
pub fn parse_backup(option: &str, rest: &[String]) -> Result<BackupCommand, String> {
    let mut words = rest.iter();
    let location = words.next().ok_or_else(|| format!("{option} needs a backup target"))?.clone();
    let (mut backup, mut archive, mut target) = (String::new(), None, RecoveryTarget::End);
    let restore = option == "--restore";
    while let Some(word) = words.next() {
        let value = words.next().ok_or_else(|| format!("{word} needs a value"))?;
        match (word.as_str(), restore) {
            ("--backup", _) => value.clone_into(&mut backup),
            ("--archive", true) => archive = Some(value.clone()),
            ("--until-time", true) => target = RecoveryTarget::Time(instant(value)?),
            ("--until-lsn", true) => target = RecoveryTarget::Position(position(value)?),
            ("--until-point", true) => target = RecoveryTarget::Point(value.to_uppercase()),
            (other, _) => return Err(format!("{other} is not an option of {option}")),
        }
    }
    if archive.is_none() && matches!(target, RecoveryTarget::Point(_) | RecoveryTarget::Time(_)) {
        return Err("--until-time and --until-point recover through the archive: name it with --archive".into());
    }
    Ok(if restore { BackupCommand::Restore { location, backup, archive, target } } else { BackupCommand::Verify { location, backup } })
}

/// The position `text` names, for `--until-lsn`.
fn position(text: &str) -> Result<Lsn, String> {
    text.parse().map(Lsn).map_err(|_| format!("--until-lsn takes a position, not {text}"))
}

/// Does `command` for `config`, telling `out` what was done.
///
/// # Errors
///
/// What failed, for the operator: a restore not into a directory, or into
/// one there already; a backup damaged or missing; a target not met.
pub fn run_backup(config: &Config, command: &BackupCommand, out: &mut dyn Write) -> Result<(), String> {
    let db = Database::new();
    let host_keys = register(&db, config)?;
    crate::hosted::start(&db, config, &host_keys, true)?;
    let written = |e: std::io::Error| format!("cannot write: {e}");
    match command {
        BackupCommand::Restore { location, backup, archive, target } => {
            let Store::Dir(dir) = &config.store else { return Err("--restore restores into a directory: name it with --dir".into()) };
            let options = RestoreOptions {
                dir: PathBuf::from(dir),
                location: location.clone(),
                backup: backup.clone(),
                archive: archive.clone(),
                target: target.clone(),
                timeline: None,
            };
            let done = db.restore(&options).map_err(|e| format!("cannot restore: {e}"))?;
            writeln!(
                out,
                "restored {} into {}, {} records replayed through {}, as timeline {}",
                done.backup,
                dir.display(),
                done.records,
                done.last.0,
                done.timeline
            )
            .map_err(written)
        }
        BackupCommand::Verify { location, backup } => {
            let done = db.verify_backup(location, backup).map_err(|e| format!("the backup is not sound: {e}"))?;
            writeln!(out, "the backup is sound: {} backups, {} chunks, {} records checked", done.backups, done.chunks, done.records)
                .map_err(written)
        }
    }
}
