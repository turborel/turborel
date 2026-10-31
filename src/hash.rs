//! \[DESIGN\] Hash functions (RULES.md D30): a `HashFunction` maps bytes to a
//! fixed-length `Digest`. The storage components use one to checksum what
//! they write, and other uses (hash indexes, partitioning) may follow; which
//! function is pluggable, and a file records the `HashId` it was written
//! with, so it is read back with the same one.
//!
//! A digest is bytes in a fixed order, never a native integer, so a file
//! checksummed on one machine verifies on any other.
//!
//! Built in: the CRC family (`hash/crc.rs`), for checksums; MurmurHash3
//! (`hash/murmur.rs`), for hash tables and filters. Cryptographic ones,
//! SHA-256 and HMAC-SHA-256, are plug-ins on a vetted crate
//! (`turborel-aes`, RULES.md D26).

pub mod crc;
pub mod murmur;

/// \[DESIGN\] The most bytes a digest holds: 256 bits, enough for the widest
/// hash expected (a cryptographic one, RULES.md D26).
pub const MAX_DIGEST_BYTES: usize = 32;

/// \[DESIGN\] Identifies a hash function in file formats. An id, once given,
/// is never changed or reused: a file written with it must always be read
/// with the same function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HashId(pub u32);

/// \[DESIGN\] The hash functions of plug-ins whose ids RULES.md D26 gives,
/// by name: so a file checksummed with one not registered says which.
const PLUGGED: [(HashId, &str); 2] = [(HashId(11), "SHA-256"), (HashId(12), "HMAC-SHA-256")];

/// Hash function `id`, as a message names it: its id, and its name if
/// RULES.md D26 gives the id to a plug-in's.
pub fn described(id: HashId) -> String {
    match PLUGGED.iter().find(|(known, _)| *known == id) {
        Some((_, name)) => format!("{} ({name})", id.0),
        None => id.0.to_string(),
    }
}

/// The value of a hash function: `len` bytes, in the order the function
/// defines (little-endian for the CRC family). The default is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Digest {
    bytes: [u8; MAX_DIGEST_BYTES],
    len: usize,
}

impl Digest {
    /// The digest made of `bytes`, or `None` if there are more than
    /// `MAX_DIGEST_BYTES`.
    pub fn new(bytes: &[u8]) -> Option<Digest> {
        let mut digest = Digest { bytes: [0; MAX_DIGEST_BYTES], len: bytes.len() };
        digest.bytes.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(digest)
    }

    /// The first `len` bytes of `value` in little-endian order; `len` is at
    /// most 8.
    pub fn from_le(value: u64, len: usize) -> Option<Digest> {
        Digest::new(value.to_le_bytes().get(..len)?)
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }
}

/// \[DESIGN\] A hash function: an extension point (RULES.md D30).
///
/// Contract (RULES.md R-SAFE-3; checked for the built-in functions by their
/// unit tests):
/// * `id` is unique among the functions in use, and constant forever.
/// * `hash_parts` of several parts equals `hash` of their concatenation, so a
///   caller can hash a header and a payload without copying them together.
/// * The digest depends only on the bytes, and on what the function was
///   made with (a keyed one's key, RULES.md D26), never on the machine, and
///   always has `digest_len` bytes.
/// * Methods may be called from several threads at once (`Send + Sync`).
pub trait HashFunction: Send + Sync {
    /// Its id in file formats.
    fn id(&self) -> HashId;
    /// Its name, for messages.
    fn name(&self) -> &'static str;
    /// The bytes in each of its digests.
    fn digest_len(&self) -> usize;
    /// The digest of `parts`, concatenated.
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest;
    /// The digest of `data`.
    fn hash(&self, data: &[u8]) -> Digest {
        self.hash_parts(&[data])
    }
}

// Unit tests: digests hold what they are given, and no more (RULES.md R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D30: a digest keeps its bytes in order; one
    /// longer than `MAX_DIGEST_BYTES`, or a little-endian prefix longer than
    /// 8 bytes, is refused.
    #[test]
    fn digests_hold_their_bytes() {
        assert_eq!(Digest::new(&[1, 2, 3]).unwrap().as_bytes(), &[1, 2, 3]);
        assert_eq!(Digest::new(&[7; MAX_DIGEST_BYTES]).unwrap().as_bytes().len(), MAX_DIGEST_BYTES);
        assert!(Digest::new(&[0; MAX_DIGEST_BYTES + 1]).is_none());
        assert_eq!(Digest::from_le(0x0102_0304, 2).unwrap().as_bytes(), &[4, 3]);
        assert!(Digest::from_le(0, 9).is_none());
        assert_eq!(Digest::new(&[]).unwrap().as_bytes().len(), 0);
    }

    /// Verifies: [DESIGN] RULES.md D26, D30: a hash function a file names
    /// is described by its id, and by its name if a plug-in's id.
    #[test]
    fn hash_functions_are_described_by_id_and_known_name() {
        assert_eq!(described(HashId(12)), "12 (HMAC-SHA-256)");
        assert_eq!(described(HashId(11)), "11 (SHA-256)");
        assert_eq!(described(HashId(9001)), "9001");
    }
}
