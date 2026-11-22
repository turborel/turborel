//! \[DESIGN\] The `<key word>`s by their spelling [ISO/IEC 9075:1992 §5.2
//! SR11, SR15]: every `<regular identifier>` a statement writes is looked
//! up among them, so the lookup is one probe, not a walk of them all.

use std::sync::OnceLock;

use super::KEY_WORDS;
use crate::hash::xxhash::Xxh3Set;

/// The `<key word>` spelled `word`, in upper case, if it is one.
pub(super) fn key_word(word: &str) -> Option<&'static str> {
    static ALL: OnceLock<Xxh3Set<&'static str>> = OnceLock::new();
    ALL.get_or_init(|| KEY_WORDS.iter().copied().collect()).get(word).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 9075:1992 §5.2 SR11, SR15] every key word is found
    /// by its spelling, and a word that is none is not.
    #[test]
    fn every_key_word_is_found() {
        assert!(KEY_WORDS.iter().all(|k| key_word(k) == Some(*k)));
        assert_eq!(key_word("SBTEST1"), None);
        assert_eq!(key_word("select"), None, "upper case only");
    }
}
