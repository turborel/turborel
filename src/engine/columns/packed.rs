//! \[DESIGN\] Integers bit-packed (RULES.md D31): each of `bits` bits, one
//! after another from the least significant bit of the first byte, so a
//! frame of reference's differences take the bits their span needs, not
//! whole bytes. Split from encoded.rs (RULES.md R-SIZE-2).

/// The bytes a window of one value is read from: a value of 64 bits at
/// any bit's offset lies in 9 of them.
const WINDOW: usize = 16;

/// The bits that hold every number up to `span`.
pub(super) fn bits_for(span: u64) -> u32 {
    u64::BITS.saturating_sub(span.leading_zeros())
}

/// The bytes `count` values of `bits` bits take.
pub(super) fn packed_len(count: usize, bits: u32) -> Option<usize> {
    count.checked_mul(usize::try_from(bits).ok()?).map(|b| b.div_ceil(8))
}

/// Appends `values`, each its lowest `bits` bits, packed.
pub(super) fn pack(values: impl Iterator<Item = u64>, bits: u32, out: &mut Vec<u8>) {
    let (mut held, mut filled) = (0_u128, 0_u32);
    let mask = mask(bits);
    for v in values {
        held |= u128::from(v & mask) << filled;
        filled = filled.saturating_add(bits);
        while filled >= 8 {
            out.push(held.to_le_bytes().first().copied().unwrap_or(0));
            held >>= 8_u32;
            filled = filled.saturating_sub(8);
        }
    }
    if filled > 0 {
        out.push(held.to_le_bytes().first().copied().unwrap_or(0));
    }
}

/// The `n`th value of `bits` bits packed in `bytes`.
pub(super) fn get(bytes: &[u8], bits: u32, n: usize) -> Option<u64> {
    let bit = n.checked_mul(usize::try_from(bits).ok()?)?;
    let (at, shift) = (bit.checked_div(8)?, u32::try_from(bit.checked_rem(8)?).ok()?);
    let mut window = [0_u8; WINDOW];
    let held = bytes.get(at..at.saturating_add(WINDOW).min(bytes.len()))?;
    window.get_mut(..held.len())?.copy_from_slice(held);
    let v = (u128::from_le_bytes(window) >> shift) & u128::from(mask(bits));
    u64::try_from(v).ok()
}

/// Each value from the `from`th to the `to`th of `bits` bits packed in
/// `bytes`, in order, given to `each`: each read of the 8 bytes it begins
/// in, but one those bytes do not hold whole, past the end of `bytes` or
/// of their 64 bits, read as `get` reads it. None if `bytes` hold fewer.
pub(super) fn unpack(bytes: &[u8], bits: u32, (from, to): (usize, usize), mut each: impl FnMut(u64)) -> Option<()> {
    if bytes.len() < packed_len(to, bits)? {
        return None;
    }
    let (width, mask) = (usize::try_from(bits).ok()?, mask(bits));
    let mut bit = from.checked_mul(width)?;
    for n in from..to {
        let (at, shift) = (bit >> 3_u32, u32::try_from(bit & 7).ok()?);
        let word = bytes.get(at..at.saturating_add(8)).and_then(|w| <[u8; 8]>::try_from(w).ok()).map(u64::from_le_bytes);
        match word.filter(|_| shift.saturating_add(bits) <= u64::BITS) {
            Some(word) => each((word >> shift) & mask),
            None => each(get(bytes, bits, n)?),
        }
        bit = bit.saturating_add(width);
    }
    Some(())
}

/// A mask of the lowest `bits` bits.
fn mask(bits: u32) -> u64 {
    if bits >= u64::BITS { u64::MAX } else { (1_u64 << bits).saturating_sub(1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D31, bit-packing: values of every width
    /// from 0 to 64 bits read back as packed, the greatest of a width
    /// among them, in the bytes their count and width take, one at a time
    /// and in order from any; past the values, none.
    #[test]
    fn packed_values_read_back() {
        for bits in 0_u32..=64_u32 {
            let top = mask(bits);
            let values: Vec<u64> =
                (0_u64..100_u64).map(|n| if n % 7 == 3 { top } else { n.wrapping_mul(0x9E37_79B9_7F4A_7C15) & top }).collect();
            let mut out = Vec::new();
            pack(values.iter().copied(), bits, &mut out);
            assert_eq!(Some(out.len()), packed_len(values.len(), bits), "{bits} bits");
            let back: Vec<u64> = (0..values.len()).map(|n| get(&out, bits, n).unwrap()).collect();
            assert_eq!(back, values, "{bits} bits");
            let mut unpacked = Vec::new();
            unpack(&out, bits, (3, values.len()), |v| unpacked.push(v)).unwrap();
            assert_eq!(unpacked, values[3..], "{bits} bits, unpacked from the third");
        }
        assert_eq!(bits_for(0), 0);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(255), 8);
        assert_eq!(bits_for(256), 9);
        assert_eq!(bits_for(u64::MAX), 64);
        assert_eq!(get(&[1, 2], 8, 5), None, "past the values");
        assert_eq!(unpack(&[1, 2], 8, (0, 3), |_| {}), None, "fewer than asked");
    }
}
