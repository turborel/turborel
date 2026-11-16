//! Stored WKB accepts exactly the canonical writer's representations.

use super::*;
use crate::value::types::ExtensionValue;
use std::sync::Arc;

fn stored(bytes: &[u8]) -> Value {
    let mut payload = vec![1];
    payload.extend_from_slice(&17_i32.to_le_bytes());
    payload.extend_from_slice(bytes);
    Value::Extension(ExtensionValue { kind: &GEOMETRY, bytes: Arc::from(payload) })
}

/// The previous decoder's parse-and-rewrite rule is an independent
/// acceptance oracle; it also checks all ordinary representation errors.
fn equivalent(bytes: &[u8]) {
    let interchange = Geometry::from_wkb(bytes, 17);
    let expected = interchange.as_ref().is_ok_and(|g| g.to_wkb() == bytes);
    let decoded = Geometry::from_value(&stored(bytes));
    assert_eq!(decoded.is_ok(), expected, "{bytes:02x?}");
    match decoded {
        Ok(geometry) => {
            assert_eq!(geometry.srid(), 17_i32);
            assert_eq!(geometry.to_wkb(), bytes);
            assert_eq!(geometry.bounds(), interchange.unwrap().bounds());
        }
        Err(error) => assert_eq!(error.code, Code::Corrupt),
    }
}

/// Verifies: [DESIGN] D82: strict stored decoding has the same acceptance
/// rule as parsing then comparing the canonical writer, for all seven
/// kinds, empty components, every truncation and single-bit mutation.
#[test]
fn canonical_reader_equals_parse_and_rewrite() {
    for text in [
        "POINT (0 2.5)",
        "POINT EMPTY",
        "LINESTRING (0 0, 2 3)",
        "LINESTRING EMPTY",
        "POLYGON ((0 0, 4 0, 4 4, 0 0), (1 1, 2 1, 1 2, 1 1))",
        "POLYGON EMPTY",
        "MULTIPOINT ((0 2), EMPTY, (3 4))",
        "MULTIPOINT EMPTY",
        "MULTILINESTRING ((0 0, 1 1), EMPTY)",
        "MULTILINESTRING EMPTY",
        "MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)), EMPTY)",
        "MULTIPOLYGON EMPTY",
        "GEOMETRYCOLLECTION (POINT EMPTY, LINESTRING (0 0, 3 4), GEOMETRYCOLLECTION EMPTY)",
        "GEOMETRYCOLLECTION EMPTY",
    ] {
        let bytes = Geometry::from_wkt(text, 17).unwrap().to_wkb();
        equivalent(&bytes);
        for end in 0..bytes.len() {
            equivalent(&bytes[..end]);
            for bit in 0..8_u32 {
                let mut changed = bytes.clone();
                changed[end] ^= 1_u8.wrapping_shl(bit);
                equivalent(&changed);
            }
        }
        let mut extra = bytes;
        extra.push(0);
        equivalent(&extra);
    }
}

/// Verifies: [DESIGN] D82: little-endian is required independently at
/// every stored component; interchange accepts both byte orders.
/// MC/DC: strict/lax with XDR, strict with NDR, and invalid order tags.
#[test]
fn canonical_reader_rejects_big_endian_at_every_level() {
    let mut big = vec![0];
    big.extend_from_slice(&1_u32.to_be_bytes());
    big.extend_from_slice(&2.5_f64.to_be_bytes());
    big.extend_from_slice(&(-3.0_f64).to_be_bytes());
    for _ in 1..MAX_DEPTH {
        assert!(Geometry::from_wkb(&big, 17).is_ok());
        assert_eq!(Geometry::from_value(&stored(&big)).unwrap_err().code, Code::Corrupt);
        equivalent(&big);
        let mut parent = vec![1, 7, 0, 0, 0, 1, 0, 0, 0];
        parent.extend_from_slice(&big);
        big = parent;
    }
    equivalent(&big);
}

