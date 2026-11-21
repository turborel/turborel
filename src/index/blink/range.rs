//! \[DESIGN\] The ranges of a B-link index's entry keys a probe reads
//! (`blink.rs`), and their cuts into parts for parallel reads.

use std::ops::Bound;

use super::BLinkIndex;
use crate::error::Result;

/// The least key after every key beginning with `prefix`: its last byte
/// below 0xFF raised by one, the bytes after it dropped; none if every byte
/// is 0xFF, when no key comes after.
pub(super) fn after(prefix: &[u8]) -> Bound<Vec<u8>> {
    let Some(at) = prefix.iter().rposition(|&b| b < u8::MAX) else { return Bound::Unbounded };
    let mut next = prefix.get(..=at).unwrap_or_default().to_vec();
    if let Some(last) = next.last_mut() {
        *last = last.saturating_add(1);
    }
    Bound::Excluded(next)
}

/// The keys beginning with `prefix`, each checked if `checked`.
pub(super) fn within(prefix: Vec<u8>, checked: bool) -> Range {
    let upper = after(&prefix);
    Range { lower: Bound::Included(prefix), upper, checked }
}

/// As `after`, as a lower bound: the keys from it on.
pub(super) fn from_after(prefix: &[u8]) -> Option<Bound<Vec<u8>>> {
    match after(prefix) {
        Bound::Excluded(next) => Some(Bound::Included(next)),
        Bound::Included(_) | Bound::Unbounded => None,
    }
}

/// The bounds of the entries' keys a probe reads, and whether each key
/// read must be checked against the probe: not if the bounds are exact.
pub(super) struct Range {
    pub(super) lower: Bound<Vec<u8>>,
    pub(super) upper: Bound<Vec<u8>>,
    pub(super) checked: bool,
}

impl Range {
    /// Every entry, each checked if `checked`.
    pub(super) fn all(checked: bool) -> Range {
        Range { lower: Bound::Unbounded, upper: Bound::Unbounded, checked }
    }
}

impl BLinkIndex {
    /// \[DESIGN\] (RULES.md D297) `range` cut at keys of its tree's inner
    /// nodes into up to about `most` parts, in order (`engine/tree/cuts.rs`).
    pub(super) fn cut(&self, range: Range, most: usize) -> Result<Vec<Range>> {
        let cuts = self.tree.cuts(range.lower.as_ref().map(Vec::as_slice), range.upper.as_ref().map(Vec::as_slice), most)?;
        let mut lower = range.lower;
        let mut parts = Vec::with_capacity(cuts.len().saturating_add(1));
        for cut in cuts {
            // A key equal to a cut is of the part below it, as of the
            // child left of it (`engine/tree/node.rs`).
            let upper = Bound::Included(cut.clone());
            parts.push(Range { lower: std::mem::replace(&mut lower, Bound::Excluded(cut)), upper, checked: range.checked });
        }
        parts.push(Range { lower, upper: range.upper, checked: range.checked });
        Ok(parts)
    }
}
