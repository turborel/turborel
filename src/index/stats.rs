//! \[DESIGN\] What an index knows of its keys, for the planner (RULES.md D34,
//! D35): how many distinct keys it holds, near enough. Each key written is
//! added to a HyperLogLog sketch (`stats/parts.rs`) as it is written, its
//! registers raised lock-free, so the count is there whenever the planner
//! asks, and a sketch holds 4 KiB however many keys there are. The counts
//! are kept with an index's locator (`Index::locator`), so each checkpoint
//! records them with the pages it commits, and an index reopened from it
//! has them again; recovery's mending (`State::mend_indexes`) changes them
//! as it changes the entries. An index whose locator holds none, as one
//! whose counts were never complete, has its keys added in one pass when
//! first asked for.
//!
//! ```text
//! locator: meta page u64, then, if the counts are complete:
//!   entries u64; 0 and each register u8, or 1, a count u32 and each
//!   register not 0 as its place u32 and its value u8 (little-endian)
//! ```
//!
//! A sketch counts the keys ever added, so a key no entry has any more
//! still counts: the live entries, kept with it, bound the count, and
//! after many removals it is high. A register is written only when a key
//! raises it, and the entries are counted in stripes, a thread's own, so
//! threads writing the index share no counter. Split from index.rs
//! (RULES.md R-SIZE-2).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::stats::parts::{REGISTERS, count, estimate_of, hash_of, placed};

/// An index's entries' keys, each in bytes.
pub(crate) type Keys<'a> = Box<dyn Iterator<Item = Result<Vec<u8>>> + 'a>;

/// The stripes the entries are counted in.
const STRIPES: usize = 16;

/// The stripe the next thread to count takes.
static NEXT_STRIPE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's stripe.
    static STRIPE: usize = NEXT_STRIPE.fetch_add(1, Ordering::Relaxed).checked_rem(STRIPES).unwrap_or_default();
}

/// A stripe of the entries: entries added, less those taken, on the
/// threads of the stripe, so below 0 where those taken were added on
/// others; on a cache line of its own.
#[repr(align(64))]
#[derive(Default)]
struct Stripe(AtomicI64);

/// An index's sketch of its keys, and its live entries.
pub(crate) struct KeyCounts {
    registers: Vec<AtomicU8>,
    /// Entries added, less those taken, by stripe: their sum.
    entries: Vec<Stripe>,
    /// Whether every key the index holds was added: false for one opened,
    /// until its pass.
    complete: AtomicBool,
    /// Set while a thread makes that pass.
    counting: AtomicBool,
}

impl KeyCounts {
    /// The counts of an index made new, holding no key: complete.
    pub(crate) fn new() -> KeyCounts {
        KeyCounts::with(true)
    }

    /// The counts of an index opened from its files: its keys added when
    /// first asked for.
    pub(crate) fn opened() -> KeyCounts {
        KeyCounts::with(false)
    }

    fn with(complete: bool) -> KeyCounts {
        let registers = (0..REGISTERS).map(|_| AtomicU8::new(0)).collect();
        let entries = (0..STRIPES).map(|_| Stripe::default()).collect();
        KeyCounts { registers, entries, complete: AtomicBool::new(complete), counting: AtomicBool::new(false) }
    }

    /// Its counts, as a locator holds them, if they are complete.
    fn encoded(&self) -> Option<Vec<u8>> {
        if !self.complete.load(Ordering::Acquire) {
            return None;
        }
        let registers: Vec<u8> = self.registers.iter().map(|r| r.load(Ordering::Relaxed)).collect();
        let raised: Vec<(usize, u8)> = registers.iter().copied().enumerate().filter(|&(_, r)| r > 0).collect();
        let mut out = Vec::new();
        out.put_u64(self.live());
        if raised.len().saturating_mul(5) < registers.len() {
            out.put_u8(1);
            out.put_u32(u32::try_from(raised.len()).unwrap_or(u32::MAX));
            for (at, r) in raised {
                out.put_u32(u32::try_from(at).unwrap_or(u32::MAX));
                out.put_u8(r);
            }
        } else {
            out.put_u8(0);
            out.extend_from_slice(&registers);
        }
        Some(out)
    }

