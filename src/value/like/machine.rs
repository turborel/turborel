//! \[DESIGN\] (RULES.md D1251) A `<pattern>` of no arbitrary character
//! specifier, `_`, as one machine over octets, so a value is matched an
//! octet at a time with nothing kept of it but a state, by whatever
//! reads its octets in order. A column store's scan steps it over texts
//! coded by a table of symbols, a code at a time, no text decoded
//! (`engine/columns/fsst/stepped.rs`).
//!
//! The machine is the pieces of `like/pieces.rs` one after another. A
//! value matches [X3.135-1986 §5.14 GR5] if it begins with the first
//! piece, holds the pieces between in order and apart, each where it is
//! first found, and ends with the last past them. So its states are,
//! in this order, the octets of the first piece matched so far, where
//! any other octet leaves no match; of each piece between, the octets
//! of it matched so far, as Knuth, Morris and Pratt, "Fast Pattern
//! Matching in Strings" (SIAM Journal on Computing, 1977), search for
//! a string, an octet that does not go on falling back to the longest
//! start of the piece the octets read end with; and of the last piece
//! the same, searched for to the value's end, where the value matches
//! if the state is the whole piece. A pattern that ends in `%` has an
//! accepting state no octet leaves in place of a last piece, and a
//! pattern of no `%` a state that any octet leaves for no match. A
//! piece found is found where it first ends, which leaves the most of
//! the value to the pieces after it, as the pieces' own matching does.
//!
//! A piece is whole characters and the encoding of a character never
//! begins inside another's, so the octets match as the characters do.

use super::LikePattern;
use super::finder::Finder;
use super::pieces::Pieces;

/// The values of an octet.
pub(crate) const OCTETS: usize = 256;

/// \[DESIGN\] The most states of a machine. A state is held in one
/// octet, so a table of the state after each state and octet is an
/// octet a place, and the one value past a machine's states is left to
/// whoever steps it over codes, for a code that is of no symbol. The
/// states are the octets of the pieces and two more, so a pattern
/// whose pieces are 253 octets together at most has a machine.
pub(crate) const MOST_STATES: usize = 255;

/// The state a value begins in.
pub(crate) const START: u8 = 0;

/// \[DESIGN\] (RULES.md D1251) A pattern's machine: the state after
/// each state reads each octet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The state after each state and octet, `OCTETS` to a state.
    after: Vec<u8>,
    /// The state a value that matches ends in.
    accept: u8,
    /// The first of the states no octet leaves.
    settled: u8,
}

/// The states of `piece` searched for, from `base`, written to `after`:
/// each state's row is that of the state the octets read fall back to,
/// but the piece's next octet goes on. If `stays`, the state of the
/// whole piece falls back likewise, as a last piece is searched for to
/// the value's end; else it is the state after, written by what follows.
fn searched(after: &mut [u8], base: usize, piece: &[u8], stays: bool) -> Option<()> {
    let row = |state: usize| state.checked_mul(OCTETS).and_then(|from| Some(from..from.checked_add(OCTETS)?));
    // The state the octets of the piece read past its first leave a search in.
    let mut back = base;
    for (matched, &octet) in piece.iter().enumerate() {
        let state = base.checked_add(matched)?;
        if matched == 0 {
            after.get_mut(row(state)?)?.fill(u8::try_from(base).ok()?);
        } else {
            after.copy_within(row(back)?, row(state)?.start);
        }
        let on = row(state)?.start.checked_add(usize::from(octet))?;
        *after.get_mut(on)? = u8::try_from(state.checked_add(1)?).ok()?;
        if matched > 0 {
            back = usize::from(*after.get(row(back)?.start.checked_add(usize::from(octet))?)?);
        }
    }
    if stays {
        after.copy_within(row(back)?, row(base.checked_add(piece.len())?)?.start);
    }
    Some(())
}

