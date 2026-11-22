//! \[DESIGN\] CRC-32C by an instruction that carries the register over a
//! word (RULES.md D572): SSE4.2's `crc32` on x86-64, the CRC extension's
//! `crc32cx` on AArch64. One register's chain waits on each instruction,
//! so a long input is read as three streams at once, each from its own
//! register, joined by the register change of the zero bytes that follow
//! each, from tables made once by the portable body. A CRC's register is
//! linear in the bits before it, so the joined register is the one the
//! bytes give one after another, exactly the portable body's.

use std::sync::OnceLock;

use turborel::kernels::portable;

/// The bytes each stream reads a round, of a long input.
const LONG: usize = 8192;
/// The bytes of a round of three long streams.
const LONG_ROUND: usize = 24_576;
/// The bytes each stream reads a round, of a short input or the rest.
const SHORT: usize = 256;
/// The bytes of a round of three short streams.
const SHORT_ROUND: usize = 768;
/// The bits of a register.
const REGISTER_BITS: u32 = 32;
/// The bits of a byte.
const BYTE_BITS: u32 = 8;

/// The register change some zero bytes make, by each byte of the register
/// (`table[k][b]` of byte `k` holding `b`).
type Shift = [[u32; 256]; 4];

/// The changes of `LONG` and of `SHORT` zero bytes.
struct Shifts {
    long: Box<Shift>,
    short: Box<Shift>,
}

/// The tables, made at the first long input.
fn shifts() -> &'static Shifts {
    static SHIFTS: OnceLock<Shifts> = OnceLock::new();
    SHIFTS.get_or_init(|| Shifts { long: shift_of(LONG), short: shift_of(SHORT) })
}

/// The register change of `n` zero bytes, each bit's by the portable body
/// and each byte value's the changes of its bits joined.
fn shift_of(n: usize) -> Box<Shift> {
    let zeros = vec![0_u8; n];
    let bits: Vec<u32> = (0..REGISTER_BITS).map(|j| portable::crc::crc32c(1_u32 << j, &zeros)).collect();
    let mut table = Box::new([[0_u32; 256]; 4]);
    for (row, images) in table.iter_mut().zip(bits.as_chunks::<8>().0) {
        for (b, slot) in (0_u32..).zip(row.iter_mut()) {
            *slot = (0..BYTE_BITS).zip(images).filter(|(i, _)| (b >> i) & 1 == 1).fold(0, |acc, (_, image)| acc ^ image);
        }
    }
    table
}

/// `register` after the zero bytes of `shift`.
fn shifted(shift: &Shift, register: u32) -> u32 {
    let [t0, t1, t2, t3] = shift;
    let [b0, b1, b2, b3] = register.to_le_bytes();
    let at = |t: &[u32; 256], b: u8| t.get(usize::from(b)).copied().unwrap_or_default();
    at(t0, b0) ^ at(t1, b1) ^ at(t2, b2) ^ at(t3, b3)
}

/// The words of `bytes`, a multiple of eight long.
fn words(bytes: &[u8]) -> impl Iterator<Item = u64> + '_ {
    bytes.as_chunks::<8>().0.iter().map(|w| u64::from_le_bytes(*w))
}

/// The register after `data`, from `register`, by `word`, which carries
/// a register over a word as the instruction does.
#[inline]
pub(crate) fn crc32c_by(word: impl Fn(u32, u64) -> u32, register: u32, data: &[u8]) -> u32 {
    let mut carried = register;
    let mut rest = data;
    for (round, each) in [(LONG_ROUND, LONG), (SHORT_ROUND, SHORT)] {
        if rest.len() < round {
            continue;
        }
        let shift = if each == LONG { &shifts().long } else { &shifts().short };
        while let Some((block, after)) = rest.split_at_checked(round) {
            let (first, others) = block.split_at(each);
            let (second, third) = others.split_at(each);
            let mut streams = (carried, 0, 0);
            for ((one, two), three) in words(first).zip(words(second)).zip(words(third)) {
                streams = (word(streams.0, one), word(streams.1, two), word(streams.2, three));
            }
            carried = shifted(shift, shifted(shift, streams.0) ^ streams.1) ^ streams.2;
            rest = after;
        }
    }
    let (whole, bytes) = rest.as_chunks::<8>();
    let carried = whole.iter().fold(carried, |r, w| word(r, u64::from_le_bytes(*w)));
    portable::crc::crc32c(carried, bytes)
}
