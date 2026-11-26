//! Unit tests of approximate numbers as decimals (`alp.rs`, `decode.rs`).

use super::*;

/// The factors e and f.
fn factors(e: u8, f: u8) -> Factors {
    Factors { e, f }
}

/// The values a chunk of `fields`, their factors chosen as a chunk's are,
/// reads back: one at a time and of the range `from` to `to` at once.
fn read_back(fields: &[Option<Vec<u8>>], (from, to): (usize, usize)) -> (Vec<Option<u64>>, Vec<u64>) {
    let mut trials = Trials::default();
    fields.iter().flatten().for_each(|f| trials.add(f));
    let ((chosen, least, bits), bytes) = trials.best(fields.len()).unwrap();
    let mut out = Vec::new();
    let records = crate::engine::columns::tests::records_of(fields);
    encode(Cells::whole(&records), (chosen, least, bits), &mut out).unwrap();
    assert_eq!(out.len(), bytes, "sized as written");
    let alp = Alp::parse(&out, fields.len()).unwrap();
    let one = (0..fields.len()).map(|n| fields[n].as_ref().map(|_| u64::from_le_bytes(alp.field(n).unwrap()))).collect();
    let mut words = Vec::new();
    alp.words((from, to), &mut words, &crate::kernels::Portable).unwrap();
    (one, words.into_iter().map(|w| u64::from_le_bytes(w.to_le_bytes())).collect())
}

fn field(v: f64) -> Vec<u8> {
    v.to_bits().to_le_bytes().to_vec()
}

/// Verifies: [DESIGN] RULES.md D31, ALP: integers convert to and from
/// approximate numbers exactly up to 2^53, negative ones too, and no
/// further; a number rounds away from zero, as `f64::round`.
#[test]
fn integers_convert_exactly() {
    for n in [0_i64, 1, -1, 1234, -987_654_321, EXACT, -EXACT, (1_i64 << 40_i32) + 7] {
        let f = exact(n).unwrap();
        assert_eq!(rounded(f), Some(n), "{n}");
    }
    assert_eq!(exact(EXACT + 1), None);
    assert_eq!(rounded(9.1e15), None, "past 2^53");
    assert_eq!(rounded(-2.5), Some(-3), "away from zero, as f64::round");
}

/// Verifies: [DESIGN] RULES.md D353, ALP: a decimal of few digits is
/// n × 10^f × 10^-e, of the least e and f that give it back,
/// 0.30000000000000004 among them, which is 3 × 0.1 multiplied; -0, the
/// greatest and the least normal number, the infinities and a NaN are not
/// of any factors.
#[test]
fn numbers_are_decimals_exactly() {
    assert_eq!(encoded(12.34, factors(2, 0)), Some(1234));
    assert_eq!(encoded(12.34, factors(1, 0)), None);
    assert_eq!(encoded(-0.5, factors(1, 0)), Some(-5));
    assert_eq!(encoded(1200.0, factors(2, 2)), Some(1200), "f takes off as many digits as e adds");
    assert_eq!(encoded(1200.0, factors(0, 0)), Some(1200));
    assert_eq!(encoded(-0.0, factors(0, 0)), None, "-0 is kept as it is");
    assert_eq!(encoded(0.1 + 0.2, factors(1, 0)), Some(3), "0.30000000000000004 is 3 × 0.1 as multiplied");
    for odd in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MAX, f64::MIN_POSITIVE] {
        assert_eq!(pairs().find_map(|p| encoded(odd, p)), None, "{odd}");
    }
    assert_eq!(pairs().count() * 2, EXPONENTS * (EXPONENTS + 1), "f at most e");
}

/// Verifies: [DESIGN] RULES.md D353, ALP decoded by multiplication: every
/// n a pair of factors keeps reads back as the value it was made of, bit
/// for bit, by the decoder's multiplication, of decimals of 1 to 18 digits
/// and of every pair; and a value the multiplication would not give back,
/// as 0.3 of n 3 and e 1 (3 × 0.1 is 0.30000000000000004), is not kept of
/// those factors.
#[test]
fn factors_give_back_their_values() {
    let mut kept = 0_usize;
    for digits in 1_u32..=18_u32 {
        for k in 0_i64..40_i64 {
            let n = k.wrapping_mul(7_919).wrapping_add(13) % 10_i64.pow(digits.min(15));
            for p in pairs() {
                let v = exact(n).unwrap() * FRACTIONS[usize::from(p.e)];
                if let Some(m) = encoded(v, p) {
                    assert_eq!(p.scale().unwrap().of(exact(m).unwrap()).to_bits(), v.to_bits(), "{v} of {p:?}");
                    kept += 1;
                }
            }
        }
    }
    assert!(kept > 10_000, "most are kept: {kept}");
    assert_eq!(encoded(0.3, factors(1, 0)), None, "3 × 0.1 is not 0.3");
    assert!(pairs().any(|p| encoded(0.3, p).is_some()), "0.3 is kept of other factors");
}

