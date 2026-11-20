//! \[DESIGN\] Selections (RULES.md D354): the rows of a vector a predicate
//! is true of, written branch-free, as Sompolski, Zukowski and Boncz,
//! "Vectorization vs. Compilation in Query Execution" (2011), §3, write
//! them: each row's place written to the next slot, the count of slots
//! moved on by the predicate's truth, so no branch is mispredicted at a
//! middle selectivity. The operator is matched once a call, each loop
//! made for its own by a generic over `Holds`. A null's bit, 64 rows a
//! word, takes its row out.

use crate::kernels::Given;
use crate::storage::Mask;
use crate::storage::mask::{WORD_BITS, index};
use crate::value::CompOp;

/// A comparison of `v` with a constant, as a kernel's loop makes it.
trait Holds<T: Copy> {
    /// Whether `v op c` is true.
    fn holds(v: T, c: T) -> bool;
}

/// `=`.
struct Eq;
/// `<>`.
struct Ne;
/// `<`.
struct Lt;
/// `>`.
struct Gt;
/// `<=`.
struct Le;
/// `>=`.
struct Ge;

impl Holds<i64> for Eq {
    fn holds(v: i64, c: i64) -> bool {
        v == c
    }
}
impl Holds<i64> for Ne {
    fn holds(v: i64, c: i64) -> bool {
        v != c
    }
}
impl Holds<i64> for Lt {
    fn holds(v: i64, c: i64) -> bool {
        v < c
    }
}
impl Holds<i64> for Gt {
    fn holds(v: i64, c: i64) -> bool {
        v > c
    }
}
impl Holds<i64> for Le {
    fn holds(v: i64, c: i64) -> bool {
        v <= c
    }
}
impl Holds<i64> for Ge {
    fn holds(v: i64, c: i64) -> bool {
        v >= c
    }
}

// Approximate numbers as `partial_cmp` orders them (R-NUM-4): no NaN is
// equal, unequal, less or greater than any number, -0.0 equals 0.0.
impl Holds<f64> for Eq {
    fn holds(v: f64, c: f64) -> bool {
        v.partial_cmp(&c).is_some_and(std::cmp::Ordering::is_eq)
    }
}
impl Holds<f64> for Ne {
    fn holds(v: f64, c: f64) -> bool {
        v.partial_cmp(&c).is_some_and(std::cmp::Ordering::is_ne)
    }
}
impl Holds<f64> for Lt {
    fn holds(v: f64, c: f64) -> bool {
        v < c
    }
}
impl Holds<f64> for Gt {
    fn holds(v: f64, c: f64) -> bool {
        v > c
    }
}
impl Holds<f64> for Le {
    fn holds(v: f64, c: f64) -> bool {
        v <= c
    }
}
impl Holds<f64> for Ge {
    fn holds(v: f64, c: f64) -> bool {
        v >= c
    }
}

/// The rows of `given`, or of all `values`, not null, whose value `v`
/// satisfies `v op c`, appended to `out`, ascending.
pub fn select_i64(values: &[i64], (op, c): (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
    match op {
        CompOp::Eq => selected::<i64, Eq>(values, c, given, out),
        CompOp::Ne => selected::<i64, Ne>(values, c, given, out),
        CompOp::Lt => selected::<i64, Lt>(values, c, given, out),
        CompOp::Gt => selected::<i64, Gt>(values, c, given, out),
        CompOp::Le => selected::<i64, Le>(values, c, given, out),
        CompOp::Ge => selected::<i64, Ge>(values, c, given, out),
    }
}

/// As `select_i64`, of approximate numbers.
pub fn select_f64(values: &[f64], (op, c): (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
    match op {
        CompOp::Eq => selected::<f64, Eq>(values, c, given, out),
        CompOp::Ne => selected::<f64, Ne>(values, c, given, out),
        CompOp::Lt => selected::<f64, Lt>(values, c, given, out),
        CompOp::Gt => selected::<f64, Gt>(values, c, given, out),
        CompOp::Le => selected::<f64, Le>(values, c, given, out),
        CompOp::Ge => selected::<f64, Ge>(values, c, given, out),
    }
}

/// The selection of `select_i64` or `select_f64`, of the operator `H`.
fn selected<T: Copy, H: Holds<T>>(values: &[T], c: T, (nulls, sel): Given<'_>, out: &mut Vec<u32>) {
    if let Some(sel) = sel {
        let truth = |r: u32| values.get(index(r)).is_some_and(|&v| H::holds(v, c)) && !nulls.is_some_and(|m| m.get(index(r)));
        return compacted(sel.iter().map(|&r| (r, truth(r))), sel.len(), out);
    }
    let words = nulls.map_or(&[][..], |m| m.words());
    let mut word = 0_u64;
    let rows = (0_u32..).zip(values).enumerate().map(|(n, (r, &v))| {
        if n & (WORD_BITS - 1) == 0 {
            word = words.get(n >> WORD_SHIFT).copied().unwrap_or(0);
        }
        // Not null: its bit clear. Both truths made, neither branched on.
        let held = (word >> (n & (WORD_BITS - 1))) & 1 == 0;
        (r, u8::from(H::holds(v, c)) & u8::from(held) == 1)
    });
    compacted(rows, values.len(), out);
}

/// The shift from a row's place to its null word's.
const WORD_SHIFT: u32 = 6;

/// Appends to `out` the row of each of `rows` whose truth holds, branch-free:
/// every row written to the next slot of room made for `count`, the slots
/// kept moved on by its truth, the rest cut off after.
fn compacted(rows: impl Iterator<Item = (u32, bool)>, count: usize, out: &mut Vec<u32>) {
    let start = out.len();
    out.resize(start.saturating_add(count), 0);
    let mut k = start;
    for (r, keep) in rows {
        if let Some(slot) = out.get_mut(k) {
            *slot = r;
        }
        k = k.wrapping_add(usize::from(keep));
    }
    out.truncate(k);
}

/// The rows of `given`, not null, whose bit is set in `bits`.
pub fn select_mask(bits: &[u64], (nulls, sel): Given<'_>, out: &mut Vec<u32>) {
    let bit = |r: u32| {
        let n = index(r);
        bits.get(n >> WORD_SHIFT).is_some_and(|w| (w >> (n & (WORD_BITS - 1))) & 1 == 1) && !nulls.is_some_and(|m| m.get(n))
    };
    if let Some(sel) = sel {
        return compacted(sel.iter().map(|&r| (r, bit(r))), sel.len(), out);
    }
    let count = nulls.map_or_else(|| bits.len().saturating_mul(WORD_BITS), Mask::len);
    let rows = u32::try_from(count).unwrap_or(u32::MAX);
    compacted((0..rows).map(|r| (r, bit(r))), count, out);
}

/// The rows of `given`, or of all `codes`, not null, whose code's bit is
/// set in `keep`.
pub fn select_codes(codes: &[u32], keep: &[u64], (nulls, sel): Given<'_>, out: &mut Vec<u32>) {
    let kept = |code: u32| {
        let n = index(code);
        keep.get(n >> WORD_SHIFT).is_some_and(|w| (w >> (n & (WORD_BITS - 1))) & 1 == 1)
    };
    let truth = |r: u32| codes.get(index(r)).is_some_and(|&c| kept(c)) && !nulls.is_some_and(|m| m.get(index(r)));
    match sel {
        Some(sel) => compacted(sel.iter().map(|&r| (r, truth(r))), sel.len(), out),
        None => compacted((0_u32..).zip(codes).map(|(r, _)| (r, truth(r))), codes.len(), out),
    }
}
