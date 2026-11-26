//! Unit tests: the literals a cast reads and writes, read where they
//! lie, against the text a reading of them made before (RULES.md
//! R-TEST-1, D1148).

use super::*;

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3, §6.10 GR3b, GR5b i] what is a
/// `<signed numeric literal>`, and the shortest approximate literal;
/// [ISO/IEC 9075-2:2023 §5.3 SR22] its digits with underscores between them.
///
/// `placed`'s decision, MC/DC pairs against "1_0", true: "_1" shows
/// the first underscore, "1_" the last, "1__0" two together, "A1" a
/// character neither digit nor underscore.
#[test]
fn numeric_literals() {
    let literal_value = |s: &str| literal_parts(s).and_then(|lit| literal_value(&lit));
    let exact = |s: &str| if let Some(Value::Exact(d)) = literal_value(s) { Some((d.units, d.scale)) } else { None };
    assert_eq!(exact("-12.50"), Some((-1250, 2)));
    assert_eq!(exact(".5"), Some((5, 1)));
    let Some(Value::Approx(x)) = literal_value("+1E-2") else { panic!("+1E-2 is approximate") };
    assert!((x - 0.01_f64).abs() < 1e-12_f64);
    assert_eq!(exact("1_000.000_5"), Some((10_000_005, 4)), "underscores between digits");
    let Some(Value::Approx(y)) = literal_value("5.E1_0") else { panic!("5.E1_0 is approximate") };
    assert!((y - 5e10_f64).abs() < 1.0_f64);
    for bad in ["", ".", "1.2.3", "E1", "1E", "1E+", "A1", "--1", "_1", "1_", "1__0", "1_.5", "1._5", "1E_1", "_"] {
        assert!(literal_value(bad).is_none(), "{bad}");
    }
    assert_eq!(approximate_literal(0.0), "0E0");
    assert_eq!(approximate_literal(100.0), "1.0E2");
    assert_eq!(approximate_literal(-1.5e-3), "-1.5E-3");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 GR8b, GR9b] RULES.md D970: a
/// literal's text no number holds is 22003, one that is no literal
/// 22018. Its leading zeros count toward no limit; a fraction past what
/// its whole digits leave of the maximum precision is truncated; whole
/// digits more than the maximum precision make the nearest approximate
/// number, and past every approximate number's range, none.
#[test]
fn literals_no_exact_number_holds() {
    let exact = |s: &str| if let Ok(Value::Exact(d)) = numeric_literal(s) { Some((d.units, d.scale)) } else { None };
    let state = |s: &str| numeric_literal(s).unwrap_err().sqlstate().to_string();
    assert_eq!(exact(&format!("{}1", "0".repeat(60))), Some((1, 0)));
    assert_eq!(exact(&format!("-0.{}", "3".repeat(60))), Some((-"3".repeat(38).parse::<i128>().unwrap(), 38)));
    let whole = "9".repeat(36);
    assert_eq!(exact(&format!("{whole}.125")), Some((pow10(38).unwrap() - 100 + 12, 2)), "two digits of the fraction left");
    let Ok(Value::Approx(x)) = numeric_literal(&"9".repeat(39)) else { panic!("39 digits are approximate") };
    assert!((x - 1e39_f64).abs() < 1e24_f64);
    for beyond in ["9".repeat(400), "1E400".to_owned(), "-1e999".to_owned()] {
        assert_eq!(state(&beyond), "22003", "{beyond}");
    }
    assert_eq!(state("1E"), "22018");
}

/// The digits of `t` without underscores, as the reading that made text
/// of them read them.
fn reference_digits(t: &str) -> Option<String> {
    let placed = !t.starts_with('_') && !t.ends_with('_') && !t.contains("__");
    (placed && t.chars().all(|c| c.is_ascii_digit() || c == '_')).then(|| t.replace('_', ""))
}

/// A literal's sign, digits, fraction digits, exponent and value.
type Reference = (bool, String, usize, Option<i64>, Option<Value>);

/// `s` read, and its value, as the reading that made text of its digits
/// read it: the reference the reading where they lie must equal.
fn reference(s: &str) -> Option<Reference> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (mantissa, exponent) = body.split_once(['E', 'e']).map_or((body, None), |(m, e)| (m, Some(e)));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let (whole, fraction) = (reference_digits(whole)?, reference_digits(fraction)?);
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let exponent = match exponent {
        Some(e) => {
            let e = e.strip_prefix('+').unwrap_or(e);
            let (minus, unsigned) = e.strip_prefix('-').map_or((false, e), |u| (true, u));
            let unsigned = reference_digits(unsigned).filter(|u| !u.is_empty())?;
            let trimmed = unsigned.trim_start_matches('0');
            let magnitude = trimmed.parse::<i64>().unwrap_or(if trimmed.is_empty() { 0 } else { i64::MAX });
            Some(if minus { magnitude.saturating_neg() } else { magnitude })
        }
        None => None,
    };
    let signed = |x: f64| Value::Approx(if negative { -x } else { x });
    let value = if let Some(e) = exponent {
        format!("{whole}.{fraction}E{e}").parse().ok().filter(|x: &f64| x.is_finite()).map(signed)
    } else {
        let whole = whole.trim_start_matches('0');
        match 38_usize.checked_sub(whole.len()) {
            None => format!("{whole}.{fraction}").parse().ok().filter(|x: &f64| x.is_finite()).map(signed),
            Some(room) => {
                let kept = fraction.get(..fraction.len().min(room))?;
                let units: i128 = format!("0{whole}{kept}").parse().ok()?;
                Some(Value::Exact(Decimal::new(if negative { units.checked_neg()? } else { units }, u32::try_from(kept.len()).ok()?)))
            }
        }
    };
    Some((negative, format!("{whole}{fraction}"), fraction.len(), exponent, value))
}

