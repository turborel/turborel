//! \[DESIGN\] The channel binding `tls-server-end-point` [RFC 5929 §4]:
//! the hash of the TLS server's certificate, as it appears in the
//! server's Certificate message, by a hash function its signature
//! algorithm chooses. Both sides compute it of the same certificate, the
//! server of the one it presents, the client of the one it received, so
//! a server in the middle, presenting its own, binds another channel.
//!
//! ```text
//! Certificate          SEQUENCE { tbsCertificate SEQUENCE, signatureAlgorithm, signature }
//! AlgorithmIdentifier  SEQUENCE { algorithm OID, parameters ANY OPTIONAL }
//! RSASSA-PSS-params    SEQUENCE { [0] hashAlgorithm DEFAULT SHA-1,
//!                                 [1] maskGenAlgorithm DEFAULT MGF1 with SHA-1, ... }
//! ```
//!
//! RFC 5280 §4.1 gives the first two, RFC 4055 §3.1 the third. A
//! signature of MD5 or SHA-1 is bound by SHA-256; of SHA-256, SHA-384 or
//! SHA-512, by that; of no hash, as Ed25519's, or of two, as RSASSA-PSS of
//! a mask hash not its message hash, or of an algorithm not known here,
//! there is no binding, as RFC 5929 §4.1 defines none.

use aws_lc_rs::digest::{self, Algorithm, SHA256, SHA384, SHA512};

use crate::der::{OID, SEQUENCE, expect};

// The DER of each object identifier below is its body, without tag and
// length [X.690 §8.19]: pkcs-1 is 1.2.840.113549.1.1, `2a 86 48 86 f7 0d
// 01 01` [RFC 4055 §6]; id-ecSigType 1.2.840.10045.4, `2a 86 48 ce 3d 04`
// [RFC 3279 §2.2.3]; hashalgs 2.16.840.1.101.3.4.2, `60 86 48 01 65 03 04
// 02` [RFC 4055 §2.1]; each arc after them below 128, one byte.

/// \[DESIGN\] md5WithRSAEncryption, pkcs-1 4 [RFC 3279 §2.2.1].
const MD5_RSA: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x04];
/// \[DESIGN\] sha1WithRSAEncryption, pkcs-1 5 [RFC 3279 §2.2.1].
const SHA1_RSA: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05];
/// \[DESIGN\] id-mgf1, pkcs-1 8 [RFC 4055 §2.2].
const MGF1: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
/// \[DESIGN\] id-RSASSA-PSS, pkcs-1 10 [RFC 4055 §3.1].
const RSA_PSS: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
/// \[DESIGN\] sha256WithRSAEncryption, pkcs-1 11 [RFC 4055 §5].
const SHA256_RSA: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
/// \[DESIGN\] sha384WithRSAEncryption, pkcs-1 12 [RFC 4055 §5].
const SHA384_RSA: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
/// \[DESIGN\] sha512WithRSAEncryption, pkcs-1 13 [RFC 4055 §5].
const SHA512_RSA: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
/// \[DESIGN\] ecdsa-with-SHA1, id-ecSigType 1 [RFC 3279 §2.2.3].
const SHA1_ECDSA: [u8; 7] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x01];
/// \[DESIGN\] ecdsa-with-SHA256, id-ecSigType 3 2 [RFC 5758 §3.2].
const SHA256_ECDSA: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
/// \[DESIGN\] ecdsa-with-SHA384, id-ecSigType 3 3 [RFC 5758 §3.2].
const SHA384_ECDSA: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
/// \[DESIGN\] ecdsa-with-SHA512, id-ecSigType 3 4 [RFC 5758 §3.2].
const SHA512_ECDSA: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
/// \[DESIGN\] id-sha1, 1.3.14.3.2.26 [RFC 4055 §2.1].
const SHA1: [u8; 5] = [0x2b, 0x0e, 0x03, 0x02, 0x1a];
/// \[DESIGN\] id-sha256, hashalgs 1 [RFC 4055 §2.1].
const SHA256_HASH: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
/// \[DESIGN\] id-sha384, hashalgs 2 [RFC 4055 §2.1].
const SHA384_HASH: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
/// \[DESIGN\] id-sha512, hashalgs 3 [RFC 4055 §2.1].
const SHA512_HASH: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
/// \[DESIGN\] The tag of RSASSA-PSS-params' hashAlgorithm, `[0]`
/// explicit [RFC 4055 §3.1, §6: EXPLICIT TAGS].
const PSS_HASH: u8 = 0xa0;
/// \[DESIGN\] The tag of RSASSA-PSS-params' maskGenAlgorithm, `[1]`
/// explicit [RFC 4055 §3.1].
const PSS_MASK: u8 = 0xa1;

