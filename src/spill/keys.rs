//! The byte keys of spilled rows: the same bytes exactly when the
//! operator would find the keys equal (`spill.rs`). Split from spill.rs
//! (RULES.md R-SIZE-2).

use crate::value::{Bucket, Decimal, NormKey, Value};

/// The key of buckets `key`: the same bytes exactly when the buckets are.
pub fn bucket_key(key: &[Bucket]) -> Vec<u8> {
    let mut out = Vec::new();
    for b in key {
        match b {
            Bucket::Char(s) => text(&mut out, 0, s),
            Bucket::Number(bits) => {
                out.push(1);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Bucket::Instant(utc) => {
                out.push(4);
                out.extend_from_slice(&utc.to_le_bytes());
            }
            Bucket::Span(units) => {
                out.push(5);
                out.extend_from_slice(&units.to_le_bytes());
            }
            Bucket::Bits(held) => {
                out.push(3);
                out.extend_from_slice(&u64::try_from(held.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(held);
            }
            Bucket::Extension(bytes) => {
                out.push(2);
                out.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// The tags of a normalized key's values (`norm_key`).
const NULL_TAG: u8 = 0;
const CHAR_TAG: u8 = 1;
const EXACT_TAG: u8 = 2;
const APPROX_TAG: u8 = 3;
const EXTENSION_TAG: u8 = 4;
const BITS_TAG: u8 = 5;
const INSTANT_TAG: u8 = 6;
const SPAN_TAG: u8 = 7;

/// Appends `v`'s normalized key to `out`: the bytes `norm_key` gives of
/// `v.norm()`, made without making it.
pub fn put_norm(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(NULL_TAG),
        Value::Char(s) => put_norm_text(s, out),
        Value::Exact(d) => put_norm_exact(d.units, d.scale, out),
        Value::Approx(f) => put_norm_approx(*f, out),
        Value::Bits(b) => {
            out.push(BITS_TAG);
            b.put_key(out);
        }
        Value::Datetime(d) => {
            out.push(INSTANT_TAG);
            out.extend_from_slice(&d.utc.to_le_bytes());
        }
        Value::Interval(i) => {
            out.push(SPAN_TAG);
            out.extend_from_slice(&i.units.to_le_bytes());
        }
        Value::Extension(e) => {
            text(out, EXTENSION_TAG, e.kind.name());
            out.extend_from_slice(&u64::try_from(e.bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
            out.extend_from_slice(&e.bytes);
        }
    }
}

/// Appends the normalized key of the character string `s`, as `put_norm`
/// writes it of the value: without its trailing spaces, as strings
/// compare padded [X3.135-1986 §5.11 GR5].
pub fn put_norm_text(s: &str, out: &mut Vec<u8>) {
    text(out, CHAR_TAG, s.trim_end_matches(' '));
}

/// Appends the normalized key of the approximate number `f`, as
/// `put_norm` writes it of the value: -0 as 0, as they compare equal.
pub fn put_norm_approx(f: f64, out: &mut Vec<u8>) {
    out.push(APPROX_TAG);
    out.extend_from_slice(&(if f == 0.0 { 0f64.to_bits() } else { f.to_bits() }).to_le_bytes());
}

/// Appends the normalized key of the exact number of `units` at `scale`,
/// as `put_norm` writes it of the value, so an exact number held as an
/// integer keys as its value does.
pub fn put_norm_exact(units: i128, scale: u32, out: &mut Vec<u8>) {
    let (units, scale) = Decimal::new(units, scale).normalized();
    out.push(EXACT_TAG);
    out.extend_from_slice(&units.to_le_bytes());
    out.extend_from_slice(&scale.to_le_bytes());
}

/// The key of `values`: the bytes `norm_key` gives of their normalized
/// values, made without making them.
pub fn norm_row(values: &[Value]) -> Vec<u8> {
    // Most values' keys are a tag and 16 bytes or fewer.
    let mut out = Vec::with_capacity(values.len().saturating_mul(NORM_BYTES));
    for v in values {
        put_norm(v, &mut out);
    }
    out
}

/// The bytes `norm_row` makes room for of each value.
const NORM_BYTES: usize = 24;

/// The key of normalized values `key`: the same bytes exactly when the
/// values are identical (`Value::norm`).
pub fn norm_key(key: &[NormKey]) -> Vec<u8> {
    let mut out = Vec::new();
    for k in key {
        match k {
            NormKey::Null => out.push(NULL_TAG),
            NormKey::Char(s) => text(&mut out, CHAR_TAG, s),
            NormKey::Exact(units, scale) => {
                out.push(EXACT_TAG);
                out.extend_from_slice(&units.to_le_bytes());
                out.extend_from_slice(&scale.to_le_bytes());
            }
            NormKey::Approx(bits) => {
                out.push(APPROX_TAG);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            NormKey::Bits(b) => {
                out.push(BITS_TAG);
                b.put_key(&mut out);
            }
            NormKey::Instant(utc) => {
                out.push(INSTANT_TAG);
                out.extend_from_slice(&utc.to_le_bytes());
            }
            NormKey::Span(units) => {
                out.push(SPAN_TAG);
                out.extend_from_slice(&units.to_le_bytes());
            }
            NormKey::Extension(held) => {
                let (name, bytes) = &**held;
                text(&mut out, EXTENSION_TAG, name);
                out.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// Appends `tag`, `s`'s length and `s`.
fn text(out: &mut Vec<u8>, tag: u8, s: &str) {
    out.push(tag);
    out.extend_from_slice(&u64::try_from(s.len()).unwrap_or(u64::MAX).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}