/// The bits of a value read, so approximate ones compare exactly.
fn bits(v: Option<Value>) -> Option<(i128, u32, u64)> {
    let v = v?;
    if let Value::Exact(d) = v {
        return Some((d.units, d.scale, 0));
    }
    let Value::Approx(x) = v else { panic!("a literal's value is a number, not {v}") };
    Some((0, u32::MAX, x.to_bits()))
}

/// Every text the sweep reads: signs, digits, periods, exponents and
/// underscores in many places, long digit runs and spaces.
fn swept() -> Vec<String> {
    let mut texts: Vec<String> = [
        "1E+-5",
        "1e-+5",
        "1_0E1_0",
        "0_0.0_0",
        "1E0000000000000000000000000005",
        "1E99999999999999999999",
        "-1E-99999999999999999999",
        "1.e5",
        ".5e-3",
        "-0",
        "+0.000",
        "00012.3400",
        "1_2_3.4_5",
        "  12  ",
        "1 2",
        "١٢",
        "1E",
        "1e+",
        "e5",
        ".",
        "-.",
        "+-1",
        "--1",
        "1..2",
        "1.2.3",
        "_1",
        "1_",
        "1__2",
        "1._2",
        "1_.2",
        "0x10",
        "inf",
        "NaN",
        "1E1.5",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    texts.extend(generated());
    texts
}

/// Texts of long digit runs, and texts of the sweep's characters made
/// from a fixed seed.
fn generated() -> Vec<String> {
    let mut texts = Vec::new();
    for n in [36_usize, 37, 38, 39, 40, 60, 400] {
        texts.push("9".repeat(n));
        texts.push(format!("0.{}", "7".repeat(n)));
        texts.push(format!("-{}.{}", "1".repeat(n.div_ceil(2)), "3".repeat(n)));
        texts.push(format!("{}.{}E-{n}", "4".repeat(n), "5".repeat(n)));
        texts.push(format!("{}_{}", "2".repeat(n), "8".repeat(n)));
    }
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..4000_u32 {
        x ^= x.wrapping_shl(13);
        x ^= x.wrapping_shr(7);
        x ^= x.wrapping_shl(17);
        let alphabet = b"0123456789.-+eE_ 09";
        let len = (x % 12).wrapping_add(1);
        let text: String = (0..len)
            .map(|k| char::from(alphabet[usize::try_from(x.wrapping_shr(u32::try_from(k.wrapping_mul(5)).unwrap()) % 19).unwrap()]))
            .collect();
        texts.push(text);
    }
    texts
}

/// Verifies: [DESIGN] RULES.md D1148: a `<signed numeric literal>` read
/// where it lies, with no text made of its digits, reads as the reading
/// that made text of them did: the same parts, digits gathered for
/// DECFLOAT, and the same value, exact or approximate, to the bit; for
/// every text of a sweep, literals and not.
#[test]
fn a_literal_read_where_it_lies_reads_as_one_made_text() {
    for text in swept() {
        let read = numeric_parts(&text).map(|lit| (lit.negative, lit.digits, lit.fraction, lit.exponent));
        let expected = reference(&text);
        let parts = expected.as_ref().map(|(n, d, f, e, _)| (*n, d.clone(), *f, *e));
        assert_eq!(read, parts, "parts of {text:?}");
        let value = literal_parts(&text).and_then(|lit| literal_value(&lit));
        assert_eq!(bits(value), bits(expected.and_then(|e| e.4)), "value of {text:?}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.3 SR22] [ISO/IEC 9075:1992 §6.10
/// GR3b] an approximate literal with underscores among its digits, which
/// `f64` parsing does not take, is read from its digits gathered: the
/// same number as without them; and "1E+-5", whose exponent's two signs
/// parsing does not take either, as 1E-5.
#[test]
fn a_literal_parsing_does_not_take_is_read_from_its_digits() {
    let approx = |s: &str| if let Ok(Value::Approx(x)) = numeric_literal(s) { x.to_bits() } else { panic!("{s} is approximate") };
    assert_eq!(approx("1_2.5_0E1_0"), approx("12.50E10"));
    assert_eq!(approx("1E+-5"), approx("1E-5"));
    assert_eq!(approx(&format!("{}_9", "9".repeat(40))), approx(&"9".repeat(41)));
    assert_eq!(approx(" -2.5e3 "), (-2500.0_f64).to_bits());
}
