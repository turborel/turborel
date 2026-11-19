//! \[EXTENSION\] The records recovery to a point stops at (RULES.md D164):
//! a commit, with the time it committed by the database's clock, and a
//! restore point, with its name. Neither is ever sealed (`log/sealed.rs`),
//! as neither holds a row, so a restore reads them from a record's bytes
//! without the database's types or keys (`commit_wall`, `point_name`).

use super::kinds::{COMMIT, RESTORE_POINT};
use super::{Change, RECORD_VERSION};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::txn::Timestamp;

/// Where a record's body begins: after its version, kind and transaction.
const BODY: usize = 10;

/// Writes a commit's body: its timestamp and wall time.
pub(super) fn put_commit(out: &mut Vec<u8>, at: Timestamp, wall: i64) {
    out.put_u64(at);
    out.put_i128(i128::from(wall));
}

/// A commit, as `put_commit` wrote it.
pub(super) fn commit(r: &mut Reader<'_>) -> Result<Change> {
    let at = r.u64()?;
    Ok(Change::Commit { at, wall: wall(r)? })
}

/// Writes a restore point's body: its name and wall time.
pub(super) fn put_point(out: &mut Vec<u8>, name: &str, wall: i64) {
    out.put_str(name);
    out.put_i128(i128::from(wall));
}

/// A restore point, as `put_point` wrote it.
pub(super) fn point(r: &mut Reader<'_>) -> Result<Change> {
    let name = r.str()?;
    Ok(Change::RestorePoint { name, wall: wall(r)? })
}

/// A wall time, microseconds from 1970-01-01 00:00 UTC.
fn wall(r: &mut Reader<'_>) -> Result<i64> {
    i64::try_from(r.i128()?).map_err(|_| r.corrupt("a wall time is out of range"))
}

/// The body of `bytes` if they are a record of kind `kind`.
fn body(bytes: &[u8], kind: u8) -> Option<Reader<'_>> {
    let is = bytes.first() == Some(&RECORD_VERSION) && bytes.get(1) == Some(&kind);
    is.then(|| Reader::new(bytes.get(BODY..).unwrap_or_default(), "log record"))
}

/// \[EXTENSION\] When the commit `bytes` record committed, if they are a
/// commit's record.
pub fn commit_wall(bytes: &[u8]) -> Option<i64> {
    let mut r = body(bytes, COMMIT)?;
    r.u64().ok()?;
    wall(&mut r).ok()
}

/// \[EXTENSION\] The name of the restore point `bytes` record, if they are
/// a restore point's record.
pub fn point_name(bytes: &[u8]) -> Option<String> {
    body(bytes, RESTORE_POINT)?.str().ok()
}

#[cfg(test)]
mod tests {
    use super::super::Record;
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D164, the records recovery to a
    /// point reads without decoding a record whole: a commit's wall time,
    /// and a restore point's name; none of another kind.
    #[test]
    fn commits_and_points_are_read_from_bytes() {
        let commit = Record { tx: 3, change: Change::Commit { at: 9, wall: -5 } }.encode();
        let point = Record { tx: 0, change: Change::RestorePoint { name: "P".into(), wall: 4 } }.encode();
        assert_eq!(commit_wall(&commit), Some(-5));
        assert_eq!(point_name(&point), Some("P".to_owned()));
        assert_eq!(commit_wall(&point), None);
        assert_eq!(point_name(&commit), None);
        assert_eq!(commit_wall(&[]), None);
    }
}