/// Verifies: [DESIGN] RULES.md D353, ALP decoded by multiplication: a
/// chunk of prices, nulls, exceptions (-0, NaN, an infinity, the least
/// and greatest numbers, a third) and negative values among them, reads
/// back bit for bit one at a time and of every range at once; a chunk of
/// n past 2^51, read checked, too.
#[test]
fn chunks_of_decimals_read_back_bit_for_bit() {
    let odd = [-0.0_f64, f64::NAN, f64::INFINITY, f64::MIN, f64::MAX, 1.0_f64 / 3.0_f64, f64::MIN_POSITIVE, -1e-300_f64];
    let mut fields: Vec<Option<Vec<u8>>> = (0_i32..700_i32)
        .map(|n| if n % 37_i32 == 5_i32 { None } else { Some(field(f64::from(n * 131_i32 % 20_000_i32 - 9_000_i32) / 100.0)) })
        .collect();
    for (at, v) in odd.iter().enumerate() {
        fields[at * 61 + 3] = Some(field(*v));
    }
    let expected: Vec<Option<u64>> = fields.iter().map(|f| f.as_ref().map(|b| u64::from_le_bytes(b[..].try_into().unwrap()))).collect();
    for (from, to) in [(0, 700), (3, 4), (60, 200), (63, 129), (699, 700), (5, 5)] {
        let (one, words) = read_back(&fields, (from, to));
        assert_eq!(one, expected, "one at a time");
        let some: Vec<u64> = expected[from..to].iter().zip(&words).map(|(e, w)| e.unwrap_or(*w)).collect();
        assert_eq!(words, some, "{from} to {to}");
    }
    let large: Vec<Option<Vec<u8>>> = (0_i32..200_i32).map(|n| Some(field(4e15 + f64::from(n) * 3.0))).collect();
    let (one, words) = read_back(&large, (0, 200));
    assert_eq!(one.into_iter().map(Option::unwrap).collect::<Vec<_>>(), words, "past 2^51");
}

/// Verifies: [DESIGN] RULES.md D353, ALP decoded by multiplication: a
/// property sweep of fixed seeds, of decimals of random digits and
/// exponents, and of random bits, reads back bit for bit.
#[test]
fn decimals_sweep_of_fixed_seeds() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state ^= state << 13_u32;
        state ^= state >> 7_u32;
        state ^= state << 17_u32;
        state
    };
    for _ in 0_u32..60_u32 {
        let e = usize::try_from(next() % 19).unwrap();
        let span = i64::try_from(next() % 1_000_000_000_000).unwrap() + 1;
        let fields: Vec<Option<Vec<u8>>> = (0_u32..300_u32)
            .map(|_| match next() % 10 {
                0 => None,
                1 => Some(field(f64::from_bits(next()))),
                _ => Some(field(
                    exact(i64::try_from(next() % 2_000_000_000_000).unwrap() % span - span.checked_div(2).unwrap()).unwrap() / POWERS[e],
                )),
            })
            .collect();
        let expected: Vec<Option<u64>> = fields.iter().map(|f| f.as_ref().map(|b| u64::from_le_bytes(b[..].try_into().unwrap()))).collect();
        let (one, words) = read_back(&fields, (0, 300));
        assert_eq!(one, expected);
        let some: Vec<u64> = expected.iter().zip(&words).map(|(x, w)| x.unwrap_or(*w)).collect();
        assert_eq!(words, some);
    }
}

/// Verifies: [DESIGN] RULES.md D353, ALP's header bit: a chunk written
/// before the factors were, its first byte an exponent alone, is read by
/// division, each value n / 10^e as it was written, one at a time and at
/// once; first bytes of an exponent above 18, of f above e, and too short,
/// are refused with SQLCODE -916.
#[test]
fn chunks_before_the_factors_read_by_division() {
    let mut old = vec![1_u8];
    old.extend_from_slice(&0_u32.to_le_bytes());
    old.push(4);
    old.extend_from_slice(&(-3_i64).to_le_bytes());
    // n of 3 values: -3 + 0, -3 + 6, -3 + 15: -0.3, 0.3, 1.2.
    old.extend_from_slice(&[0x60_u8, 0x0f_u8]);
    let alp = Alp::parse(&old, 3).unwrap();
    let values: Vec<f64> = (0_usize..3_usize).map(|n| f64::from_bits(u64::from_le_bytes(alp.field(n).unwrap()))).collect();
    assert_eq!(values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), [-0.3_f64, 0.3_f64, 1.2_f64].map(f64::to_bits));
    let mut words = Vec::new();
    alp.words((0, 3), &mut words, &crate::kernels::Portable).unwrap();
    assert_eq!(
        words.iter().map(|&w| u64::from_le_bytes(w.to_le_bytes())).collect::<Vec<_>>(),
        [-0.3_f64, 0.3_f64, 1.2_f64].map(f64::to_bits)
    );
    let refused = |bytes: &[u8]| Alp::parse(bytes, 0).err().map(|e| e.code);
    assert_eq!(refused(&[19, 0, 0, 0, 0, 0]), Some(crate::error::Code::Corrupt), "an exponent of 19");
    assert_eq!(refused(&[FACTORED | 0x02, 3, 0, 0, 0, 0]), Some(crate::error::Code::Corrupt), "f above e");
    assert_eq!(refused(&[FACTORED | 0x13, 0, 0, 0, 0, 0]), Some(crate::error::Code::Corrupt), "e of 19");
    assert_eq!(refused(&[FACTORED | 0x02]), Some(crate::error::Code::Corrupt), "no f");
    assert_eq!(refused(&[]), Some(crate::error::Code::Corrupt), "empty");
    let mut wrong = Vec::new();
    let one = crate::engine::columns::tests::records_of(&[Some(field(1.0))]);
    assert!(encode(Cells::whole(&one), (factors(2, 3), 0, 0), &mut wrong).is_err(), "f above e is not written");
}
