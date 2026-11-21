//! \[DESIGN\] A node's key as it lies in the node (RULES.md D500): the
//! prefix every key of the node shares, kept once, and the key's own tail,
//! which together are the key. Keys compare, copy and test as the bytes
//! they stand for, so no caller joins the two pieces to read a key.

use std::cmp::Ordering;

/// \[DESIGN\] Bytes of a key head: the first bytes of a key's tail, kept
/// beside its slot and compared as one integer (Müller, Benson and Leis,
/// "B-Trees Are Back", 2025, §3).
pub(in crate::engine::tree) const HEAD: usize = 4;

/// A key of a node: its prefix, shared by the node's keys, then its tail.
#[derive(Clone, Copy, Debug)]
pub(in crate::engine::tree) struct KeyRef<'a> {
    prefix: &'a [u8],
    tail: &'a [u8],
}

impl<'a> KeyRef<'a> {
    /// The key `prefix` then `tail`.
    pub(in crate::engine::tree) fn new(prefix: &'a [u8], tail: &'a [u8]) -> KeyRef<'a> {
        KeyRef { prefix, tail }
    }

    /// The key `key`, whole.
    pub(in crate::engine::tree) fn whole(key: &'a [u8]) -> KeyRef<'a> {
        KeyRef { prefix: &[], tail: key }
    }

    /// Its bytes.
    pub(in crate::engine::tree) fn len(&self) -> usize {
        self.prefix.len().saturating_add(self.tail.len())
    }

    /// Its bytes, as one.
    pub(in crate::engine::tree) fn to_vec(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        self.put(&mut out);
        out
    }

    /// Appends its bytes to `out`.
    pub(in crate::engine::tree) fn put(self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.prefix);
        out.extend_from_slice(self.tail);
    }

    /// Its bytes from `at` on, in two pieces.
    pub(in crate::engine::tree) fn from(self, at: usize) -> (&'a [u8], &'a [u8]) {
        match at.checked_sub(self.prefix.len()) {
            None => (self.prefix.get(at..).unwrap_or_default(), self.tail),
            Some(past) => (&[], self.tail.get(past..).unwrap_or_default()),
        }
    }

    /// How it compares with the key `other`, byte by byte.
    pub(in crate::engine::tree) fn cmp_bytes(&self, other: &[u8]) -> Ordering {
        let (head, rest) = other.split_at_checked(self.prefix.len()).unwrap_or((other, &[]));
        match self.prefix.get(..head.len()).unwrap_or(self.prefix).cmp(head) {
            Ordering::Equal if head.len() < self.prefix.len() => Ordering::Greater,
            Ordering::Equal => self.tail.cmp(rest),
            unequal @ (Ordering::Less | Ordering::Greater) => unequal,
        }
    }

    /// Whether it is the key `other`.
    pub(in crate::engine::tree) fn is(&self, other: &[u8]) -> bool {
        self.len() == other.len() && self.cmp_bytes(other) == Ordering::Equal
    }

    /// Whether it begins with `begins`.
    pub(in crate::engine::tree) fn starts_with(&self, begins: &[u8]) -> bool {
        let (head, rest) = begins.split_at_checked(self.prefix.len()).unwrap_or((begins, &[]));
        self.prefix.starts_with(head) && (rest.is_empty() || self.tail.starts_with(rest))
    }

    /// The bytes it shares from the start with the key `other`.
    pub(in crate::engine::tree) fn common(&self, other: &[u8]) -> usize {
        self.prefix.iter().chain(self.tail).zip(other).take_while(|(a, b)| a == b).count()
    }
}

/// The head of `tail`: its first `HEAD` bytes, zeros past its end, as a
/// big-endian integer, so heads order as their tails do, or are equal.
pub(in crate::engine::tree) fn head_of(tail: &[u8]) -> u32 {
    let mut head = [0_u8; HEAD];
    for (to, from) in head.iter_mut().zip(tail) {
        *to = *from;
    }
    u32::from_be_bytes(head)
}

/// The bytes `a` and `b` share from the start.
pub(in crate::engine::tree) fn common(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}
