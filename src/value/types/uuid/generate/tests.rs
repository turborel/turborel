use super::*;
use crate::value::types::uuid::version;

/// The variant bits of a UUID's octets, the top two of octet 8.
fn variant(u: &[u8; UUID_OCTETS]) -> u8 {
    u[8] >> 6_u8
}

/// Verifies: [EXTENSION] RULES.md D182 [RFC 9562 §5.4]: a version 4
/// UUID has version 4 and variant `10` whatever its bits, and keeps the
/// other 122 bits as drawn.
#[test]
fn random_uuids_carry_version_and_variant() {
    for bits in [0_u128, u128::MAX, 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef] {
        let u = random_uuid(bits);
        assert_eq!((version(&u), variant(&u)), (4, 0b10), "{bits:x}");
        let kept = !(0xf_u128 << 76_u32) & !(0b11_u128 << 62_u32);
        assert_eq!(u128::from_be_bytes(u) & kept, bits & kept);
    }
}

/// Verifies: [EXTENSION] RULES.md D182: UUID_V4 draws a new value each
/// call, so a thousand calls give a thousand UUIDs, each of version 4.
#[test]
fn random_uuids_differ() {
    let made: std::collections::BTreeSet<[u8; UUID_OCTETS]> = (0..1000_u32).map(|_| random_uuid(random_bits())).collect();
    assert_eq!(made.len(), 1000);
    assert!(made.iter().all(|u| version(u) == 4 && variant(u) == 0b10));
}

/// Verifies: [EXTENSION] RULES.md D183 [RFC 9562 §5.7]: a version 7
/// UUID begins with its 48 bits of milliseconds, has version 7 and
/// variant `10`, and carries its 74 random bits around them.
#[test]
fn ordered_uuids_lay_out_time_and_bits() {
    let payload = next_payload(0, 1_700_000_000_123_456, u128::MAX);
    let u = ordered_uuid(payload);
    assert_eq!((version(&u), variant(&u)), (7, 0b10));
    let millis = u64::from_be_bytes([0, 0, u[0], u[1], u[2], u[3], u[4], u[5]]);
    assert_eq!(millis, 1_700_000_000_123);
    assert_eq!((u[6] & 0x0f, u[7], u[8] & 0x3f), (0x0f, 0xff, 0x3f), "every random bit kept");
    assert_eq!(ordered_uuid(next_payload(0, 1_700_000_000_123_456, 0))[6..], [0x70, 0, 0x80, 0, 0, 0, 0, 0, 0, 0]);
}

/// Verifies: [EXTENSION] RULES.md D183 [RFC 9562 §6.2]: each payload
/// exceeds the last, within one millisecond, as the clock goes back and
/// as the random bits run out; a later millisecond starts afresh, and an
/// instant before 1970 counts as 0.
#[test]
fn ordered_payloads_increase() {
    let at = 1_700_000_000_123_456_i64;
    let first = next_payload(0, at, 5);
    assert_eq!(next_payload(first, at, 4), first + 1, "the same millisecond, lower bits");
    assert_eq!(next_payload(first, at - 5_000, u128::MAX), first + 1, "the clock gone back");
    let later = next_payload(first, at + 1_000, 0);
    assert!(later > first && later >> RANDOM_BITS == (first >> RANDOM_BITS) + 1);
    assert_eq!(next_payload(0, -5_000, 3), 3);
    let ordered = Ordered::default();
    let made: Vec<[u8; UUID_OCTETS]> = (0..100_u32).map(|_| ordered.next(at).unwrap()).collect();
    assert!(made.windows(2).all(|w| w[0] < w[1]));
}

/// Verifies: [EXTENSION] RULES.md D182, D183: each function takes no
/// arguments, gives a UUID, is never deterministic, and makes values
/// with no database; UUID_V7 alone reads its statement's clock.
#[test]
fn functions_are_typed_and_volatile() {
    let all = functions();
    let names: Vec<&str> = all.iter().map(|f| f.name()).collect();
    assert_eq!(names, [UUID_V4, UUID_V7]);
    for f in &all {
        assert!(!f.deterministic());
        assert_eq!(f.return_type(&[]).unwrap(), UuidType::data_type());
        assert_eq!(f.return_type(&[DataType::Boolean]).unwrap_err().code, Code::SyntaxRule);
    }
    assert!(!all[0].uses_context(0) && all[1].uses_context(0));
    let v4 = super::super::octets_of(&all[0].call(&[]).unwrap()).unwrap();
    let v7 = super::super::octets_of(&all[1].call(&[]).unwrap()).unwrap();
    assert_eq!((version(&v4), version(&v7)), (4, 7));
}
