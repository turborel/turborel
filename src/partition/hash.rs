//! \[EXTENSION\] PARTITION BY HASH (RULES.md D154): a partition of MODULUS
//! m and REMAINDER r holds the keys whose hash h has h mod m = r. The hash
//! is MurmurHash3's (`hash/murmur.rs`) of the key's values in their
//! normalized form (`Value::norm`), so values equal as they compare
//! [X3.135-1986 §5.11] hash alike, on any machine.
//!
//! Siblings' moduli each divide every larger one. Then partitions (m1,
//! r1) and (m2, r2), m1 dividing m2, overlap exactly when r2 mod m1 = r1,
//! so overlap is decided exactly; a modulus that divides no other nor is
//! divided by it is refused.

use super::{HASH, MODULUS, PartitionStrategy, REMAINDER, Restriction, refused, scalar_key};
use crate::catalog::PartitionBound;
use crate::error::Result;
use crate::hash::murmur::murmur3_x64_128;
use crate::value::{DataType, NormKey, Value};

/// \[DESIGN\] The seed of the key's hash: fixed, as stored rows depend on it.
const SEED: u32 = 0;

/// \[DESIGN\] The most combinations of equal values `admits` hashes; past
/// it a partition is admitted unhashed.
const MOST_HASHED: usize = 64;

/// The tags of a normalized value's kinds in the bytes hashed.
const NULL_TAG: u8 = 0;
/// A character string.
const CHAR_TAG: u8 = 1;
/// An exact number.
const EXACT_TAG: u8 = 2;
/// An approximate number.
const APPROX_TAG: u8 = 3;
/// A binary string.
const BINARY_TAG: u8 = 4;
/// A truth value.
const BOOL_TAG: u8 = 5;
/// A datetime.
const INSTANT_TAG: u8 = 6;
/// An interval.
const SPAN_TAG: u8 = 7;

/// The built-in HASH strategy.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashStrategy;

impl PartitionStrategy for HashStrategy {
    fn name(&self) -> &str {
        HASH
    }

    fn check_key(&self, types: &[DataType]) -> Result<()> {
        if types.iter().any(|t| matches!(t, DataType::DecFloat { .. })) {
            return refused("[EXTENSION] PARTITION BY HASH takes no key part of DECFLOAT");
        }
        scalar_key(HASH, types)
    }

    fn bound(&self, _types: &[DataType], spec: PartitionBound) -> Result<PartitionBound> {
        let PartitionBound::With(options) = &spec else {
            return refused("[EXTENSION] a partition of PARTITION BY HASH is FOR VALUES WITH (MODULUS m, REMAINDER r)");
        };
        let names: Vec<&str> = options.iter().map(|(n, _)| n.as_str()).collect();
        if names != [MODULUS, REMAINDER] {
            return refused("[EXTENSION] a hash partition's bound is WITH (MODULUS m, REMAINDER r), in that order");
        }
        if modulus_remainder(&spec).is_none_or(|(m, r)| r >= m) {
            return refused("[EXTENSION] a hash partition's MODULUS is a whole number above 0 and its REMAINDER one from 0 below it");
        }
        Ok(spec)
    }

    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool> {
        let (Some((ma, ra)), Some((mb, rb))) = (modulus_remainder(a), modulus_remainder(b)) else { return Ok(false) };
        let ((small, r_small), (large, r_large)) = if ma <= mb { ((ma, ra), (mb, rb)) } else { ((mb, rb), (ma, ra)) };
        if large.checked_rem(small) != Some(0) {
            return refused(format!(
                "[EXTENSION] hash partitions of MODULUS {small} and {large} are siblings, but each modulus must divide every larger one"
            ));
        }
        Ok(r_large.checked_rem(small) == Some(r_small))
    }

    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool> {
        let Some((m, r)) = modulus_remainder(bound) else { return Ok(false) };
        Ok(key_hash(key).checked_rem(m) == Some(r))
    }

    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool> {
        let Some(keys) = points(restrictions) else { return Ok(true) };
        for key in keys {
            if self.contains(bound, &key)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn takes_default(&self) -> bool {
        false
    }
}

/// The modulus and remainder of a hash bound, both whole and not
/// negative, the modulus above 0; none of another bound.
fn modulus_remainder(bound: &PartitionBound) -> Option<(u64, u64)> {
    let PartitionBound::With(options) = bound else { return None };
    let whole = |name: &str| options.iter().find(|(n, _)| n == name).and_then(|(_, v)| whole_number(v));
    let (m, r) = (whole(MODULUS)?, whole(REMAINDER)?);
    (m > 0).then_some((m, r))
}

/// The value of `v` if it is an exact number with no fraction, from 0.
fn whole_number(v: &Value) -> Option<u64> {
    let Value::Exact(d) = v else { return None };
    let (units, scale) = d.normalized();
    if scale != 0 {
        return None;
    }
    u64::try_from(units).ok()
}

/// The hash of a key: of its parts' normalized forms, each tagged.
pub fn key_hash(key: &[Value]) -> u64 {
    let mut bytes = Vec::new();
    for v in key {
        put_norm(&mut bytes, &v.norm());
    }
    murmur3_x64_128(&bytes, SEED).0
}

/// A normalized value in the bytes hashed. A kind no hash key holds
/// (`check_key`) is hashed by its tag alone.
fn put_norm(out: &mut Vec<u8>, n: &NormKey) {
    match n {
        NormKey::Char(s) => tagged(out, CHAR_TAG, s.as_bytes()),
        NormKey::Exact(units, scale) => {
            out.push(EXACT_TAG);
            out.extend_from_slice(&units.to_be_bytes());
            out.extend_from_slice(&scale.to_be_bytes());
        }
        NormKey::Approx(bits) => tagged(out, APPROX_TAG, &bits.to_be_bytes()),
        NormKey::Binary(b) => tagged(out, BINARY_TAG, b.bytes()),
        NormKey::Bool(b) => tagged(out, BOOL_TAG, &[u8::from(*b)]),
        NormKey::Instant(i) => tagged(out, INSTANT_TAG, &i.to_be_bytes()),
        NormKey::Span(s) => tagged(out, SPAN_TAG, &s.to_be_bytes()),
        NormKey::DecFloat(_)
        | NormKey::Lob(_)
        | NormKey::Json(_)
        | NormKey::Extension(_)
        | NormKey::Composite(_)
        | NormKey::Bag(_)
        | NormKey::Null => out.push(NULL_TAG),
    }
}

/// `bytes` after their tag and length.
fn tagged(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    out.push(tag);
    out.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Every key the restrictions allow, if each part is equal to values (or
/// null) and they are at most `MOST_HASHED`; none otherwise.
fn points(restrictions: &[Option<Restriction>]) -> Option<Vec<Vec<Value>>> {
    let mut keys: Vec<Vec<Value>> = vec![Vec::new()];
    for r in restrictions {
        let values = match r {
            Some(Restriction::Equal(values)) => values.clone(),
            Some(Restriction::Null) => vec![Value::Null],
            Some(Restriction::Range { .. }) | None => return None,
        };
        if keys.len().saturating_mul(values.len()) > MOST_HASHED {
            return None;
        }
        keys =
            keys.iter().flat_map(|k| values.iter().map(move |v| k.iter().cloned().chain(std::iter::once(v.clone())).collect())).collect();
    }
    Some(keys)
}
