//! \[EXTENSION\] The decisions of backups and the archive of two or more
//! conditions (RULES.md D160 to D166), each a function of its conditions
//! alone, so its MC/DC cases are written out beside it (RULES.md R-TEST-8).
//! Each test names the pair of cases that shows a condition decides.

use std::time::Duration;

use crate::log::Lsn;

/// \[EXTENSION\] Whether a backup's record at `at`, after the record at
/// `last` if any, is in order and within the backup's positions, from
/// `from` up to `stop` (RULES.md D161).
pub fn record_in_order(last: Option<Lsn>, at: Lsn, (from, stop): (Lsn, Lsn)) -> bool {
    last.is_none_or(|l| at > l) && at >= from && at < stop
}

/// \[EXTENSION\] Whether a chunk read is the one its manifest lists, of
/// the digest `listed` and the length `expected` (RULES.md D161).
pub fn chunk_matches<D: PartialEq>((listed, found): (Option<D>, D), (length, expected): (u64, u64)) -> bool {
    listed == Some(found) && length == expected
}

/// \[EXTENSION\] Whether an archived segment is removed, archived at
/// `archived` (if the history says when), before `cutoff`, and its last
/// record before `needed`, the newest backup's start (RULES.md D163).
pub fn segment_expired(archived: Option<i64>, cutoff: i64, (last, needed): (Lsn, Lsn)) -> bool {
    archived.is_some_and(|wall| wall < cutoff) && last < needed
}

/// \[EXTENSION\] Whether records gathered short of a segment's bytes are
/// archived now, `forced` or having `waited` ARCHIVE.TIMEOUT_MS, `timeout`
/// (RULES.md D163).
pub fn waited_enough(forced: bool, waited: Duration, timeout: Duration) -> bool {
    forced || waited >= timeout
}

/// \[EXTENSION\] Whether the log is archived, ARCHIVE.MODE `on` and no
/// transaction `held` across the database (RULES.md D163).
pub fn archiving((on, held): (bool, bool)) -> bool {
    on && !held
}

/// \[EXTENSION\] Whether a seed's record at `at` is served, none served
/// yet (`served` 0) or past the last served, `last` (RULES.md D164).
pub fn serves(served: u64, at: Lsn, last: Lsn) -> bool {
    served == 0 || at > last
}

/// \[EXTENSION\] Whether a restore is refused, the database having files
/// open or log storage set (RULES.md D162).
pub fn restore_refused((files, log_storage): (bool, bool)) -> bool {
    files || log_storage
}

/// \[EXTENSION\] Whether a timeline's history is read, not the first
/// timeline and its history `listed` in the archive (RULES.md D165).
pub fn has_history((first, listed): (bool, bool)) -> bool {
    !first && listed
}

/// \[EXTENSION\] Whether a rescan marks a segment removed, `recorded` in
/// the history and not `found` in the target (RULES.md D166).
pub fn went((recorded, found): (bool, bool)) -> bool {
    recorded && !found
}

/// \[EXTENSION\] Whether the history is left unread, not to be `made` and
/// its file not `there` (RULES.md D166).
pub fn history_absent((made, there): (bool, bool)) -> bool {
    !made && !there
}

