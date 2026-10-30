//! \[DESIGN\] Cyclic redundancy checks: one engine for every CRC of the
//! parameterised model of Williams, "A Painless Guide to CRC Error
//! Detection Algorithms" (1993), with the parameters and check values of the
//! CRC RevEng catalogue (reveng.sourceforge.io/crc-catalogue). A CRC
//! detects every error of one burst no longer than its width, which is why
//! storage uses it to detect torn and corrupted writes.
//!
//! Speed comes from slicing (Kounavis and Berry, "A Systematic Approach to
//! Building High Performance Software-based CRC Generators", 2005): with
//! sixteen tables, one step consumes sixteen bytes. The byte-at-a-time
//! method (Sarwate, 1988) is kept as the reference the tests and the
//! benchmark compare against.

use std::sync::{Arc, OnceLock};

use super::{Digest, HashFunction, HashId};
use crate::error::{Code, Result, SqlError};

/// The parameters of a CRC in the RevEng catalogue's terms. Only CRCs whose
/// input and output are both reflected, or both not, are supported (`refin`
/// equals `refout`), which covers the catalogue entries below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrcParams {
    /// Its id in file formats.
    pub id: HashId,
    /// Its catalogue name.
    pub name: &'static str,
    /// Bits in the CRC: a multiple of 8, from 8 to 64.
    pub width: u32,
    /// The generator polynomial, without its top term, unreflected.
    pub poly: u64,
    /// The register's value before the first byte, unreflected.
    pub init: u64,
    /// Whether bytes enter, and the CRC leaves, least significant bit first.
    pub reflected: bool,
    /// What the final register is XORed with.
    pub xorout: u64,
    /// The CRC of the ASCII bytes "123456789".
    pub check: u64,
    /// The register, before `xorout`, after a message followed by its CRC.
    pub residue: u64,
}

// ---------------------------------------------------------------------------
// The built-in CRCs: the CRC RevEng catalogue entries of the same names, each
// checked against an independent bit-at-a-time implementation of the model.
// Their ids are fixed forever (`HashId`).
// ---------------------------------------------------------------------------

/// CRC-16/ARC.
pub const CRC16_ARC: CrcParams = CrcParams {
    id: HashId(1),
    name: "CRC-16/ARC",
    width: 16,
    poly: 0x8005,
    init: 0,
    reflected: true,
    xorout: 0,
    check: 0xbb3d,
    residue: 0,
};
/// CRC-16/IBM-3740, also known as CRC-16/CCITT-FALSE.
pub const CRC16_IBM_3740: CrcParams = CrcParams {
    id: HashId(2),
    name: "CRC-16/IBM-3740",
    width: 16,
    poly: 0x1021,
    init: 0xffff,
    reflected: false,
    xorout: 0,
    check: 0x29b1,
    residue: 0,
};
/// CRC-16/KERMIT.
pub const CRC16_KERMIT: CrcParams = CrcParams {
    id: HashId(3),
    name: "CRC-16/KERMIT",
    width: 16,
    poly: 0x1021,
    init: 0,
    reflected: true,
    xorout: 0,
    check: 0x2189,
    residue: 0,
};
/// CRC-16/XMODEM.
pub const CRC16_XMODEM: CrcParams = CrcParams {
    id: HashId(4),
    name: "CRC-16/XMODEM",
    width: 16,
    poly: 0x1021,
    init: 0,
    reflected: false,
    xorout: 0,
    check: 0x31c3,
    residue: 0,
};
/// CRC-32/ISO-HDLC: the CRC-32 of zlib, Ethernet and PNG.
pub const CRC32_ISO_HDLC: CrcParams = CrcParams {
    id: HashId(5),
    name: "CRC-32/ISO-HDLC",
    width: 32,
    poly: 0x04c1_1db7,
    init: 0xffff_ffff,
    reflected: true,
    xorout: 0xffff_ffff,
    check: 0xcbf4_3926,
    residue: 0xdebb_20e3,
};
/// CRC-32/ISCSI: CRC-32C, with Castagnoli's polynomial.
pub const CRC32_ISCSI: CrcParams = CrcParams {
    id: HashId(6),
    name: "CRC-32/ISCSI",
    width: 32,
    poly: 0x1edc_6f41,
    init: 0xffff_ffff,
    reflected: true,
    xorout: 0xffff_ffff,
    check: 0xe306_9283,
    residue: 0xb798_b438,
};
/// CRC-64/ECMA-182.
pub const CRC64_ECMA_182: CrcParams = CrcParams {
    id: HashId(7),
    name: "CRC-64/ECMA-182",
    width: 64,
    poly: 0x42f0_e1eb_a9ea_3693,
    init: 0,
    reflected: false,
    xorout: 0,
    check: 0x6c40_df5f_0b49_7347,
    residue: 0,
};
/// CRC-64/XZ: ECMA-182's polynomial, reflected, as the xz format uses it.
pub const CRC64_XZ: CrcParams = CrcParams {
    id: HashId(8),
    name: "CRC-64/XZ",
    width: 64,
    poly: 0x42f0_e1eb_a9ea_3693,
    init: u64::MAX,
    reflected: true,
    xorout: u64::MAX,
    check: 0x995d_c9bb_df19_39fa,
    residue: 0x4995_8c9a_bd7d_353f,
};
/// CRC-64/NVME.
pub const CRC64_NVME: CrcParams = CrcParams {
    id: HashId(9),
    name: "CRC-64/NVME",
    width: 64,
    poly: 0xad93_d235_94c9_3659,
    init: u64::MAX,
    reflected: true,
    xorout: u64::MAX,
    check: 0xae8b_1486_0a79_9888,
    residue: 0xf310_303b_2b6f_6e42,
};

