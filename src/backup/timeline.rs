//! \[EXTENSION\] The archive's layout, version 1, and its timelines
//! (RULES.md D163, D165). Each timeline's records are archived under a
//! prefix of its own, a segment an object named by its first and last
//! positions, so a timeline begun by a restore never writes over the
//! segments of the one it branched from, though positions after the branch
//! repeat. A timeline but the first has a history object naming its
//! ancestors, each with the last position of its that the timeline's
//! records follow.
//!
//! ```text
//! archive/<timeline, 8 digits>/<first, 20 digits>-<last, 20 digits>
//! archive/<timeline, 8 digits>.history
//! history:  format u8 (ARCHIVE_FORMAT) | timeline u32 |
//!           ancestors u32, each: timeline u32 | branch u64
//! ```

use std::sync::Arc;

use super::object::{ObjectIn, Reading, Writing, write_one};
use super::target::BackupTarget;
use super::{ARCHIVE, HISTORY_SUFFIX};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;

/// \[EXTENSION\] The format of the archive's layout and history objects
/// this code writes and reads.
pub const ARCHIVE_FORMAT: u8 = 1;

/// \[EXTENSION\] The first timeline, of a database never restored.
pub const FIRST_TIMELINE: u32 = 1;

/// \[EXTENSION\] A segment of the archive: its timeline, the positions of
/// its first and last records, and its object's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Its timeline.
    pub timeline: u32,
    /// Its first record's position.
    pub first: Lsn,
    /// Its last record's position.
    pub last: Lsn,
    /// Its object.
    pub name: String,
}

/// The prefix of timeline `timeline`'s segments.
pub fn timeline_prefix(timeline: u32) -> String {
    format!("{ARCHIVE}{timeline:08}/")
}

/// The object of timeline `timeline`'s history.
pub fn history_name(timeline: u32) -> String {
    format!("{ARCHIVE}{timeline:08}{HISTORY_SUFFIX}")
}

/// The object of a segment of `timeline` from `first` to `last`.
pub fn segment_name(timeline: u32, first: Lsn, last: Lsn) -> String {
    format!("{}{:020}-{:020}", timeline_prefix(timeline), first.0, last.0)
}

/// The segment object `name` is, of timeline `timeline`, if it is one.
fn segment_of(timeline: u32, name: &str) -> Option<Segment> {
    let rest = name.strip_prefix(&timeline_prefix(timeline))?;
    let (first, last) = rest.split_once('-')?;
    let (first, last) = (Lsn(first.parse().ok()?), Lsn(last.parse().ok()?));
    Some(Segment { timeline, first, last, name: name.to_owned() })
}

/// \[EXTENSION\] The segments of timeline `timeline` in `target`, in
/// position order.
///
/// # Errors
///
/// The target's.
pub fn segments(target: &dyn BackupTarget, timeline: u32) -> Result<Vec<Segment>> {
    let mut found: Vec<Segment> = target.list(&timeline_prefix(timeline))?.iter().filter_map(|n| segment_of(timeline, n)).collect();
    found.sort_by_key(|s| s.first);
    Ok(found)
}

/// \[EXTENSION\] The highest timeline `target`'s archive has segments or a
/// history of, if any.
///
/// # Errors
///
/// The target's.
pub fn highest(target: &dyn BackupTarget) -> Result<Option<u32>> {
    let names = target.list(ARCHIVE)?;
    let timeline = |n: &String| n.strip_prefix(ARCHIVE)?.get(..8)?.parse::<u32>().ok();
    Ok(names.iter().filter_map(timeline).max())
}

/// \[EXTENSION\] A timeline's ancestry: each ancestor, oldest first, with
/// the last position of its that the next follows.
pub type Ancestry = Vec<(u32, Lsn)>;

/// \[EXTENSION\] The ancestry of `timeline` in `target`: none for the
/// first, or a timeline without a history object.
///
/// # Errors
///
/// SQLCODE -916 (SQLSTATE TBB01) if its history is damaged; the target's.
pub fn ancestry(target: &Arc<dyn BackupTarget>, timeline: u32, reading: &Reading) -> Result<Ancestry> {
    let name = history_name(timeline);
    if !super::decide::has_history((timeline == FIRST_TIMELINE, target.list(&name)?.contains(&name))) {
        return Ok(Vec::new());
    }
    let bytes = ObjectIn::only(target, &name, reading)?;
    decode(&bytes).map_err(|e| super::damaged(&name, &e.msg))
}

/// The ancestry `bytes` encode.
fn decode(bytes: &[u8]) -> Result<Ancestry> {
    let mut r = Reader::new(bytes, "a timeline's history");
    if r.u8()? != ARCHIVE_FORMAT {
        return Err(SqlError::new(Code::Corrupt, "[EXTENSION] RULES.md D165: a history of an unknown format"));
    }
    let _timeline = r.u32()?;
    let mut ancestors = Vec::new();
    for _ in 0..r.u32()? {
        ancestors.push((r.u32()?, Lsn(r.u64()?)));
    }
    r.finish()?;
    Ok(ancestors)
}

/// \[EXTENSION\] Writes the history of timeline `timeline`, which follows
/// `ancestors`, to `target`.
///
/// # Errors
///
/// SQLCODE -910 if the target holds one already (objects are never
/// replaced); the target's.
pub fn write_history(target: &dyn BackupTarget, (timeline, ancestors): (u32, &Ancestry), writing: &Writing) -> Result<()> {
    let mut out = vec![ARCHIVE_FORMAT];
    out.put_u32(timeline);
    out.put_u32(u32::try_from(ancestors.len()).unwrap_or(u32::MAX));
    for (ancestor, branch) in ancestors {
        out.put_u32(*ancestor);
        out.put_u64(branch.0);
    }
    write_one(target, &history_name(timeline), writing, &out).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D163, the archive's names: a
    /// segment's object sorts by timeline and position, and is read back.
    #[test]
    fn segment_names_round_trip() {
        let name = segment_name(2, Lsn(10), Lsn(19));
        assert_eq!(name, "archive/00000002/00000000000000000010-00000000000000000019");
        assert_eq!(segment_of(2, &name).map(|s| (s.first, s.last)), Some((Lsn(10), Lsn(19))));
        assert_eq!(segment_of(3, &name), None);
        assert_eq!(history_name(3), "archive/00000003.history");
        let mut bytes = vec![ARCHIVE_FORMAT];
        bytes.put_u32(3);
        bytes.put_u32(1);
        bytes.put_u32(1);
        bytes.put_u64(7);
        assert_eq!(decode(&bytes).unwrap(), vec![(1, Lsn(7))]);
    }
}