/// \[EXTENSION\] Whether a history file held is the one asked for, of the
/// `same_fs` and at the `same_path` (RULES.md D166).
pub fn same_history((same_fs, same_path): (bool, bool)) -> bool {
    same_fs && same_path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D161, R-TEST-8: a record is in
    /// order past the last (pair 1, 2), at or past the start (1, 3) and
    /// before the stop (1, 4); with none before, the first condition holds.
    #[test]
    fn record_order_shows_mcdc() {
        let span = (Lsn(10), Lsn(20));
        assert!(record_in_order(Some(Lsn(11)), Lsn(12), span), "1");
        assert!(!record_in_order(Some(Lsn(12)), Lsn(12), span), "2");
        assert!(!record_in_order(Some(Lsn(5)), Lsn(9), span), "3");
        assert!(!record_in_order(Some(Lsn(11)), Lsn(20), span), "4");
        assert!(record_in_order(None, Lsn(10), span), "none before");
    }

    /// Verifies: [EXTENSION] RULES.md D161, R-TEST-8: a chunk matches by
    /// its digest (pair 1, 2) and its length (1, 3); a chunk not listed
    /// never does.
    #[test]
    fn chunk_match_shows_mcdc() {
        assert!(chunk_matches((Some(7_u8), 7_u8), (100, 100)), "1");
        assert!(!chunk_matches((Some(8_u8), 7_u8), (100, 100)), "2");
        assert!(!chunk_matches((Some(7_u8), 7_u8), (99, 100)), "3");
        assert!(!chunk_matches((None, 7_u8), (100, 100)), "not listed");
    }

    /// Verifies: [EXTENSION] RULES.md D163, R-TEST-8: a segment is removed
    /// once old (pair 1, 2) and not needed (1, 3); one the history has no
    /// time for is kept.
    #[test]
    fn retention_shows_mcdc() {
        assert!(segment_expired(Some(5), 10, (Lsn(3), Lsn(4))), "1");
        assert!(!segment_expired(Some(10), 10, (Lsn(3), Lsn(4))), "2");
        assert!(!segment_expired(Some(5), 10, (Lsn(4), Lsn(4))), "3");
        assert!(!segment_expired(None, 10, (Lsn(3), Lsn(4))), "no time");
    }

    /// Verifies: [EXTENSION] RULES.md D163, R-TEST-8: records are archived
    /// short of a segment when forced (pair 1, 3) or waited long enough
    /// (2, 3).
    #[test]
    fn waiting_shows_mcdc() {
        let timeout = Duration::from_millis(100);
        assert!(waited_enough(true, Duration::ZERO, timeout), "1");
        assert!(waited_enough(false, timeout, timeout), "2");
        assert!(!waited_enough(false, Duration::from_millis(99), timeout), "3");
    }

    /// Verifies: [EXTENSION] RULES.md D163, R-TEST-8: the log is archived
    /// with ARCHIVE.MODE ON (pair 1, 2) and no transaction held (1, 3).
    #[test]
    fn archiving_shows_mcdc() {
        assert!(archiving((true, false)), "1");
        assert!(!archiving((false, false)), "2");
        assert!(!archiving((true, true)), "3");
    }

    /// Verifies: [EXTENSION] RULES.md D164, R-TEST-8: a seed's record is
    /// served as the first (pair 1, 3) or past the last served (2, 3).
    #[test]
    fn serving_shows_mcdc() {
        assert!(serves(0, Lsn(5), Lsn(9)), "1");
        assert!(serves(1, Lsn(10), Lsn(9)), "2");
        assert!(!serves(1, Lsn(9), Lsn(9)), "3");
    }

    /// Verifies: [EXTENSION] RULES.md D162, R-TEST-8: a restore is refused
    /// by files open (pair 1, 3) or log storage set (2, 3).
    #[test]
    fn restore_refusal_shows_mcdc() {
        assert!(restore_refused((true, false)), "1");
        assert!(restore_refused((false, true)), "2");
        assert!(!restore_refused((false, false)), "3");
    }

    /// Verifies: [EXTENSION] RULES.md D165, D166, R-TEST-8: a history is
    /// read past the first timeline (pair 1, 2) and listed (1, 3); a
    /// segment went if recorded (4, 5) and not found (4, 6); the history
    /// is absent unless made (7, 8) or there (7, 9); a held one is the one
    /// asked for by its file system (10, 11) and path (10, 12).
    #[test]
    fn history_decisions_show_mcdc() {
        assert!(has_history((false, true)), "1");
        assert!(!has_history((true, true)), "2");
        assert!(!has_history((false, false)), "3");
        assert!(went((true, false)), "4");
        assert!(!went((false, false)), "5");
        assert!(!went((true, true)), "6");
        assert!(history_absent((false, false)), "7");
        assert!(!history_absent((true, false)), "8");
        assert!(!history_absent((false, true)), "9");
        assert!(same_history((true, true)), "10");
        assert!(!same_history((false, true)), "11");
        assert!(!same_history((true, false)), "12");
    }

    /// Verifies: [EXTENSION] RULES.md D166, R-TEST-8: a location names a
    /// scheme of two characters or more (pair 1, 2), each a letter, digit,
    /// `+`, `-` or `.` (1, 3).
    #[test]
    fn scheme_shows_mcdc() {
        assert_eq!(super::super::scheme_of("dir:/a"), Some("dir"), "1");
        assert_eq!(super::super::scheme_of("d:/a"), None, "2");
        assert_eq!(super::super::scheme_of("d_r:/a"), None, "3");
    }
}
