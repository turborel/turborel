//! \[EXTENSION\] Adler-32, the checksum a zlib stream ends with (RFC 1950
//! §8.2, RULES.md D193): two sums modulo 65521, the first of the bytes
//! plus one, the second of the first's running values.

/// RFC 1950's modulus, the largest prime below 2^16.
const MOD_ADLER: u32 = 65_521;
/// The most bytes summed before reducing: the largest n for which
/// 255n(n+1)/2 + (n+1)(BASE-1) stays below 2^32 (RFC 1950 §9, zlib's NMAX).
const NMAX: usize = 5552;

/// The Adler-32 of `data`, continuing from `adler`; 1 starts a new sum.
pub fn update(adler: u32, data: &[u8]) -> u32 {
    let mut a = adler & 0xFFFF;
    let mut b = adler >> 16_u32;
    for chunk in data.chunks(NMAX) {
        for byte in chunk {
            a = a.wrapping_add(u32::from(*byte));
            b = b.wrapping_add(a);
        }
        a = a.checked_rem(MOD_ADLER).unwrap_or(a);
        b = b.checked_rem(MOD_ADLER).unwrap_or(b);
    }
    (b << 16_u32) | a
}

/// The Adler-32 of `data` alone.
pub fn adler32(data: &[u8]) -> u32 {
    update(1, data)
}
