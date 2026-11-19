//! \[EXTENSION\] The eight table lock modes and which of them conflict
//! (RULES.md D186). A transaction may hold a table's lock in several modes
//! at once, as its statements took them, and a request waits only for
//! another transaction holding a mode that conflicts with it.
//!
//! The coordinator's own locks are three of the eight. A reader of a table
//! whose engine leaves concurrency to the coordinator holds ACCESS SHARE;
//! a read that intends to write, SHARE UPDATE EXCLUSIVE; a writer of such a
//! table, or a change to its definition, ACCESS EXCLUSIVE. A writer of a
//! table whose engine isolates transactions itself holds ROW EXCLUSIVE,
//! which other writers share, and a query locking such a table's rows,
//! ROW SHARE. The rest LOCK TABLE alone takes.

/// The table lock modes, the modes a lock is held in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Held by readers of a table whose engine leaves concurrency to the
    /// coordinator; conflicts with ACCESS EXCLUSIVE alone.
    AccessShare,
    /// \[EXTENSION\] Held by a query locking rows of a table whose engine
    /// keeps row locks.
    RowShare,
    /// \[EXTENSION\] Held by writers of a table whose engine isolates
    /// transactions itself; any number may hold it together.
    RowExclusive,
    /// \[DESIGN\] Held by a read that intends to write what it reads: by a
    /// searched UPDATE or DELETE on its table, by an updatable cursor, and
    /// by FOR UPDATE of a table locked whole. One transaction may hold it,
    /// beside any number of readers; so two such reads of a table take
    /// turns instead of deadlocking when each goes on to write.
    ShareUpdateExclusive,
    /// \[EXTENSION\] Held by FOR SHARE of a table locked whole: excludes
    /// writers, not other holders of SHARE.
    Share,
    /// \[EXTENSION\] SHARE that one transaction alone may hold.
    ShareRowExclusive,
    /// \[EXTENSION\] Excludes every mode but ACCESS SHARE.
    Exclusive,
    /// Held by one writer of a table locked whole, or a change to a table's
    /// definition, alone.
    AccessExclusive,
}

/// Every mode, in the order of `LockMode`.
pub(crate) const MODES: [LockMode; 8] = [
    LockMode::AccessShare,
    LockMode::RowShare,
    LockMode::RowExclusive,
    LockMode::ShareUpdateExclusive,
    LockMode::Share,
    LockMode::ShareRowExclusive,
    LockMode::Exclusive,
    LockMode::AccessExclusive,
];

/// A set of modes, a bit for each.
pub(crate) type Modes = u8;

impl LockMode {
    /// Its bit in a set of modes.
    pub(crate) fn bit(self) -> Modes {
        match self {
            LockMode::AccessShare => 1,
            LockMode::RowShare => 1 << 1,
            LockMode::RowExclusive => 1 << 2,
            LockMode::ShareUpdateExclusive => 1 << 3,
            LockMode::Share => 1 << 4,
            LockMode::ShareRowExclusive => 1 << 5,
            LockMode::Exclusive => 1 << 6,
            LockMode::AccessExclusive => 1 << 7,
        }
    }

    /// The modes that conflict with it, as a set.
    fn conflicting(self) -> Modes {
        // Bits, from ACCESS SHARE at bit 0 to ACCESS EXCLUSIVE at bit 7.
        match self {
            LockMode::AccessShare => 0b1000_0000,
            LockMode::RowShare => 0b1100_0000,
            LockMode::RowExclusive => 0b1111_0000,
            LockMode::ShareUpdateExclusive => 0b1111_1000,
            LockMode::Share => 0b1110_1100,
            LockMode::ShareRowExclusive => 0b1111_1100,
            LockMode::Exclusive => 0b1111_1110,
            LockMode::AccessExclusive => 0b1111_1111,
        }
    }

    /// Whether a request in it conflicts with a holder of `held`.
    pub(crate) fn conflicts_with(self, held: Modes) -> bool {
        self.conflicting() & held != 0
    }

    /// Its name, as LOCK TABLE writes it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            LockMode::AccessShare => "ACCESS SHARE",
            LockMode::RowShare => "ROW SHARE",
            LockMode::RowExclusive => "ROW EXCLUSIVE",
            LockMode::ShareUpdateExclusive => "SHARE UPDATE EXCLUSIVE",
            LockMode::Share => "SHARE",
            LockMode::ShareRowExclusive => "SHARE ROW EXCLUSIVE",
            LockMode::Exclusive => "EXCLUSIVE",
            LockMode::AccessExclusive => "ACCESS EXCLUSIVE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MODES;

    /// Verifies: [EXTENSION] RULES.md D186, the conflicts are symmetric,
    /// a request in one mode conflicting with a holder of another exactly as
    /// the other's request conflicts with a holder of it.
    #[test]
    fn conflicts_are_symmetric() {
        for a in MODES {
            for b in MODES {
                assert_eq!(a.conflicts_with(b.bit()), b.conflicts_with(a.bit()), "{a:?} and {b:?}");
            }
        }
    }

    /// Verifies: [EXTENSION] RULES.md D186, the eight modes' conflicts,
    /// row by row, 'x' a conflict.
    #[test]
    fn conflicts_are_the_matrix() {
        let matrix = [".......x", "......xx", "....xxxx", "...xxxxx", "..xx.xxx", "..xxxxxx", ".xxxxxxx", "xxxxxxxx"];
        for (a, row) in MODES.iter().zip(matrix) {
            for (b, cell) in MODES.iter().zip(row.chars()) {
                assert_eq!(a.conflicts_with(b.bit()), cell == 'x', "{a:?} and {b:?}");
            }
        }
    }
}