/// The states of the first piece, `first`, written to `after` from the
/// start: each octet of it leads on, and any other, as `after` holds
/// already, to no match. The state after them.
fn begun(after: &mut [u8], first: &[u8]) -> Option<usize> {
    for (at, &octet) in first.iter().enumerate() {
        *after.get_mut(at.checked_mul(OCTETS)?.checked_add(usize::from(octet))?)? = u8::try_from(at.checked_add(1)?).ok()?;
    }
    Some(first.len())
}

/// The states a pattern ends in, written to `after` from `at`, `last`
/// its last piece if it has a `%`: the state a value that matches ends
/// in, and the first of the states no octet leaves.
fn ended(after: &mut [u8], at: usize, last: Option<&[u8]>) -> Option<(usize, usize)> {
    match last.map(|piece| (piece, piece.is_empty())) {
        // Of no `%`, the value ends here, and any octet more is no match.
        None => Some((at, at.checked_add(1)?)),
        // A last `%` takes whatever follows.
        Some((_, true)) => {
            after.get_mut(at.checked_mul(OCTETS)?..)?.get_mut(..OCTETS)?.fill(u8::try_from(at).ok()?);
            Some((at, at))
        }
        Some((piece, false)) => {
            searched(after, at, piece, true)?;
            let whole = at.checked_add(piece.len())?;
            Some((whole, whole.checked_add(1)?))
        }
    }
}

impl Machine {
    /// The machine of `pieces`; none if its states are more than
    /// `MOST_STATES`.
    pub(super) fn of(pieces: &Pieces) -> Option<Machine> {
        let first = pieces.first.as_bytes();
        let last = pieces.last.as_deref().map(str::as_bytes);
        let between = pieces.between.iter().map(Finder::bytes);
        let octets = between.clone().chain([first, last.unwrap_or_default()]).try_fold(0_usize, |sum, p| sum.checked_add(p.len()))?;
        // The octets matched so far, a state to accept or to end in, and
        // the state of no match.
        let states = octets.checked_add(2).filter(|&n| n <= MOST_STATES)?;
        let dead = u8::try_from(states.checked_sub(1)?).ok()?;
        let mut after = vec![dead; states.checked_mul(OCTETS)?];
        let mut at = begun(&mut after, first)?;
        for piece in between {
            searched(&mut after, at, piece, false)?;
            at = at.checked_add(piece.len())?;
        }
        let (accept, settled) = ended(&mut after, at, last)?;
        Some(Machine { after, accept: u8::try_from(accept).ok()?, settled: u8::try_from(settled).ok()? })
    }

    /// How many states it has.
    pub(crate) fn states(&self) -> usize {
        self.after.len().checked_div(OCTETS).unwrap_or(0)
    }

    /// The state after `state` reads `octet`.
    pub(crate) fn step(&self, state: u8, octet: u8) -> u8 {
        let at = usize::from(state).saturating_mul(OCTETS).saturating_add(usize::from(octet));
        self.after.get(at).copied().unwrap_or(state)
    }

    /// The first of the states no octet leaves, those from it on: in
    /// one of them a value matches whatever follows or never does, and
    /// no more of it need be read.
    pub(crate) fn first_settled(&self) -> u8 {
        self.settled
    }

    /// The state a value that matches ends in.
    pub(crate) fn accept(&self) -> u8 {
        self.accept
    }

    /// [X3.135-1986 §5.14 GR5] Whether the value of the octets `value`
    /// matches the pattern, as a test steps the machine.
    #[cfg(test)]
    pub(crate) fn matches(&self, value: &[u8]) -> bool {
        let mut state = START;
        for &octet in value {
            if state >= self.settled {
                break;
            }
            state = self.step(state, octet);
        }
        state == self.accept
    }
}

impl LikePattern {
    /// \[DESIGN\] (RULES.md D1251) Its machine over octets; none of a
    /// pattern with `_`, which matches characters and not octets, or of
    /// one whose pieces are too long for a machine (`MOST_STATES`).
    pub(crate) fn machine(&self) -> Option<Machine> {
        Machine::of(self.pieces.as_ref()?)
    }
}

