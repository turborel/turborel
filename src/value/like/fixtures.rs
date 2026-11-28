//! Fixtures of the unit tests of a LIKE pattern's machine, shared with
//! the tests of what steps it over codes (RULES.md R-TEST-10).

/// Every string of the octets of `alphabet`, of `longest` octets at
/// most, the shorter first.
pub(crate) fn strings(alphabet: &[u8], longest: usize) -> Vec<Vec<u8>> {
    let mut all = vec![Vec::new()];
    let mut from = 0_usize;
    for _ in 0..longest {
        let until = all.len();
        for at in from..until {
            for &octet in alphabet {
                let mut longer = all[at].clone();
                longer.push(octet);
                all.push(longer);
            }
        }
        from = until;
    }
    all
}

/// Patterns of no `_` of every shape a machine has states for: of no
/// `%`, a first piece, a last, pieces between, a piece that begins again
/// inside itself, and `%` alone and doubled.
pub(crate) const PATTERNS: [&str; 24] = [
    "", "%", "%%", "a", "ab", "ab%", "%ab", "%ab%", "a%b", "ab%ab", "%a%b%", "a%b%c", "%ab%ba%", "aba%ba", "%aab%", "%aab", "%abab%",
    "%aba", "%aa", "%a%a", "%aa%aa%", "ab%%ba", "%b%ab", "%c%",
];
