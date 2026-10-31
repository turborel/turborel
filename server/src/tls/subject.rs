//! \[DESIGN\] The common name of a certificate's subject (RFC 5280
//! §4.1.2.6), which the authenticator `certificate` maps to a user: read
//! from the DER of a certificate TLS has verified, by walking its fields
//! to the subject's, and refused if there is not exactly one. Only the
//! lengths and tags of DER [ITU-T X.690 §8.1] are read; the signature and
//! everything else were checked by rustls.
//!
//! ```text
//! Certificate     SEQUENCE { tbsCertificate, signatureAlgorithm, signature }
//! tbsCertificate  SEQUENCE { [0] version OPTIONAL, serialNumber INTEGER,
//!                            signature SEQUENCE, issuer Name, validity SEQUENCE,
//!                            subject Name, ... }
//! Name            SEQUENCE OF SET OF SEQUENCE { type OID, value ANY }
//! ```

/// \[DESIGN\] DER's tag of a SEQUENCE [X.690 §8.9], constructed.
const SEQUENCE: u8 = 0x30;
/// \[DESIGN\] DER's tag of a SET [X.690 §8.11], constructed.
const SET: u8 = 0x31;
/// \[DESIGN\] DER's tag of a certificate's version, `[0]` explicit
/// [RFC 5280 §4.1].
const VERSION: u8 = 0xa0;
/// \[DESIGN\] DER's tag of an OBJECT IDENTIFIER [X.690 §8.19].
const OID: u8 = 0x06;
/// \[DESIGN\] The DER of id-at-commonName, 2.5.4.3 [RFC 5280 Appendix A.1]:
/// its body, without tag and length.
const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
/// \[DESIGN\] The string tags a common name is taken in [RFC 5280
/// §4.1.2.4]: UTF8String, PrintableString and IA5String, each of UTF-8.
const STRINGS: &[u8] = &[0x0c, 0x13, 0x16];
/// The fields of tbsCertificate before the subject, but the version.
const BEFORE_SUBJECT: usize = 4;