    /// The counts `encoded` gave, complete.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not counts.
    fn decoded(bytes: &[u8]) -> Result<KeyCounts> {
        let counts = KeyCounts::new();
        let mut r = Reader::new(bytes, "index key counts");
        let entries = r.u64()?;
        counts.count_entries(i64::try_from(entries).unwrap_or(i64::MAX));
        if r.u8()? == 0 {
            for (register, &value) in counts.registers.iter().zip(r.take(REGISTERS)?) {
                register.store(value, Ordering::Relaxed);
            }
        } else {
            for _ in 0..r.u32()? {
                let (at, value) = (usize::try_from(r.u32()?).unwrap_or(usize::MAX), r.u8()?);
                counts.registers.get(at).ok_or_else(|| r.corrupt("a register past the sketch"))?.store(value, Ordering::Relaxed);
            }
        }
        r.finish()?;
        Ok(counts)
    }

    /// The locator of an index of meta page `meta` and these counts.
    pub(crate) fn locator(&self, meta: u64) -> Vec<u8> {
        [meta.to_le_bytes().to_vec(), self.encoded().unwrap_or_default()].concat()
    }

    /// The meta page and the counts a `locator` holds: those it holds, else
    /// none, to be added when first asked for.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not a locator.
    pub(crate) fn located(locator: &[u8]) -> Result<(u64, KeyCounts)> {
        let r = Reader::new(locator, "index locator");
        let (meta, counts) = locator.split_at_checked(8).ok_or_else(|| r.corrupt("no meta page"))?;
        let meta = <[u8; 8]>::try_from(meta).map(u64::from_le_bytes).map_err(|_| r.corrupt("no meta page"))?;
        Ok((meta, if counts.is_empty() { KeyCounts::opened() } else { KeyCounts::decoded(counts)? }))
    }

    /// Adds `n` to this thread's stripe of the entries.
    fn count_entries(&self, n: i64) {
        if let Some(stripe) = STRIPE.with(|&at| self.entries.get(at)) {
            stripe.0.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// The live entries: the stripes' sum, never below 0, as a removal of
    /// what was added before the index was opened may come before its pass.
    fn live(&self) -> u64 {
        let sum = self.entries.iter().fold(0_i64, |n, s| n.saturating_add(s.0.load(Ordering::Relaxed)));
        u64::try_from(sum).unwrap_or_default()
    }

    /// Counts an entry of key `key`, in bytes, added.
    pub(crate) fn added(&self, key: &[u8]) {
        let (register, rank) = placed(hash_of(key));
        if let Some(r) = self.registers.get(register)
            && r.load(Ordering::Relaxed) < rank
        {
            r.fetch_max(rank, Ordering::Relaxed);
        }
        self.count_entries(1);
    }

    /// Counts an entry taken.
    pub(crate) fn removed(&self) {
        self.count_entries(-1);
    }

    /// How many distinct keys there are, near enough: none known of an
    /// opened index until a pass over `keys`, its entries' keys in bytes,
    /// the same bytes for equal keys, has added them; the first to ask
    /// makes it, while others have none. A pass that fails is made again
    /// at the next.
    pub(crate) fn distinct<'a>(&self, keys: impl FnOnce() -> Result<Keys<'a>>) -> Option<f64> {
        if !self.complete.load(Ordering::Acquire) {
            if self.counting.swap(true, Ordering::Acquire) {
                return None;
            }
            let passed = self.pass(keys);
            self.counting.store(false, Ordering::Release);
            passed.ok()?;
        }
        let registers: Vec<u8> = self.registers.iter().map(|r| r.load(Ordering::Relaxed)).collect();
        Some(estimate_of(&registers).clamp(0.0_f64, count(self.live())))
    }

    /// Adds every key of `keys` the index held when opened: added again,
    /// those written since are counted once, and their entries too, as the
    /// count bounds the estimate only.
    fn pass<'a>(&self, keys: impl FnOnce() -> Result<Keys<'a>>) -> Result<()> {
        for key in keys()? {
            self.added(&key?);
        }
        self.complete.store(true, Ordering::Release);
        Ok(())
    }
}

