//! \[EXTENSION\] The settings of backups and the archive (RULES.md D161,
//! D163, D165): whether the log is archived, where, how long it is kept,
//! how much is gathered before a segment is archived and how long a record
//! waits at most, the timeline the database writes, and whether backups
//! are compressed.

use super::builtin::{text, whole};
use super::{Definition, Effect, Key, Kind, OFF, SWITCHES};

/// ARCHIVE.MODE.
pub const ARCHIVE_MODE: Key = Key(71);
/// ARCHIVE.TARGET.
pub const ARCHIVE_TARGET: Key = Key(72);
/// ARCHIVE.RETENTION_SECONDS.
pub const ARCHIVE_RETENTION_SECONDS: Key = Key(73);
/// ARCHIVE.SEGMENT_BYTES.
pub const ARCHIVE_SEGMENT_BYTES: Key = Key(74);
/// ARCHIVE.TIMEOUT_MS.
pub const ARCHIVE_TIMEOUT_MS: Key = Key(75);
/// ARCHIVE.TIMELINE.
pub const ARCHIVE_TIMELINE: Key = Key(76);
/// BACKUP.COMPRESSION.
pub const BACKUP_COMPRESSION: Key = Key(77);

/// \[EXTENSION\] The default of ARCHIVE.SEGMENT_BYTES: 16 MiB of records an
/// object, few objects for a busy log and little lost to a target's per
/// object cost.
pub const DEFAULT_SEGMENT_BYTES: i128 = 16 << 20;

/// \[EXTENSION\] The default of ARCHIVE.TIMEOUT_MS: a minute, the most the
/// archive lags the log.
pub const DEFAULT_TIMEOUT_MS: i128 = 60_000;

/// \[EXTENSION\] The longest location of a target ARCHIVE.TARGET holds.
pub const MAX_LOCATION: usize = 4096;

/// A database's setting, from the next statement.
const DATABASE: (bool, Effect) = (false, Effect::Statement);

/// The largest whole number a count setting holds.
const MOST: i128 = 9_223_372_036_854_775_807;

/// One setting.
fn defined((name, kind, default): (&str, Kind, crate::value::Value), doc: &str) -> Definition {
    let (per_session, effect) = DATABASE;
    Definition { name: name.to_owned(), kind, default, per_session, node: false, effect, doc: doc.to_owned() }
}

/// The settings of backups and the archive, in the order of their keys.
pub(super) fn definitions() -> [Definition; 7] {
    [
        defined(
            ("ARCHIVE.MODE", Kind::Choice(SWITCHES), text(OFF)),
            "Whether the log's records are archived to ARCHIVE.TARGET as they are made durable, for recovery to a point in time: ON; or not, OFF. While ON, a checkpoint keeps every record not yet archived.",
        ),
        defined(
            ("ARCHIVE.TARGET", Kind::Text { max: MAX_LOCATION }, text("")),
            "Where the log is archived: a directory's path, or a location of a target provider's scheme, as scheme:rest; none if empty.",
        ),
        defined(
            ("ARCHIVE.RETENTION_SECONDS", Kind::Integer { min: 0, max: MOST }, whole(0)),
            "The seconds an archived segment is kept, 0 for ever. An older one is removed once the newest backup no longer needs its records.",
        ),
        defined(
            ("ARCHIVE.SEGMENT_BYTES", Kind::Integer { min: 1, max: MOST }, whole(DEFAULT_SEGMENT_BYTES)),
            "The bytes of records gathered before they are archived as one segment.",
        ),
        defined(
            ("ARCHIVE.TIMEOUT_MS", Kind::Integer { min: 0, max: MOST }, whole(DEFAULT_TIMEOUT_MS)),
            "The most milliseconds a durable record waits to be archived, whatever ARCHIVE.SEGMENT_BYTES says; 0 archives every record as soon as the archiver runs.",
        ),
        defined(
            ("ARCHIVE.TIMELINE", Kind::Integer { min: 1, max: i128::from(u32::MAX) }, whole(1)),
            "The timeline the database's archived records are kept under: 1 for a database never restored, and a new one for each restore, set by the restore so it never writes over the archive of the database it was restored from.",
        ),
        defined(
            ("BACKUP.COMPRESSION", Kind::Choice(SWITCHES), text(OFF)),
            "Whether backups and archived segments pass through the LZ4 encoding, which must be registered, before the database's own encodings: ON; or not, OFF.",
        ),
    ]
}