// Unit tests: the machine against the pieces' own matching (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::super::fixtures::{PATTERNS, strings};
    use super::*;

    /// Verifies: [X3.135-1986 §5.14 GR5] (RULES.md D1251) a pattern's
    /// machine matches every string of three octets, of seven at most,
    /// as its pieces do: a pattern of no `%`, a first and a last piece,
    /// which may not overlap, pieces between found where each first
    /// ends, a piece that begins again inside itself, found after a
    /// start of it that failed (the failure function), a last piece
    /// found more than once and only its last place at the end counted,
    /// and `%` alone.
    #[test]
    fn a_machine_matches_as_the_pieces_do() {
        let values = strings(b"abc", 7);
        assert_eq!(values.len(), 3280);
        for p in PATTERNS {
            let pattern = LikePattern::new(p, None).unwrap();
            let machine = pattern.machine().unwrap_or_else(|| panic!("{p:?} has a machine"));
            for v in &values {
                let text = std::str::from_utf8(v).unwrap();
                assert_eq!(machine.matches(v), pattern.matches(text), "{text:?} LIKE {p:?}");
            }
        }
    }

    /// Verifies: [X3.135-1986 §5.14 GR2a, GR5] (RULES.md D1251) an
    /// escape character that makes `%`, `_` or itself a character of a
    /// piece gives a machine of those octets; characters of several
    /// octets match by their octets, one never found inside another; a
    /// state no octet leaves is told, the accepting one of a pattern
    /// that ends in `%` and the one of no match, and a state of a last
    /// piece is not.
    #[test]
    fn a_machine_is_of_the_octets_of_its_pieces() {
        let escaped = LikePattern::new("%50!%!_!!%", Some('!')).unwrap().machine().unwrap();
        assert!(escaped.matches(b"at 50%_! off") && !escaped.matches(b"at 50%x! off") && !escaped.matches(b"at 500_! off"));
        let wide = LikePattern::new("%€%é", None).unwrap().machine().unwrap();
        assert!(wide.matches("x€yé".as_bytes()) && !wide.matches("x€yéz".as_bytes()) && !wide.matches("é€".as_bytes()));
        let ends_any = LikePattern::new("%ab%", None).unwrap().machine().unwrap();
        assert_eq!((ends_any.states(), ends_any.accept(), ends_any.first_settled()), (4, 2, 2));
        assert_eq!((ends_any.step(2, b'x'), ends_any.step(3, b'a')), (2, 3), "neither is left");
        let ends_piece = LikePattern::new("%ab", None).unwrap().machine().unwrap();
        assert_eq!((ends_piece.states(), ends_piece.accept(), ends_piece.first_settled()), (4, 2, 3));
        assert_eq!(ends_piece.step(2, b'a'), 1, "the whole piece is left by what follows it");
        let whole = LikePattern::new("ab", None).unwrap().machine().unwrap();
        assert_eq!((whole.accept(), whole.step(2, b'a'), whole.step(0, b'b')), (2, 3, 3), "an octet more, or another, is no match");
    }

    /// Verifies: [DESIGN] RULES.md D1251: a pattern with `_` has no
    /// machine, and one whose pieces are 254 octets together has none,
    /// its states more than an octet tells apart with one value left,
    /// while one of 253 has.
    #[test]
    fn a_pattern_of_an_arbitrary_character_or_too_long_has_no_machine() {
        for p in ["_", "a_b", "%a_%"] {
            assert_eq!(LikePattern::new(p, None).unwrap().machine(), None, "{p:?}");
        }
        let long = |pairs: usize, more: usize| LikePattern::new(&format!("%{}%", "ab".repeat(pairs) + &"c".repeat(more)), None).unwrap();
        let most = long(126, 1).machine().unwrap();
        assert_eq!(most.states(), MOST_STATES);
        assert!(most.matches(format!("x{}y", "ab".repeat(126) + "c").as_bytes()) && !most.matches("ab".repeat(126).as_bytes()));
        assert_eq!(long(127, 0).machine(), None);
        assert_eq!(
            LikePattern::new(&format!("{}%b%{}", "a".repeat(200), "c".repeat(53)), None).unwrap().machine(),
            None,
            "in three pieces"
        );
    }
}
