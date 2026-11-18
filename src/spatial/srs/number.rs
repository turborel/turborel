//! Numbers in the text representations: the \<exact numeric literal\> and
//! \<approximate numeric literal\> of ISO/IEC 9075-2 §5.3 that §14.1.9,
//! §16.1.21 and §16.2.23 of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! reference, and the rounded decimal strings of ST_String and the
//! bearing and azimuth methods.

/// The decimal radix of the literals.
const RADIX: u32 = 10;
/// The largest magnitude an `f64` holds as an exact integer, 2^53.
const EXACT: f64 = 9_007_199_254_740_992.0;

/// The length of the <signed numeric literal> at the start of `text`, or
/// none if it does not start with one. A sign is accepted because
/// longitudes and parameters are negative in practice (§14.1.9 reading);
/// `<unsigned integer> [ <period> [ <unsigned integer> ] ]`,
/// `<period> <unsigned integer>` and an `E` exponent follow §5.3.
pub(super) fn literal_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(bytes, at);
    at = at.checked_add(whole)?;
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        at = at.checked_add(1)?;
        fraction = digits(bytes, at);
        at = at.checked_add(fraction)?;
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        let signed = usize::from(matches!(bytes.get(at.checked_add(1)?), Some(b'+' | b'-')));
        let start = at.checked_add(1)?.checked_add(signed)?;
        let exponent = digits(bytes, start);
        if exponent == 0 {
            return None;
        }
        at = start.checked_add(exponent)?;
    }
    Some(at)
}

/// How many decimal digits `bytes` holds from `from`.
fn digits(bytes: &[u8], from: usize) -> usize {
    bytes.get(from..).map_or(0, |rest| rest.iter().take_while(|b| b.is_ascii_digit()).count())
}

/// The finite value of the literal `text`, which [`literal_length`]
/// accepted whole.
pub(super) fn value(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `value` rounded to `places` decimal places, as text without a negative
/// zero. Rounding, not truncation, is the implementation-defined choice
/// of §16.1.8 Description 2 and 4 and §16.2.8 to §16.2.16 Description 5.
pub(super) fn fixed(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.starts_with('-') && text.bytes().all(|b| matches!(b, b'-' | b'0' | b'.')) {
        return text.chars().skip(1).collect();
    }
    text
}

/// The shortest decimal text that reads back as `value`, as written in the
/// well-known text representations. Negative zero is written as zero.
pub(super) fn shortest(value: f64) -> String {
    format!("{}", value + 0.0_f64)
}

/// The integer `value`, whose magnitude must be below 2^53, or none.
pub(super) fn whole(value: f64) -> Option<i64> {
    if value.is_nan() || value.abs() >= EXACT || value.fract().abs() > 0.0_f64 {
        return None;
    }
    let text = format!("{value:.0}");
    i64::from_str_radix(&text, RADIX).ok()
}