/// \[DESIGN\] The signature algorithms of one hash, and the hash that
/// binds a certificate they sign [RFC 5929 §4.1]: SHA-256 for MD5 and
/// SHA-1.
static SIGNATURES: [(&[u8], &Algorithm); 9] = [
    (&MD5_RSA, &SHA256),
    (&SHA1_RSA, &SHA256),
    (&SHA256_RSA, &SHA256),
    (&SHA384_RSA, &SHA384),
    (&SHA512_RSA, &SHA512),
    (&SHA1_ECDSA, &SHA256),
    (&SHA256_ECDSA, &SHA256),
    (&SHA384_ECDSA, &SHA384),
    (&SHA512_ECDSA, &SHA512),
];

/// \[DESIGN\] The hash functions RSASSA-PSS names, and the hash that binds
/// a certificate it signs with one [RFC 5929 §4.1]: SHA-256 for SHA-1.
static HASHES: [(&[u8], &Algorithm); 4] = [(&SHA1, &SHA256), (&SHA256_HASH, &SHA256), (&SHA384_HASH, &SHA384), (&SHA512_HASH, &SHA512)];

/// The channel binding `tls-server-end-point` of the TLS server whose
/// certificate, as DER, is `certificate` [RFC 5929 §4.1]: its hash; none
/// if its signature algorithm defines none, or it is not a certificate.
pub fn end_point(certificate: &[u8]) -> Option<Vec<u8>> {
    let (body, _) = expect(certificate, SEQUENCE)?;
    let (_, after) = expect(body, SEQUENCE)?;
    let (algorithm, parameters) = identifier(after)?;
    let hash = if algorithm == RSA_PSS {
        pss(parameters)?
    } else {
        SIGNATURES.iter().find(|(oid, _)| *oid == algorithm).map(|(_, hash)| *hash)?
    };
    Some(digest::digest(hash, certificate).as_ref().to_vec())
}

/// The algorithm and parameters of the AlgorithmIdentifier `der` begins
/// with.
fn identifier(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let (body, _) = expect(der, SEQUENCE)?;
    expect(body, OID)
}

