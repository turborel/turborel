//! \[DESIGN\] A value alone on its cache lines (RULES.md D782), so a
//! thread writing it never takes the line from a thread reading a field
//! beside it. 128 bytes, as a processor that fetches lines in pairs moves
//! two at once.

use std::ops::Deref;

/// `T`, aligned to and padded out to 128 bytes.
#[repr(align(128))]
#[derive(Debug, Default)]
pub(crate) struct Alone<T>(pub(crate) T);

impl<T> Deref for Alone<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;

    /// Verifies: [DESIGN] RULES.md D782: values side by side, each alone,
    /// fall on separate pairs of cache lines.
    #[test]
    fn values_alone_share_no_line() {
        let pair = [Alone(AtomicU64::new(1)), Alone(AtomicU64::new(2))];
        let [first, second] = pair.each_ref().map(|a| std::ptr::from_ref(&a.0).addr());
        assert_eq!(first % 128, 0, "aligned");
        assert_eq!(second.abs_diff(first), 128, "a pair of lines each");
        assert_eq!(size_of::<Alone<u8>>(), 128, "padded");
    }
}