/// Every built-in CRC.
pub const CATALOGUE: [CrcParams; 9] =
    [CRC16_ARC, CRC16_IBM_3740, CRC16_KERMIT, CRC16_XMODEM, CRC32_ISO_HDLC, CRC32_ISCSI, CRC64_ECMA_182, CRC64_XZ, CRC64_NVME];

/// Bits in a byte, the unit a CRC consumes.
const BYTE_BITS: u32 = u8::BITS;
/// Entries in a table: one per byte value.
const TABLE_LEN: usize = 256;
/// Tables, and so bytes consumed per step: slicing-by-16, chosen over
/// slicing-by-8 by `bench/hash.rs`.
const SLICES: usize = 16;

/// One table: the register change each byte value causes.
type Table = [u64; TABLE_LEN];

/// \[DESIGN\] A CRC with its tables, ready to use: a `HashFunction`.
///
/// The register is kept in the orientation that needs no reflecting while
/// bytes are processed: a reflected CRC in its low `width` bits, least
/// significant bit first; an unreflected one in its high `width` bits.
pub struct Crc {
    params: CrcParams,
    /// Bits below an unreflected register (64 - `width`); 0 when reflected.
    shift: u32,
    /// `tables[k][b]`: the change byte `b` makes when `k` bytes follow it.
    tables: Box<[Table; SLICES]>,
}

/// \[DESIGN\] The CRCs a database checksums its files with, built in: of
/// 32 bits or more, CRC-32/ISCSI first, its default (STORAGE.CHECKSUM).
/// Their tables are made once, and shared.
pub fn builtin() -> Vec<Arc<dyn HashFunction>> {
    static BUILT: OnceLock<Vec<Arc<dyn HashFunction>>> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let params = [CRC32_ISCSI, CRC32_ISO_HDLC, CRC64_ECMA_182, CRC64_XZ, CRC64_NVME];
            params.into_iter().filter_map(|p| Crc::new(p).ok()).map(|c| -> Arc<dyn HashFunction> { Arc::new(c) }).collect()
        })
        .clone()
}