/// The hash binding a certificate signed by RSASSA-PSS of `parameters`:
/// its hash function's, if its mask's MGF1 is of the same, so the
/// signature uses a single hash function; none else [RFC 5929 §4.1].
fn pss(parameters: &[u8]) -> Option<&'static Algorithm> {
    let (mut fields, _) = expect(parameters, SEQUENCE)?;
    let mut hash: &[u8] = &SHA1;
    if let Some((explicit, rest)) = expect(fields, PSS_HASH) {
        hash = identifier(explicit)?.0;
        fields = rest;
    }
    let mut mask: &[u8] = &SHA1;
    if let Some((explicit, _)) = expect(fields, PSS_MASK) {
        let (function, of) = identifier(explicit)?;
        mask = (function == MGF1).then_some(identifier(of)?.0)?;
    }
    if hash != mask {
        return None;
    }
    HASHES.iter().find(|(oid, _)| *oid == hash).map(|(_, binding)| *binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DER of `tag` around `body`, short of 128 bytes.
    fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        [&[tag, u8::try_from(body.len()).unwrap()][..], body].concat()
    }

    /// An AlgorithmIdentifier of `oid` and `parameters`.
    fn algorithm(oid: &[u8], parameters: &[u8]) -> Vec<u8> {
        tlv(SEQUENCE, &[tlv(OID, oid), parameters.to_vec()].concat())
    }

    /// A certificate's DER, signed by `signature`, an AlgorithmIdentifier.
    fn certificate(signature: &[u8]) -> Vec<u8> {
        tlv(SEQUENCE, &[tlv(SEQUENCE, &[0x02, 1, 7]), signature.to_vec(), tlv(0x03, &[0])].concat())
    }

    /// The hash of `der` by `hash`.
    fn hashed(hash: &'static Algorithm, der: &[u8]) -> Vec<u8> {
        digest::digest(hash, der).as_ref().to_vec()
    }

    /// RSASSA-PSS-params of the hash `hash` and the MGF1 hash `mask`, each
    /// if given.
    fn pss_of(hash: Option<&[u8]>, mask: Option<&[u8]>) -> Vec<u8> {
        let hash = hash.map(|h| tlv(PSS_HASH, &algorithm(h, &[0x05, 0]))).unwrap_or_default();
        let mask = mask.map(|m| tlv(PSS_MASK, &algorithm(&MGF1, &algorithm(m, &[0x05, 0])))).unwrap_or_default();
        algorithm(&RSA_PSS, &tlv(SEQUENCE, &[hash, mask].concat()))
    }

    /// Verifies: [DESIGN] RFC 5929 §4.1: a certificate signed with MD5 or
    /// SHA-1, by RSA or ECDSA, is bound by SHA-256 of its whole DER; with
    /// SHA-256, SHA-384 or SHA-512, by that hash.
    #[test]
    fn the_signature_hash_chooses_the_binding_hash() {
        let cases: [(&[u8], &'static Algorithm); 9] = [
            (&MD5_RSA, &SHA256),
            (&SHA1_RSA, &SHA256),
            (&SHA1_ECDSA, &SHA256),
            (&SHA256_RSA, &SHA256),
            (&SHA256_ECDSA, &SHA256),
            (&SHA384_RSA, &SHA384),
            (&SHA384_ECDSA, &SHA384),
            (&SHA512_RSA, &SHA512),
            (&SHA512_ECDSA, &SHA512),
        ];
        for (oid, hash) in cases {
            let der = certificate(&algorithm(oid, &[0x05, 0]));
            assert_eq!(end_point(&der), Some(hashed(hash, &der)), "{oid:?}");
        }
        let ecdsa = certificate(&algorithm(&SHA384_ECDSA, &[]));
        assert_eq!(end_point(&ecdsa).map(|h| h.len()), Some(48), "parameters absent, as ECDSA's are");
    }

    /// Verifies: [DESIGN] RFC 5929 §4.1, RFC 4055 §3.1: RSASSA-PSS is bound
    /// by its hash, SHA-256 for SHA-1 or the defaults, when its MGF1 is of
    /// the same hash; of two hashes, or a mask not MGF1, it has no binding.
    #[test]
    fn rsassa_pss_is_bound_by_its_one_hash() {
        let same = certificate(&pss_of(Some(&SHA384_HASH), Some(&SHA384_HASH)));
        assert_eq!(end_point(&same), Some(hashed(&SHA384, &same)));
        let defaults = certificate(&pss_of(None, None));
        assert_eq!(end_point(&defaults), Some(hashed(&SHA256, &defaults)), "SHA-1 by default");
        let sha1 = certificate(&pss_of(Some(&SHA1), None));
        assert_eq!(end_point(&sha1), Some(hashed(&SHA256, &sha1)));
        assert_eq!(end_point(&certificate(&pss_of(Some(&SHA256_HASH), None))), None, "two hashes");
        assert_eq!(end_point(&certificate(&pss_of(Some(&SHA512_HASH), Some(&SHA256_HASH)))), None, "two hashes");
        let other_mask = tlv(PSS_MASK, &algorithm(&SHA1_RSA, &algorithm(&SHA256_HASH, &[])));
        let masked = algorithm(&RSA_PSS, &tlv(SEQUENCE, &[tlv(PSS_HASH, &algorithm(&SHA256_HASH, &[])), other_mask].concat()));
        assert_eq!(end_point(&certificate(&masked)), None, "a mask not MGF1");
        assert_eq!(end_point(&certificate(&pss_of(Some(&MGF1), Some(&MGF1)))), None, "a hash not known");
        assert_eq!(end_point(&certificate(&algorithm(&RSA_PSS, &[]))), None, "no parameters");
    }

    /// Verifies: [DESIGN] RFC 5929 §4.1, RULES.md R-ERR-1: an algorithm of
    /// no hash, as Ed25519 (1.3.101.112), or not known, has no binding;
    /// nor has DER not of a certificate, never a panic.
    #[test]
    fn no_binding_without_one_known_hash() {
        assert_eq!(end_point(&certificate(&algorithm(&[0x2b, 0x65, 0x70], &[]))), None, "Ed25519");
        let whole = certificate(&algorithm(&SHA256_ECDSA, &[]));
        for cut in 0..whole.len() {
            assert_eq!(end_point(&whole[..cut]), None, "cut at {cut}");
        }
        assert_eq!(end_point(&certificate(&tlv(SEQUENCE, &[0x05, 0]))), None, "no algorithm");
    }
}
