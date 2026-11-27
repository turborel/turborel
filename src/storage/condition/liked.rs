//! \[DESIGN\] (RULES.md D1251) A LIKE as a scan is told it, with the
//! constant pattern made ready to match a text's octets, whether NOT was
//! written, and the pattern's machine over octets
//! (`value/like/machine.rs`), made once for the statement, which a scan
//! steps over texts it holds coded, none decoded.

use crate::value::like::machine::Machine;
use crate::value::{BytesLike, LikePattern};

/// [X3.135-1986 §5.14] (RULES.md D1251) `LIKE pattern`, or NOT LIKE,
/// of a constant pattern of no `_`, as a scan is told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Liked {
    /// The pattern, matched of a text's octets by its pieces.
    pattern: BytesLike,
    /// NOT LIKE was written.
    not: bool,
    /// The pattern's machine.
    machine: Machine,
}

impl Liked {
    /// `LIKE pattern`, or NOT LIKE if `not`: none of a pattern that has
    /// no machine, one with `_` or of pieces too long
    /// (`LikePattern::machine`), which a scan is not told.
    pub fn of(pattern: &LikePattern, not: bool) -> Option<Liked> {
        Some(Liked { pattern: pattern.bytes()?, not, machine: pattern.machine()? })
    }

    /// [X3.135-1986 §5.14 GR5; §5.18] Whether the predicate is true of
    /// the character string of the octets `text`, which matches the
    /// pattern or, of NOT LIKE, does not.
    pub fn keeps(&self, text: &[u8]) -> bool {
        self.pattern.matches(text) != self.not
    }

    /// Whether NOT LIKE was written.
    pub fn not(&self) -> bool {
        self.not
    }

    /// The pattern's machine over octets.
    pub(crate) fn machine(&self) -> &Machine {
        &self.machine
    }
}