// Unit tests: the count, and when it is taken again (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::error::{Code, SqlError};

    /// Keys of `entries` entries, `distinct` of them, counting each pass.
    fn keys(entries: u32, distinct: u32, passes: &Cell<u32>) -> Keys<'static> {
        passes.set(passes.get().saturating_add(1));
        Box::new((0..entries).map(move |n| Ok(n.checked_rem(distinct).unwrap_or_default().to_le_bytes().to_vec())))
    }

    /// Verifies: [DESIGN] RULES.md D35, `KeyCounts`: the keys of an index
    /// made new are counted as they are added, with no pass, near their
    /// number and never past the live entries; an index opened has its
    /// keys added in one pass when first asked for, once, and a pass that
    /// fails is made again at the next.
    #[test]
    fn keys_are_counted_as_added() {
        let passes = Cell::new(0_u32);
        let made = KeyCounts::new();
        for key in keys(1000, 100, &passes) {
            made.added(&key.unwrap());
        }
        let first = made.distinct(|| Err(SqlError::new(Code::Io, "[DESIGN] no pass"))).unwrap();
        assert!((first - 100.0_f64).abs() < 5.0_f64, "{first}");
        for _ in 0_u32..998_u32 {
            made.removed();
        }
        assert_eq!(made.distinct(|| Err(SqlError::new(Code::Io, "[DESIGN] no pass"))), Some(2.0_f64), "no more than the entries");
        let opened = KeyCounts::opened();
        assert_eq!(opened.distinct(|| Err(SqlError::new(Code::Io, "[DESIGN] told to fail"))), None, "a pass that failed");
        let counted = opened.distinct(|| Ok(keys(1000, 3, &passes))).unwrap();
        assert!((counted - 3.0_f64).abs() < 0.5_f64, "{counted}");
        assert_eq!(opened.distinct(|| Ok(keys(1000, 3, &passes))), Some(counted));
        assert_eq!(passes.get(), 2, "the pass made once");
    }

    /// Verifies: [DESIGN] RULES.md D35, `KeyCounts::locator`: complete
    /// counts, of few keys or many, are kept in the locator and given back
    /// by it with no pass; counts not complete are not, and those of the
    /// locator are counted when first asked for; a locator not one is
    /// SQLCODE -916.
    #[test]
    fn counts_are_kept_with_the_locator() {
        let passes = Cell::new(0_u32);
        let no_pass = || Err(SqlError::new(Code::Io, "[DESIGN] no pass"));
        for distinct in [3_u32, 50_000_u32] {
            let made = KeyCounts::new();
            for key in keys(60_000, distinct, &passes) {
                made.added(&key.unwrap());
            }
            let (meta, kept) = KeyCounts::located(&made.locator(42)).unwrap();
            assert_eq!(meta, 42);
            assert_eq!(kept.distinct(no_pass), made.distinct(no_pass), "{distinct} keys");
            assert_eq!(kept.live(), 60_000);
        }
        let opened = KeyCounts::opened();
        assert_eq!(opened.locator(7), 7_u64.to_le_bytes().to_vec(), "no counts until they are complete");
        let (_, again) = KeyCounts::located(&opened.locator(7)).unwrap();
        assert!(again.distinct(|| Ok(keys(10, 2, &passes))).is_some());
        assert_eq!(passes.get(), 3, "the pass made for counts not kept");
        assert_eq!(KeyCounts::located(&[1, 2]).err().map(|e| e.code), Some(Code::Corrupt));
        assert_eq!(KeyCounts::located(&[0, 0, 0, 0, 0, 0, 0, 0, 1]).err().map(|e| e.code), Some(Code::Corrupt));
        let mut past = 0_u64.to_le_bytes().to_vec();
        past.extend([0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0xFF, 0xFF, 0, 0, 1]);
        assert_eq!(KeyCounts::located(&past).err().map(|e| e.code), Some(Code::Corrupt), "a register past the sketch");
    }
}
