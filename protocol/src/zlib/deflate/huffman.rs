//! \[EXTENSION\] Huffman codes for the deflater (RFC 1951 §3.2.2, §3.2.7,
//! RULES.md D193): code lengths limited to a longest code by
//! package-merge, which gives the optimal lengths under the limit; the
//! canonical codes of those lengths; and the run-length coding of a
//! dynamic block's code lengths.

use super::super::tables::{MAX_BITS, REPEAT_PREVIOUS, REPEAT_ZERO, REPEAT_ZERO_LONG};

/// An item of package-merge: its weight and the symbols it holds, each
/// once for each package it is in.
type Item = (u64, Vec<u16>);

/// The fewest symbols a code is given: a code of one symbol is complete
/// only for some readers, so a second is added.
const FEWEST_SYMBOLS: usize = 2;

/// The code lengths of `freqs`, none above `limit`: 0 for a symbol of no
/// frequency. At least two symbols get a code, so the code is complete.
pub(crate) fn lengths(freqs: &[u32], limit: usize) -> Vec<u8> {
    package_merge(&pad(freqs.to_vec()), limit)
}

/// Symbols of `freqs` used.
fn weights_used(freqs: &[u32]) -> usize {
    freqs.iter().filter(|f| **f > 0).count()
}

/// `weights` with zero-frequency symbols raised to 1 until two are used.
fn pad(mut weights: Vec<u32>) -> Vec<u32> {
    let mut used = weights_used(&weights);
    for w in &mut weights {
        if used >= FEWEST_SYMBOLS {
            break;
        }
        if *w == 0 {
            *w = 1;
            used = used.saturating_add(1);
        }
    }
    weights
}

/// Package-merge: the lengths minimising the sum of weight times length,
/// none above `limit`, of the weights over 0.
fn package_merge(weights: &[u32], limit: usize) -> Vec<u8> {
    let mut leaves: Vec<Item> =
        weights.iter().enumerate().filter(|(_, w)| **w > 0).map(|(s, w)| (u64::from(*w), vec![u16::try_from(s).unwrap_or(0)])).collect();
    leaves.sort_by_key(|(w, symbols)| (*w, symbols.first().copied().unwrap_or(0)));
    let mut current = leaves.clone();
    for _ in 1..limit {
        let packages: Vec<Item> =
            current.as_chunks::<2>().0.iter().map(|[a, b]| (a.0.saturating_add(b.0), a.1.iter().chain(&b.1).copied().collect())).collect();
        current = merge(&leaves, packages);
    }
    let mut lengths = vec![0_u8; weights.len()];
    let taken = leaves.len().saturating_mul(2).saturating_sub(2);
    for (_, symbols) in current.iter().take(taken) {
        for s in symbols {
            if let Some(l) = lengths.get_mut(usize::from(*s)) {
                *l = l.saturating_add(1);
            }
        }
    }
    lengths
}

/// `leaves` and `packages`, each sorted by weight, merged; a leaf before a
/// package of the same weight.
fn merge(leaves: &[Item], packages: Vec<Item>) -> Vec<Item> {
    let mut out = Vec::with_capacity(leaves.len().saturating_add(packages.len()));
    let mut leaves = leaves.iter().peekable();
    let mut packages = packages.into_iter().peekable();
    loop {
        let take_leaf = match (leaves.peek(), packages.peek()) {
            (Some(l), Some(p)) => l.0 <= p.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return out,
        };
        if take_leaf {
            out.extend(leaves.next().cloned());
        } else {
            out.extend(packages.next());
        }
    }
}

/// The canonical code of each symbol of `lengths` (§3.2.2), its bits
/// reversed, since a code is sent from its most significant bit and the
/// writer sends from the least.
pub(crate) fn codes(lengths: &[u8]) -> Vec<u16> {
    let mut count = [0_u16; MAX_BITS + 1];
    for l in lengths {
        if let Some(c) = count.get_mut(usize::from(*l)) {
            *c = c.saturating_add(1);
        }
    }
    let mut next = [0_u16; MAX_BITS + 1];
    let mut code = 0_u16;
    for bits in 1..=MAX_BITS {
        let before = count.get(bits.saturating_sub(1)).copied().filter(|_| bits > 1).unwrap_or(0);
        code = code.wrapping_add(before) << 1_u32;
        if let Some(n) = next.get_mut(bits) {
            *n = code;
        }
    }
    lengths
        .iter()
        .map(|l| {
            let Some(n) = next.get_mut(usize::from(*l)).filter(|_| *l != 0) else { return 0 };
            let c = *n;
            *n = n.wrapping_add(1);
            reverse(c, *l)
        })
        .collect()
}

/// The low `length` bits of `code`, reversed.
fn reverse(code: u16, length: u8) -> u16 {
    code.reverse_bits().checked_shr(u32::from(16_u8.saturating_sub(length))).unwrap_or(0)
}

/// A code length symbol, with its extra bits' value and count.
pub(crate) type Run = (u16, u16, u8);

/// The longest runs each repeat symbol says, and its least.
const PREVIOUS_RUN: (usize, usize) = (3, 6);
const ZERO_RUN: (usize, usize) = (3, 10);
const ZERO_LONG_RUN: (usize, usize) = (11, 138);
/// The extra bits of each repeat symbol.
const PREVIOUS_BITS: u8 = 2;
const ZERO_BITS: u8 = 3;
const ZERO_LONG_BITS: u8 = 7;

/// `lengths` run-length coded with the symbols 16, 17 and 18 (§3.2.7).
pub(crate) fn runs(lengths: &[u8]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut at = 0_usize;
    while let Some(&length) = lengths.get(at) {
        let same = lengths.iter().skip(at).take_while(|l| **l == length).count();
        let used = if length == 0 { zeros(same, &mut out) } else { repeats(length, same, &mut out) };
        at = at.saturating_add(used.max(1));
    }
    out
}

/// A run of `same` zero lengths, or its first part; the lengths it says.
fn zeros(same: usize, out: &mut Vec<Run>) -> usize {
    if same >= ZERO_LONG_RUN.0 {
        let n = same.min(ZERO_LONG_RUN.1);
        out.push((REPEAT_ZERO_LONG, extra(n, ZERO_LONG_RUN.0), ZERO_LONG_BITS));
        return n;
    }
    if same >= ZERO_RUN.0 {
        out.push((REPEAT_ZERO, extra(same, ZERO_RUN.0), ZERO_BITS));
        return same;
    }
    out.push((0, 0, 0));
    1
}

/// A run of `same` nonzero `length`s, or its first part: the length once,
/// then 16 for up to six more.
fn repeats(length: u8, same: usize, out: &mut Vec<Run>) -> usize {
    out.push((u16::from(length), 0, 0));
    let more = same.saturating_sub(1);
    if more < PREVIOUS_RUN.0 {
        return 1;
    }
    let n = more.min(PREVIOUS_RUN.1);
    out.push((REPEAT_PREVIOUS, extra(n, PREVIOUS_RUN.0), PREVIOUS_BITS));
    n.saturating_add(1)
}

/// A run's extra bits: its count less the least its symbol says.
fn extra(n: usize, least: usize) -> u16 {
    u16::try_from(n.saturating_sub(least)).unwrap_or(0)
}