impl Crc {
    /// The CRC with `params`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `width` is not a multiple of 8 from 8 to 64, or
    /// `poly`, `init` or `xorout` has bits above `width`.
    #[expect(
        clippy::large_stack_arrays,
        reason = "R-DEV-4: the 32 KiB of tables are built once per CRC and moved to the heap; thread stacks are far larger"
    )]
    pub fn new(params: CrcParams) -> Result<Crc> {
        let shift = check_params(&params)?;
        let (shift, t0) = if params.reflected { (0, reflected_table(&params)) } else { (shift, unreflected_table(&params, shift)) };
        let mut tables = Box::new([t0; SLICES]);
        let mut previous = t0;
        for table in tables.iter_mut().skip(1) {
            previous = std::array::from_fn(|b| {
                let v = previous.get(b).copied().unwrap_or_default();
                if params.reflected { (v >> BYTE_BITS) ^ at(&t0, low_byte(v)) } else { (v << BYTE_BITS) ^ at(&t0, high_byte(v)) }
            });
            *table = previous;
        }
        Ok(Crc { params, shift, tables })
    }

    /// Its parameters.
    pub fn params(&self) -> &CrcParams {
        &self.params
    }

    /// The register before any byte.
    pub fn start(&self) -> u64 {
        if self.params.reflected { reflect(self.params.init, self.params.width) } else { self.params.init << self.shift }
    }

    /// The register after `data`, from `register`: sixteen bytes per step,
    /// then the rest one at a time.
    pub fn update(&self, register: u64, data: &[u8]) -> u64 {
        let [t0, t1, t2, t3, t4, t5, t6, t7, t8, t9, t10, t11, t12, t13, t14, t15] = &*self.tables;
        let (blocks, rest) = data.as_chunks::<SLICES>();
        let mut r = register;
        for &bytes in blocks {
            // The register meets the first eight bytes; the second eight
            // enter the tables as they are, in the order they arrived.
            let ([b0, b1, b2, b3, b4, b5, b6, b7], [c0, c1, c2, c3, c4, c5, c6, c7]) = if self.params.reflected {
                let x = u128::from_le_bytes(bytes);
                ((r ^ low_half(x)).to_le_bytes(), high_half(x).to_le_bytes())
            } else {
                let x = u128::from_be_bytes(bytes);
                ((r ^ high_half(x)).to_be_bytes(), low_half(x).to_be_bytes())
            };
            r = at(t15, b0) ^ at(t14, b1) ^ at(t13, b2) ^ at(t12, b3) ^ at(t11, b4) ^ at(t10, b5) ^ at(t9, b6) ^ at(t8, b7);
            r ^= at(t7, c0) ^ at(t6, c1) ^ at(t5, c2) ^ at(t4, c3) ^ at(t3, c4) ^ at(t2, c5) ^ at(t1, c6) ^ at(t0, c7);
        }
        self.update_bytewise(r, rest)
    }

    /// The register after `data`, from `register`, one byte at a time: the
    /// reference method, slower than `update` but with the same result.
    pub fn update_bytewise(&self, register: u64, data: &[u8]) -> u64 {
        let [t0, ..] = &*self.tables;
        data.iter().fold(register, |r, &b| {
            if self.params.reflected { (r >> BYTE_BITS) ^ at(t0, low_byte(r) ^ b) } else { (r << BYTE_BITS) ^ at(t0, high_byte(r) ^ b) }
        })
    }

    /// The CRC of the bytes that brought the register to `register`.
    pub fn finish(&self, register: u64) -> u64 {
        (register >> self.shift) ^ self.params.xorout
    }

    /// The CRC of `data`.
    pub fn checksum(&self, data: &[u8]) -> u64 {
        self.finish(self.update(self.start(), data))
    }

    /// Bytes in the CRC.
    fn bytes(&self) -> usize {
        usize::try_from(self.params.width.checked_div(BYTE_BITS).unwrap_or_default()).unwrap_or_default()
    }
}

impl HashFunction for Crc {
    fn id(&self) -> HashId {
        self.params.id
    }
    fn name(&self) -> &'static str {
        self.params.name
    }
    fn digest_len(&self) -> usize {
        self.bytes()
    }
    /// The CRC, least significant byte first.
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let crc = self.finish(parts.iter().fold(self.start(), |r, part| self.update(r, part)));
        Digest::from_le(crc, self.bytes()).unwrap_or_default()
    }
}

/// Checks `params` and returns the bits below an unreflected register.
fn check_params(params: &CrcParams) -> Result<u32> {
    let invalid = |why: &str| SqlError::new(Code::Api, format!("[DESIGN] the CRC {} {why}", params.name));
    let bytes = params.width.checked_rem(BYTE_BITS);
    if bytes != Some(0) || params.width == 0 || params.width > u64::BITS {
        return Err(invalid("must be 8 to 64 bits wide, in whole bytes"));
    }
    let shift = u64::BITS.saturating_sub(params.width);
    let above_width = |v: u64| v.checked_shr(params.width).unwrap_or_default() != 0;
    if [params.poly, params.init, params.xorout].into_iter().any(above_width) {
        return Err(invalid("has a polynomial, initial value or final XOR wider than the CRC"));
    }
    Ok(shift)
}

/// The table of a reflected CRC: each byte value run through the register,
/// least significant bit first.
fn reflected_table(params: &CrcParams) -> Table {
    let poly = reflect(params.poly, params.width);
    std::array::from_fn(|b| {
        let start = u64::try_from(b).unwrap_or_default();
        (0..BYTE_BITS).fold(start, |r, _| if r & 1 == 1 { (r >> 1_u32) ^ poly } else { r >> 1_u32 })
    })
}

/// The table of an unreflected CRC held in the top `width` bits: each byte
/// value run through the register, most significant bit first.
fn unreflected_table(params: &CrcParams, shift: u32) -> Table {
    let poly = params.poly << shift;
    let top_bit = 1_u64 << (u64::BITS - 1);
    let byte_to_top = u64::BITS - BYTE_BITS;
    std::array::from_fn(|b| {
        let start = u64::try_from(b).unwrap_or_default() << byte_to_top;
        (0..BYTE_BITS).fold(start, |r, _| if r & top_bit == 0 { r << 1_u32 } else { (r << 1_u32) ^ poly })
    })
}