/// The tag, body and rest of the DER element `der` begins with.
fn element(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
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
fn expect(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (found, body, rest) = element(der)?;
    (found == tag).then_some((body, rest))
}

/// The body of the subject of the certificate `der`.
fn subject(der: &[u8]) -> Option<&[u8]> {
    let (certificate, _) = expect(der, SEQUENCE)?;
    let (mut fields, _) = expect(certificate, SEQUENCE)?;
    if let Some((_, rest)) = expect(fields, VERSION) {
        fields = rest;
    }
    for _ in 0..BEFORE_SUBJECT {
        fields = element(fields)?.2;
    }
    expect(fields, SEQUENCE).map(|(body, _)| body)
}

/// The common names among the attributes of the name `name`.
fn common_names(mut name: &[u8]) -> Option<Vec<&[u8]>> {
    let mut found = Vec::new();
    while !name.is_empty() {
        let (mut set, rest) = expect(name, SET)?;
        name = rest;
        while !set.is_empty() {
            let (attribute, after) = expect(set, SEQUENCE)?;
            set = after;
            let (kind, value) = expect(attribute, OID)?;
            let (tag, text, _) = element(value)?;
            if kind == COMMON_NAME && STRINGS.contains(&tag) {
                found.push(text);
            }
        }
    }
    Some(found)
}

/// The one common name of the subject of the certificate `der`, if it has
/// exactly one, of UTF-8; none if it has none, more, or `der` is not a
/// certificate.
pub fn common_name(der: &[u8]) -> Option<String> {
    let names = common_names(subject(der)?)?;
    let [one] = names.as_slice() else { return None };
    String::from_utf8(one.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DER of `tag` around `body`, its length in the short or long form.
    fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if body.len() < 0x80 {
            out.push(u8::try_from(body.len()).unwrap());
        } else {
            out.extend([0x82, u8::try_from(body.len() >> 8_u32).unwrap(), u8::try_from(body.len() & 0xff).unwrap()]);
        }
        out.extend(body);
        out
    }

    /// A certificate's DER, of the subject attributes `subject`, with a
    /// version or not.
    fn certificate(subject: &[(&[u8], u8, &str)], versioned: bool) -> Vec<u8> {
        let name: Vec<u8> = subject
            .iter()
            .flat_map(|(oid, tag, text)| tlv(SET, &tlv(SEQUENCE, &[tlv(OID, oid), tlv(*tag, text.as_bytes())].concat())))
            .collect();
        let mut fields = if versioned { tlv(VERSION, &tlv(0x02, &[2])) } else { Vec::new() };
        fields.extend(
            [tlv(0x02, &[1]), tlv(SEQUENCE, &[]), tlv(SEQUENCE, &[]), tlv(SEQUENCE, &[]), tlv(SEQUENCE, &name), tlv(SEQUENCE, &[0; 200])]
                .concat(),
        );
        tlv(SEQUENCE, &[tlv(SEQUENCE, &fields), tlv(SEQUENCE, &[]), tlv(0x03, &[0])].concat())
    }

    const ORG: &[u8] = &[0x55, 0x04, 0x0a];

    /// Verifies: [DESIGN] RULES.md D42, D43: a certificate's one common name
    /// is read, among other attributes, in UTF8String, PrintableString or
    /// IA5String, with a version or without, its lengths short or long;
    /// none with no common name, two, one of another string type, or not
    /// UTF-8.
    #[test]
    fn the_one_common_name_is_read() {
        let one = certificate(&[(ORG, 0x0c, "Example"), (COMMON_NAME, 0x0c, "ann")], true);
        assert_eq!(common_name(&one).as_deref(), Some("ann"));
        assert_eq!(common_name(&certificate(&[(COMMON_NAME, 0x13, "BOB")], false)).as_deref(), Some("BOB"));
        assert_eq!(common_name(&certificate(&[(COMMON_NAME, 0x16, "c")], true)).as_deref(), Some("c"));
        assert_eq!(common_name(&certificate(&[(ORG, 0x0c, "Example")], true)), None, "none");
        assert_eq!(common_name(&certificate(&[(COMMON_NAME, 0x0c, "a"), (COMMON_NAME, 0x0c, "b")], true)), None, "two");
        assert_eq!(common_name(&certificate(&[(COMMON_NAME, 0x1e, "a")], true)), None, "a BMPString");
        let mut bad = certificate(&[(COMMON_NAME, 0x0c, "é")], true);
        let at = bad.windows(2).position(|w| w == "é".as_bytes()).unwrap();
        bad[at] = 0xff;
        assert_eq!(common_name(&bad), None, "not UTF-8");
    }

    /// Verifies: [DESIGN] RULES.md R-ERR-1: DER not of a certificate gives
    /// no name, never a panic: empty, cut short at each byte, a length of
    /// more than four bytes, or of none, a subject's attribute not a
    /// sequence.
    #[test]
    fn malformed_der_gives_no_name() {
        let whole = certificate(&[(COMMON_NAME, 0x0c, "ann")], true);
        for cut in 0..whole.len() {
            assert_eq!(common_name(&whole[..cut]), None, "cut at {cut}");
        }
        assert_eq!(common_name(&[SEQUENCE, 0x85, 0, 0, 0, 0, 0]), None);
        assert_eq!(common_name(&[SEQUENCE, 0x80]), None);
        let odd = certificate(&[(COMMON_NAME, 0x0c, "ann")], true);
        let at = odd.windows(4).position(|w| w == [SET, 0x0c, SEQUENCE, 0x0a]).unwrap();
        let mut not_sequence = odd.clone();
        not_sequence[at + 2] = SET;
        assert_eq!(common_name(&not_sequence), None);
    }
}