/// Verifies: [DESIGN] D82: signed zero is normalized on interchange and
/// rejected in stored bytes on either axis of points, lines and rings.
/// MC/DC: positive zero and nonzero are accepted, negative zero refused
/// only when the reader's canonical mode is enabled.
#[test]
fn canonical_reader_rejects_signed_zero_in_every_coordinate() {
    for (text, start, coordinates) in [
        ("POINT (0 0)", 5_usize, 2_usize),
        ("LINESTRING (0 0, 0 0)", 9_usize, 4_usize),
        ("POLYGON ((0 0, 0 0, 0 0, 0 0))", 13_usize, 8_usize),
    ] {
        let bytes = Geometry::from_wkt(text, 17).unwrap().to_wkb();
        equivalent(&bytes);
        for coordinate in 0..coordinates {
            let mut changed = bytes.clone();
            changed[start.saturating_add(coordinate.saturating_mul(8)).saturating_add(7)] = 0x80;
            assert!(Geometry::from_wkb(&changed, 17).is_ok());
            assert_eq!(Geometry::from_value(&stored(&changed)).unwrap_err().code, Code::Corrupt);
            equivalent(&changed);
        }
    }
}

/// Verifies: [DESIGN] D82: EMPTY accepts any NaN pair on interchange but
/// only the writer's quiet NaN pair in storage. Either ordinate alone
/// determines rejection; payload, sign and signalling bits are covered.
/// MC/DC: canonical and other NaNs in both reader modes vary the two
/// conditions of the empty point's canonical-representation check.
#[test]
fn canonical_reader_rejects_alternate_empty_nan_bits() {
    let canonical = 0x7ff8_0000_0000_0000_u64;
    for alternate in [canonical, 0x7ff8_0000_0000_0001_u64, 0xfff8_0000_0000_0000_u64, 0x7ff0_0000_0000_0001_u64] {
        for pair in [[alternate, canonical], [canonical, alternate], [alternate, alternate]] {
            let mut bytes = vec![1, 1, 0, 0, 0];
            for bits in pair {
                bytes.extend_from_slice(&bits.to_le_bytes());
            }
            assert!(Geometry::from_wkb(&bytes, 17).unwrap().is_empty());
            assert_eq!(Geometry::from_value(&stored(&bytes)).is_ok(), alternate == canonical);
            equivalent(&bytes);
        }
    }
}

/// Verifies: [DESIGN] D53, D82: strict validation retains representation
/// limits and corruption categories at value, record, key and wire boundaries.
#[test]
fn canonical_reader_preserves_limits_and_boundary_errors() {
    let mut bytes = Geometry::point(-f64::from_bits(1), f64::MAX, 17).unwrap().to_wkb();
    for _ in 1..MAX_DEPTH {
        let mut parent = vec![1, 7, 0, 0, 0, 1, 0, 0, 0];
        parent.extend_from_slice(&bytes);
        bytes = parent;
    }
    equivalent(&bytes);
    let mut deep = vec![1, 7, 0, 0, 0, 1, 0, 0, 0];
    deep.extend_from_slice(&bytes);
    equivalent(&deep);
    equivalent(&vec![0; MAX_BYTES.saturating_add(1)]);
    let mut signed = Geometry::point(0.0, 0.0, 17).unwrap().to_wkb();
    signed[12] = 0x80;
    let value = stored(&signed);
    let Value::Extension(raw) = &value else { panic!("stored geometry") };
    assert!(GEOMETRY.decode_record(&raw.bytes, &data_type()).is_none());
    let mut framed = Vec::new();
    GEOMETRY.put_value(&value, &mut framed);
    assert_eq!(GEOMETRY.value(&mut Reader::new(&framed, "test")).unwrap_err().code, Code::Corrupt);
    assert_eq!(GEOMETRY.value(&mut Reader::received(&framed, "test")).unwrap_err().code, Code::Connection);
    let mut key = Vec::new();
    crate::value::Binary::new(raw.bytes.to_vec()).put_key(&mut key);
    assert_eq!(GEOMETRY.decode_key(&mut key.as_slice(), &data_type()).unwrap_err().code, Code::Corrupt);
}