/// The low `width` bits of `value`, in reverse order.
fn reflect(value: u64, width: u32) -> u64 {
    value.reverse_bits().checked_shr(u64::BITS.saturating_sub(width)).unwrap_or_default()
}

/// Entry `b` of `table`; every byte value has one, so the lookup cannot fail.
#[inline]
fn at(table: &Table, b: u8) -> u64 {
    table.get(usize::from(b)).copied().unwrap_or_default()
}

/// The low 64 bits of `x`.
fn low_half(x: u128) -> u64 {
    u64::try_from(x & u128::from(u64::MAX)).unwrap_or_default()
}

/// The high 64 bits of `x`.
fn high_half(x: u128) -> u64 {
    u64::try_from(x >> u64::BITS).unwrap_or_default()
}

/// The least significant byte of `v`.
fn low_byte(v: u64) -> u8 {
    let [low, ..] = v.to_le_bytes();
    low
}

/// The most significant byte of `v`.
fn high_byte(v: u64) -> u8 {
    let [high, ..] = v.to_be_bytes();
    high
}

// Unit tests: every built-in CRC against the catalogue and an independent
// implementation, the fast method against the reference one, and the errors
// a CRC must detect (RULES.md R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue's check message.
    const CHECK: &[u8] = b"123456789";
    /// A message of 43 bytes: two steps of sixteen and eleven bytes over.
    const FOX: &[u8] = b"The quick brown fox jumps over the lazy dog";

    fn crcs() -> Vec<Crc> {
        CATALOGUE.iter().map(|p| Crc::new(*p).unwrap()).collect()
    }

    /// 1,280 bytes: 0 to 255, five times.
    fn ramp() -> Vec<u8> {
        (0..1280_u32).map(|i| u8::try_from(i % 256).unwrap()).collect()
    }

    /// Verifies: [DESIGN] RULES.md D30: each built-in CRC gives the RevEng
    /// catalogue's check value, and its digest is the CRC in `width / 8`
    /// bytes, least significant first.
    #[test]
    fn crcs_give_the_catalogue_check_values() {
        for crc in crcs() {
            let p = *crc.params();
            assert_eq!(crc.checksum(CHECK), p.check, "{}", p.name);
            let len = usize::try_from(p.width.checked_div(8).unwrap()).unwrap();
            assert_eq!(crc.hash(CHECK).as_bytes(), &p.check.to_le_bytes()[..len], "{}", p.name);
            assert_eq!((crc.id(), crc.name(), crc.digest_len()), (p.id, p.name, len));
        }
    }

    /// Verifies: [DESIGN] RULES.md D30: each built-in CRC agrees with an
    /// independent bit-at-a-time implementation of the model (and, for
    /// CRC-32/ISO-HDLC, with zlib) on an empty message, a message that is
    /// not a whole number of words, and one of many words.
    #[test]
    fn crcs_agree_with_an_independent_implementation() {
        // (name, empty, FOX, ramp): computed outside this crate.
        let expected: [(&str, u64, u64, u64); 9] = [
            ("CRC-16/ARC", 0x0, 0xfcdf, 0x477e),
            ("CRC-16/IBM-3740", 0xffff, 0x8fdd, 0x4613),
            ("CRC-16/KERMIT", 0x0, 0xc459, 0xf6be),
            ("CRC-16/XMODEM", 0x0, 0xf0c8, 0x5e64),
            ("CRC-32/ISO-HDLC", 0x0, 0x414f_a339, 0x1e7a_6d24),
            ("CRC-32/ISCSI", 0x0, 0x2262_0404, 0x23b6_2c98),
            ("CRC-64/ECMA-182", 0x0, 0x41e0_5242_ffa9_883b, 0xc79e_fcf8_7811_a1cf),
            ("CRC-64/XZ", 0x0, 0x5b5e_b8c2_e54a_a1c4, 0xdad0_7e0b_aac7_74fa),
            ("CRC-64/NVME", 0x0, 0xd76c_5405_4954_c143, 0x20b2_e6d2_0677_d312),
        ];
        let ramp = ramp();
        for (crc, (name, empty, fox, ramped)) in crcs().iter().zip(expected) {
            assert_eq!(crc.params().name, name);
            assert_eq!((crc.checksum(&[]), crc.checksum(FOX), crc.checksum(&ramp)), (empty, fox, ramped), "{name}");
        }
    }

    /// Verifies: [DESIGN] slicing gives the byte-at-a-time result for every
    /// length from 0 to 40 bytes at every alignment within a step.
    #[test]
    fn slicing_equals_the_reference_method() {
        let ramp = ramp();
        for crc in crcs() {
            for start in 0..16 {
                for len in 0..=40 {
                    let data = &ramp[start..start + len];
                    let fast = crc.update(crc.start(), data);
                    assert_eq!(fast, crc.update_bytewise(crc.start(), data), "{} at {start}, {len} bytes", crc.params().name);
                }
            }
        }
    }

    /// Verifies: [DESIGN] RULES.md D30, the `HashFunction` contract: hashing a
    /// message in two parts, split anywhere, equals hashing it whole.
    #[test]
    fn hashing_in_parts_equals_hashing_whole() {
        for crc in crcs() {
            let whole = crc.hash(FOX);
            for split in 0..=FOX.len() {
                let (head, tail) = FOX.split_at(split);
                assert_eq!(crc.hash_parts(&[head, tail]), whole, "{} split at {split}", crc.params().name);
            }
            assert_eq!(crc.hash_parts(&[]), crc.hash(&[]));
        }
    }

    /// Verifies: [DESIGN] a message followed by its own CRC leaves the
    /// catalogue's residue in the register, which lets a reader check a
    /// stored record in one pass.
    #[test]
    fn a_message_and_its_crc_leave_the_residue() {
        for crc in crcs() {
            let p = *crc.params();
            let len = usize::try_from(p.width.checked_div(8).unwrap()).unwrap();
            for message in [CHECK, FOX, &[]] {
                let value = crc.checksum(message);
                let stored = if p.reflected { value.to_le_bytes()[..len].to_vec() } else { value.to_be_bytes()[8 - len..].to_vec() };
                let register = crc.update(crc.update(crc.start(), message), &stored);
                assert_eq!(crc.finish(register) ^ p.xorout, p.residue, "{}", p.name);
            }
        }
    }

    /// `message` with the bits numbered `bits` inverted, bit 0 being the least
    /// significant of the first byte.
    fn flip(message: &[u8], bits: std::ops::Range<usize>) -> Vec<u8> {
        let mut bad = message.to_vec();
        for bit in bits {
            bad[bit.checked_div(8).unwrap()] ^= 1 << bit.checked_rem(8).unwrap();
        }
        bad
    }

    /// Verifies: [DESIGN] RULES.md D30: a CRC detects every error confined to
    /// one burst no longer than its width. Shown for every single-bit error
    /// and every run of 2 to `width` flipped bits, at every position of a
    /// 64-byte message.
    #[test]
    fn crcs_detect_every_burst_up_to_their_width() {
        let message: Vec<u8> = ramp()[..64].to_vec();
        let bits = message.len().checked_mul(8).unwrap();
        for crc in crcs() {
            let good = crc.checksum(&message);
            for len in 1..=usize::try_from(crc.params().width).unwrap() {
                for first in 0..=bits - len {
                    let bad = flip(&message, first..first + len);
                    assert_ne!(crc.checksum(&bad), good, "{}: {len} bits from bit {first}", crc.params().name);
                }
            }
        }
    }

    /// Verifies: [DESIGN] `Crc::new` refuses, with SQLCODE -910, a width that
    /// is not whole bytes from 8 to 64, and a polynomial, initial value or
    /// final XOR wider than the CRC; every built-in id is distinct.
    #[test]
    fn invalid_parameters_are_refused() {
        let refused = |p: CrcParams| Crc::new(p).err().map(|e| e.code);
        for width in [0, 12, 72] {
            assert_eq!(refused(CrcParams { width, ..CRC16_ARC }), Some(Code::Api), "width {width}");
        }
        assert_eq!(refused(CrcParams { poly: 0x1_8005, ..CRC16_ARC }), Some(Code::Api));
        assert_eq!(refused(CrcParams { init: 0x1_0000, ..CRC16_ARC }), Some(Code::Api));
        assert_eq!(refused(CrcParams { xorout: 0x1_0000, ..CRC16_ARC }), Some(Code::Api));
        let narrow = Crc::new(CrcParams { width: 8, poly: 0x07, check: 0xf4, ..CRC16_XMODEM }).unwrap();
        assert_eq!(narrow.checksum(CHECK), 0xf4, "CRC-8/SMBUS, the catalogue's 8-bit check value");
        let mut ids: Vec<u32> = CATALOGUE.iter().map(|p| p.id.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len());
    }
}
