//! \[DESIGN\] (RULES.md D1187) Whether a range of character strings lies
//! wholly past a sort's bound of a key that orders strings by their
//! octets. A group's and a part's least and greatest are kept in the
//! padded order [X3.135-1986 §5.11 GR5], which differs from the order of
//! octets in two cases alone (RULES.md D1183). Strings that differ only
//! in spaces after them are one value padded and apart by octets, the
//! shorter the less. And a string that goes on past another with an
//! octet below a space, after any spaces, is the less padded and the
//! greater by octets. So a range is passed over only where neither case
//! can put a string of it at or before the bound.
//!
//! Each rule reads the first octet at which the range's end and the
//! bound differ, both having one there.
//!
//! - Of a descending key the bound is the least string kept, and the
//!   range is past it if its greatest has the lesser octet there. A
//!   string at most the greatest padded either agrees with it that far,
//!   and so has that lesser octet, or falls below it earlier, by a
//!   lesser octet where the bound has the greatest's, or by ending
//!   there, a prefix of the bound. Each is before the bound by octets.
//! - Of an ascending key the bound is the greatest string kept, and the
//!   range is past it if its least has the greater octet there, that
//!   octet is above a space, and no octet of the least before it is
//!   below a space. A string at least the least padded either agrees
//!   with it that far or rises above it earlier, by a greater octet
//!   where the bound has the least's. It cannot end before that octet,
//!   as then the least would go on with spaces alone to an octet that
//!   is no space, or with an octet below a space, which the rule
//!   excludes. So it is after the bound by octets.
//!
//! A range whose end is a prefix of the bound, or the bound of it, is
//! never passed over. The least and greatest are compared as they are
//! kept, whichever of the strings equal padded was written.

use crate::value::Value;

/// The pad character's octet [X3.135-1986 §5.11 GR5].
const SPACE: u8 = b' ';

/// Whether every character string from `least` to `greatest`, in the
/// padded order, is past `bound` by octets, which is before it of a key
/// `descending` and after it of any other. False if any of the three is
/// no character string.
pub(super) fn past(descending: bool, (least, greatest): (&Value, &Value), bound: &Value) -> bool {
    let (Value::Char(least), Value::Char(greatest), Value::Char(bound)) = (least, greatest, bound) else { return false };
    if descending { all_before(greatest.as_bytes(), bound.as_bytes()) } else { all_after(least.as_bytes(), bound.as_bytes()) }
}

/// The first place at which `end` and `bound` both have an octet and
/// differ, and the two octets there.
fn parted(end: &[u8], bound: &[u8]) -> Option<(usize, u8, u8)> {
    let at = end.iter().zip(bound).position(|(a, b)| a != b)?;
    Some((at, *end.get(at)?, *bound.get(at)?))
}

/// Whether every string at most `greatest` padded is before `bound` by
/// octets.
fn all_before(greatest: &[u8], bound: &[u8]) -> bool {
    parted(greatest, bound).is_some_and(|(_, ours, theirs)| ours < theirs)
}

/// Whether every string at least `least` padded is after `bound` by
/// octets.
fn all_after(least: &[u8], bound: &[u8]) -> bool {
    let Some((at, ours, theirs)) = parted(least, bound) else { return false };
    ours > theirs && ours > SPACE && least.iter().take(at).all(|&octet| octet >= SPACE)
}

