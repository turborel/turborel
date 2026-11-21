use super::*;

/// DECIMAL(`precision`, `scale`).
fn decimal(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Decimal }
}

/// The units of an exact value.
fn units(v: &Value) -> Option<(i128, u32)> {
    if let Value::Exact(d) = v { Some((d.units, d.scale)) } else { None }
}

/// 10^38 - 1, the greatest units of 38 digits.
const MOST: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

/// Verifies: [DESIGN] RULES.md D698: a type of up to 18 digits is kept in
/// 8 bytes, a wider one in 16, and either reads back what it wrote, a
/// value of a smaller scale at the type's; a narrow type refuses a value
/// past 64 bits.
#[test]
fn wide_types_keep_16_bytes() {
    let (narrow, wide) = (decimal(18, 2), decimal(38, 4));
    assert_eq!((EXACT.record_width(&narrow), EXACT.record_width(&wide)), (Some(8), Some(16)));
    assert_eq!((narrow_scale(&narrow), narrow_scale(&wide)), (Some(2), None));
    for (t, v) in
        [(&wide, Decimal::new(MOST, 4)), (&wide, Decimal::new(-MOST, 4)), (&wide, Decimal::new(-15, 1)), (&narrow, Decimal::new(-15, 1))]
    {
        let field = EXACT.encode_record(&Value::Exact(v), t).unwrap();
        assert_eq!(field.len(), EXACT.record_width(t).unwrap());
        let read = EXACT.decode_record(&field, t).unwrap();
        assert_eq!(EXACT.compare(&read, &Value::Exact(v)), Some(Ordering::Equal), "{v}");
    }
    let past = Value::Exact(Decimal::new(i128::from(i64::MAX) + 1, 2));
    assert_eq!(EXACT.encode_record(&past, &narrow).err().map(|e| e.code), Some(Code::Api));
    assert!(EXACT.encode_record(&past, &wide).is_ok());
}

/// Verifies: [DESIGN] RULES.md D698: the keys of a wide type order as
/// its values do, negative ones first, and read back their values.
#[test]
fn wide_keys_order_as_values() {
    let t = decimal(38, 0);
    let values = [-MOST, -(1_i128 << 70), -1, 0, 1, 1_i128 << 70, MOST];
    let keys: Vec<Vec<u8>> = values
        .iter()
        .map(|&u| {
            let mut key = Vec::new();
            EXACT.encode_key(&Value::Exact(Decimal::new(u, 0)), &t, &mut key).unwrap();
            key
        })
        .collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "keys in the values' order");
    for (key, &u) in keys.iter().zip(&values) {
        let mut rest = key.as_slice();
        assert_eq!(units(&EXACT.decode_key(&mut rest, &t).unwrap()), Some((u, 0)));
        assert!(rest.is_empty(), "16 bytes taken");
    }
}

/// Verifies: [DESIGN] RULES.md D698: a wide column's anchor is the value
/// rounded down at its scale in 128 bits, exact only of a value of the
/// scale; a narrow one's is clamped to 64 bits.
#[test]
fn wide_anchors_are_kept_in_128_bits() {
    let anchor = |v: Decimal, t: &DataType| EXACT.anchor(&Value::Exact(v), t).map(|(a, exact)| (units(&a), exact));
    assert_eq!(anchor(Decimal::new(MOST, 0), &decimal(38, 0)), Some((Some((MOST, 0)), true)));
    assert_eq!(anchor(Decimal::new(-15, 1), &decimal(30, 0)), Some((Some((-2, 0)), false)));
    assert_eq!(anchor(Decimal::new(MOST, 0), &decimal(18, 0)), Some((Some((i128::from(i64::MAX), 0)), false)));
}
