//! [ISO/IEC 9075-2:2023 §6.31 GR2, GR3] Where one string first occurs in
//! another, either of them perhaps a large object, found without holding
//! either whole (RULES.md D79). The search is the Two-Way string matching
//! of Crochemore and Perrin, which keeps four counters and reads both
//! strings by position, so a large object is read through two windows of
//! its octets, the last two read.

use crate::error::Result;
use crate::value::lob::{Lob, LobReader, broken};

/// \[DESIGN\] The octets of a large object read into one window: a quarter
/// of a chunk, so a window read touches one chunk or two.
const WINDOW_OCTETS: u64 = 65_536;

/// The octets of a string, read by position.
pub(crate) enum Octets<'a> {
    /// Held in memory.
    Held(&'a [u8]),
    /// Of a large object, read a window at a time.
    Large(Windows<'a>),
}

/// The windows read of a large object: the last read, and the one before,
/// so a search reading two places in turn reads neither again.
pub(crate) struct Windows<'a> {
    reader: LobReader<'a>,
    length: u64,
    recent: Option<(u64, Vec<u8>)>,
    older: Option<(u64, Vec<u8>)>,
}

/// The octet at `i` of `window`, a start and its octets, if it holds it.
fn hit(window: Option<&(u64, Vec<u8>)>, i: u64) -> Option<u8> {
    let (start, bytes) = window?;
    bytes.get(usize::try_from(i.checked_sub(*start)?).ok()?).copied()
}

impl<'a> Octets<'a> {
    /// The octets of `lob`.
    pub(crate) fn of_lob(lob: &'a Lob) -> Octets<'a> {
        Octets::Large(Windows { reader: lob.reader(), length: lob.octets(), recent: None, older: None })
    }

    /// How many it has.
    pub(crate) fn len(&self) -> u64 {
        match self {
            Octets::Held(bytes) => u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            Octets::Large(w) => w.length,
        }
    }

    /// The octet at `i`, from 0.
    ///
    /// # Errors
    ///
    /// As reading a chunk fails; SQLCODE -910 past the end.
    pub(crate) fn at(&mut self, i: u64) -> Result<u8> {
        match self {
            Octets::Held(bytes) => {
                usize::try_from(i).ok().and_then(|i| bytes.get(i)).copied().ok_or_else(|| broken("a search read past a string"))
            }
            Octets::Large(w) => w.at(i),
        }
    }
}

impl Windows<'_> {
    /// The octet at `i`, from a window read, or from the window holding it,
    /// read now.
    fn at(&mut self, i: u64) -> Result<u8> {
        if let Some(b) = hit(self.recent.as_ref(), i) {
            return Ok(b);
        }
        if let Some(b) = hit(self.older.as_ref(), i) {
            std::mem::swap(&mut self.recent, &mut self.older);
            return Ok(b);
        }
        let start = i.saturating_sub(i.checked_rem(WINDOW_OCTETS).unwrap_or(0));
        let bytes = self.reader.read(start, WINDOW_OCTETS)?;
        let window = (start, bytes);
        let b = hit(Some(&window), i).ok_or_else(|| broken("a search read past a large object"))?;
        self.older = self.recent.replace(window);
        Ok(b)
    }
}

/// How a needle is searched for: its critical factorization, at `crit`,
/// and the period by which a search shifts past a mismatch left of it;
/// `long` if the needle is not periodic, so no memory of a match is kept.
struct Plan {
    crit: u64,
    period: u64,
    long: bool,
}

/// The maximal suffix of `needle` by the order of its octets, or by the
/// reverse order if `greater`: where it begins, and its period.
fn maximal_suffix(needle: &mut Octets<'_>, greater: bool) -> Result<(u64, u64)> {
    let m = needle.len();
    let (mut left, mut right, mut offset, mut period) = (0_u64, 1_u64, 0_u64, 1_u64);
    while right.saturating_add(offset) < m {
        let a = needle.at(right.saturating_add(offset))?;
        let b = needle.at(left.saturating_add(offset))?;
        if a == b {
            // Advance through the repetition of the current period.
            if offset.saturating_add(1) == period {
                right = right.saturating_add(period);
                offset = 0;
            } else {
                offset = offset.saturating_add(1);
            }
        } else if (a < b) != greater {
            // A suffix past `right` is smaller: the period is all of it.
            right = right.saturating_add(offset).saturating_add(1);
            offset = 0;
            period = right.saturating_sub(left);
        } else {
            // A larger suffix begins at `right`.
            left = right;
            right = right.saturating_add(1);
            offset = 0;
            period = 1;
        }
    }
    Ok((left, period))
}

impl Plan {
    /// The plan of searching for `needle`, not empty.
    fn new(needle: &mut Octets<'_>) -> Result<Plan> {
        let m = needle.len();
        let (c1, p1) = maximal_suffix(needle, false)?;
        let (c2, p2) = maximal_suffix(needle, true)?;
        let (crit, period) = if c1 > c2 { (c1, p1) } else { (c2, p2) };
        // Periodic if the part left of the critical point recurs a period on.
        let mut periodic = period.saturating_add(crit) <= m;
        let mut i = 0_u64;
        while periodic && i < crit {
            let (x, y) = (needle.at(i)?, needle.at(period.saturating_add(i))?);
            periodic = x == y;
            i = i.saturating_add(1);
        }
        if periodic {
            return Ok(Plan { crit, period, long: false });
        }
        Ok(Plan { crit, period: crit.max(m.saturating_sub(crit)).saturating_add(1), long: true })
    }

    /// Where `needle`, of `m` octets, first occurs in `hay`.
    fn search(&self, needle: &mut Octets<'_>, hay: &mut Octets<'_>) -> Result<Option<u64>> {
        let (m, n) = (needle.len(), hay.len());
        let (mut pos, mut memory) = (0_u64, 0_u64);
        while pos.saturating_add(m) <= n {
            // The right part, left to right.
            let mut i = if self.long { self.crit } else { self.crit.max(memory) };
            while i < m {
                let same = needle.at(i)? == hay.at(pos.saturating_add(i))?;
                if !same {
                    break;
                }
                i = i.saturating_add(1);
            }
            if i < m {
                pos = pos.saturating_add(i.saturating_sub(self.crit)).saturating_add(1);
                memory = 0;
                continue;
            }
            // The left part, right to left, down to what is known to match.
            let low = if self.long { 0 } else { memory };
            let mut j = self.crit;
            while j > low {
                let at = j.saturating_sub(1);
                let same = needle.at(at)? == hay.at(pos.saturating_add(at))?;
                if !same {
                    break;
                }
                j = at;
            }
            if j > low {
                pos = pos.saturating_add(self.period);
                memory = if self.long { 0 } else { m.saturating_sub(self.period) };
                continue;
            }
            return Ok(Some(pos));
        }
        Ok(None)
    }
}

/// Where `needle` first occurs in `hay`: the octets of `hay` before it;
/// none if it does not. An empty needle occurs at 0.
///
/// # Errors
///
/// As reading a chunk of either fails.
pub(crate) fn find(needle: &mut Octets<'_>, hay: &mut Octets<'_>) -> Result<Option<u64>> {
    let (m, n) = (needle.len(), hay.len());
    if m == 0 {
        return Ok(Some(0));
    }
    if m > n {
        return Ok(None);
    }
    Plan::new(needle)?.search(needle, hay)
}
