//! \[EXTENSION\] LZ77 with hash chains (RFC 1951 §4, RULES.md D193): each
//! position's first three bytes are hashed, each hash heads a chain of
//! the earlier positions with it, and the longest match among the last
//! `CHAIN` of them within the window is taken, greedily.
//!
//! The tables are sized to the input, no larger than the window, so a
//! short packet costs little, and the whole input is one window's reach:
//! no state is kept from one call to the next.

use super::super::tables::{MAX_MATCH, MIN_MATCH, WINDOW};

/// The most earlier positions a match is looked for at.
const CHAIN: usize = 32;
/// The most and fewest bits of a hash.
const MOST_HASH_BITS: u32 = 15;
const FEWEST_HASH_BITS: u32 = 8;
/// The bytes a position's hash is of: the shortest match.
const HASHED: usize = 3;
/// A chain's end: no earlier position.
const NONE: usize = usize::MAX;

/// A literal byte, or a match of `length` bytes `distance` back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    /// A byte as it is.
    Literal(u8),
    /// A copy of earlier bytes.
    Match {
        /// Its length, 3 to 258.
        length: u16,
        /// How far back it begins, 1 to 32768.
        distance: u16,
    },
}

impl Token {
    /// The input bytes it stands for.
    pub(crate) fn span(self) -> usize {
        match self {
            Token::Literal(_) => 1,
            Token::Match { length, .. } => usize::from(length),
        }
    }
}

/// The hash heads and chains of the positions seen.
struct Chains {
    head: Vec<usize>,
    prev: Vec<usize>,
    hash_mask: usize,
    window_mask: usize,
}

impl Chains {
    fn new(len: usize) -> Chains {
        let bits = usize::BITS.saturating_sub(len.leading_zeros()).clamp(FEWEST_HASH_BITS, MOST_HASH_BITS);
        let hash_size = 1_usize << bits;
        let window = len.next_power_of_two().clamp(1, WINDOW);
        Chains {
            head: vec![NONE; hash_size],
            prev: vec![NONE; window],
            hash_mask: hash_size.wrapping_sub(1),
            window_mask: window.wrapping_sub(1),
        }
    }

    fn hash(&self, data: &[u8], at: usize) -> Option<usize> {
        // XXH3-64 of the three bytes (RULES.md D460), its low bits the head.
        let bytes = data.get(at..at.checked_add(HASHED)?)?;
        let hash = usize::try_from(turborel::hash::xxhash::xxh3_64(bytes) & u64::from(u32::MAX)).ok()?;
        Some(hash & self.hash_mask)
    }

    /// Position `at` added to its hash's chain.
    fn insert(&mut self, data: &[u8], at: usize) {
        let Some(h) = self.hash(data, at) else { return };
        let Some(head) = self.head.get_mut(h) else { return };
        let earlier = *head;
        *head = at;
        if let Some(slot) = self.prev.get_mut(at & self.window_mask) {
            *slot = earlier;
        }
    }

    /// The longest match for position `at`, its length and distance.
    fn longest(&self, data: &[u8], at: usize) -> (usize, usize) {
        let Some(h) = self.hash(data, at) else { return (0, 0) };
        let mut candidate = self.head.get(h).copied().unwrap_or(NONE);
        let (mut best, mut distance) = (0_usize, 0_usize);
        for _ in 0..CHAIN {
            let Some(back) = at.checked_sub(candidate).filter(|b| *b > 0 && *b <= WINDOW) else { break };
            let len = common(data, candidate, at);
            if len > best {
                (best, distance) = (len, back);
            }
            if best >= MAX_MATCH {
                break;
            }
            let next = self.prev.get(candidate & self.window_mask).copied().unwrap_or(NONE);
            if next >= candidate {
                break;
            }
            candidate = next;
        }
        (best, distance)
    }
}

/// The bytes positions `from` and `at` have in common, at most a match's
/// longest.
fn common(data: &[u8], from: usize, at: usize) -> usize {
    let (Some(a), Some(b)) = (data.get(from..), data.get(at..)) else { return 0 };
    a.iter().zip(b).take(MAX_MATCH).take_while(|(x, y)| x == y).count()
}

/// `data` as literals and matches.
pub(crate) fn tokens(data: &[u8]) -> Vec<Token> {
    let mut chains = Chains::new(data.len());
    let mut out = Vec::with_capacity(data.len().checked_div(2).unwrap_or(0));
    let mut at = 0_usize;
    while let Some(&byte) = data.get(at) {
        let (length, distance) = chains.longest(data, at);
        let mut token = Token::Literal(byte);
        if length >= MIN_MATCH
            && let (Ok(length), Ok(distance)) = (u16::try_from(length), u16::try_from(distance))
        {
            token = Token::Match { length, distance };
        }
        for p in at..at.saturating_add(token.span()) {
            chains.insert(data, p);
        }
        at = at.saturating_add(token.span());
        out.push(token);
    }
    out
}
