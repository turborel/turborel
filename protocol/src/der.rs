//! \[DESIGN\] DER's elements, read by their tags and lengths alone [ITU-T
//! X.690 §8.1]: enough to walk a certificate to a field, as the server
//! does to its client's subject and both sides do to the server's
//! signature algorithm (`scram::end_point`). Nothing is checked here but
//! the lengths: what is read comes from a certificate TLS has verified.

/// \[DESIGN\] DER's tag of a SEQUENCE [X.690 §8.9], constructed.
pub const SEQUENCE: u8 = 0x30;
/// \[DESIGN\] DER's tag of an OBJECT IDENTIFIER [X.690 §8.19].
pub const OID: u8 = 0x06;

/// The tag, body and rest of the DER element `der` begins with; none if
/// it is cut short, or its length is of more than four bytes.
pub fn element(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        // Long form: the low bits count the length's bytes, at most four.
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let (bytes, after) = rest.split_at_checked(count)?;
        (bytes.iter().fold(0_usize, |n, b| n.wrapping_shl(8) | usize::from(*b)), after)
    };
    let (body, after) = rest.split_at_checked(length)?;
    Some((tag, body, after))
}

/// The body of the element `der` begins with, if its tag is `tag`, and
/// the rest after it.
pub fn expect(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (found, body, rest) = element(der)?;
    (found == tag).then_some((body, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] X.690 §8.1: an element's tag, body and rest, its
    /// length short or long; none if cut short, of another tag, or its
    /// length of no bytes or more than four.
    #[test]
    fn elements_are_read_by_tag_and_length() {
        assert_eq!(element(&[OID, 1, 7, 9]), Some((OID, &[7_u8][..], &[9_u8][..])));
        let long = [&[SEQUENCE, 0x81, 2][..], &[1, 2, 3]].concat();
        assert_eq!(expect(&long, SEQUENCE), Some((&[1_u8, 2][..], &[3_u8][..])));
        assert_eq!(expect(&long, OID), None, "another tag");
        for short in [&[][..], &[SEQUENCE], &[SEQUENCE, 2, 1], &[SEQUENCE, 0x80], &[SEQUENCE, 0x85, 0, 0, 0, 0, 1], &[SEQUENCE, 0x82, 1]] {
            assert_eq!(element(short), None, "{short:?}");
        }
    }
}