// Unit tests: the rules against every short string of the octets the two
// orders differ by (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::compare;
    use std::cmp::Ordering;

    /// Every string of at most four of a tab, a space, `a` and `b`.
    fn strings() -> Vec<String> {
        let mut all = vec![String::new()];
        let mut last = all.clone();
        for _ in 0..4_usize {
            last = last.iter().flat_map(|s| ['\t', ' ', 'a', 'b'].map(|c| format!("{s}{c}"))).collect();
            all.extend(last.iter().cloned());
        }
        all
    }

    /// Whether the rules pass over the zone of the two strings of `zone`
    /// for `bound`, each given with its value. Its least and greatest are
    /// those the padded comparison gives of the two, the second written
    /// kept where they are equal. If the zone is passed over, neither
    /// string may be one the sort keeps.
    fn passed_soundly([(x, vx), (y, vy)]: [(&str, &Value); 2], (bound, vb): (&str, &Value), descending: bool) -> bool {
        let (least, greatest) = if compare(vx, vy) == Some(Ordering::Less) { (vx, vy) } else { (vy, vx) };
        if !past(descending, (least, greatest), vb) {
            return false;
        }
        for s in [x, y] {
            let kept = if descending { s.as_bytes() >= bound.as_bytes() } else { s.as_bytes() <= bound.as_bytes() };
            assert!(!kept, "{s:?} of the zone {x:?}, {y:?} is kept by the bound {bound:?}, descending {descending}");
        }
        true
    }

    /// Verifies: [DESIGN] RULES.md D1187 [X3.135-1986 §5.11 GR5]: over
    /// every string of at most four octets of a tab, a space and two
    /// letters, as a zone's contents and as a bound, in both directions,
    /// a zone is never passed over that holds a string the sort would
    /// keep. A zone is a pair of strings or one string twice, its least
    /// and greatest those the padded comparison gives of them in either
    /// order they were written, which covers every zone, as a zone
    /// wrongly passed over has a string at or before the bound and that
    /// string with the zone's end is such a pair. And the rules do pass
    /// over zones, of each direction, so they are not vacuous.
    #[test]
    fn a_zone_passed_over_holds_no_string_at_or_before_the_bound() {
        let all = strings();
        assert_eq!(all.len(), 341);
        let valued: Vec<(&str, Value)> = all.iter().map(|s| (s.as_str(), Value::Char(s.as_str().into()))).collect();
        let mut passed = [0_usize; 2];
        for ((x, vx), (y, vy)) in valued.iter().flat_map(|first| valued.iter().map(move |second| (first, second))) {
            for (bound, vb) in &valued {
                for descending in [false, true] {
                    passed[usize::from(descending)] += usize::from(passed_soundly([(x, vx), (y, vy)], (bound, vb), descending));
                }
            }
        }
        assert!(passed[0] > 0 && passed[1] > 0, "zones are passed over: {passed:?}");
    }

    /// Verifies: [DESIGN] RULES.md D1187: a range is passed over by the
    /// first octet its end and the bound differ at. Descending, its
    /// greatest has the lesser octet there. Ascending, its least has the
    /// greater there, above a space, with no octet below a space before
    /// it. One whose end is a prefix of the bound, or the bound of it,
    /// is kept, as is one of values that are no character strings.
    ///
    /// MC/DC of the ascending rule, `greater && above a space && none
    /// below a space before`: `ab` against `aa` (all true, passed over),
    /// `aa` against `ab` (the first false), `a ` against `a` and a tab
    /// (the second false), a tab and `b` against a tab and `a` (the
    /// third false).
    #[test]
    fn a_range_is_passed_over_by_the_first_octet_that_differs() {
        let s = |t: &str| Value::Char(t.into());
        let up = |least: &str, bound: &str| past(false, (&s(least), &s("zz")), &s(bound));
        let down = |greatest: &str, bound: &str| past(true, (&s(""), &s(greatest)), &s(bound));
        assert!(up("ab", "aa") && up("b", "a\t") && up("a b", "a a"));
        assert!(!up("aa", "ab") && !up("a ", "a\t") && !up("\tb", "\ta"));
        assert!(!up("ab", "a") && !up("a", "ab") && !up("a", "a"), "a prefix of the other, or equal");
        assert!(down("aa", "ab") && down("a\t", "a ") && down("\ta", "\tb") && down("a", "b "));
        assert!(!down("ab", "aa") && !down("a", "a ") && !down("a ", "a") && !down("a", "a"));
        let n = Value::Exact(crate::value::Decimal::new(1, 0));
        assert!(!past(false, (&n, &s("b")), &s("a")) && !past(true, (&s("a"), &n), &s("b")) && !past(false, (&s("b"), &s("b")), &n));
    }
}
