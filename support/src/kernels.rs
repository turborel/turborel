//! The contract every implementation of `Kernels` keeps (RULES.md D354,
//! R-TEST-10): each primitive gives what a plain loop over the same values
//! gives, at every length from none to past two null words, of every
//! comparison, of nulls and selections or none, and at the extremes of
//! each type. The core's tests run it against the portable kernels; a
//! kernel crate's tests run it against theirs, written once here.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::integer_division,
    clippy::default_numeric_fallback,
    clippy::missing_panics_doc,
    clippy::shadow_unrelated,
    reason = "test code: a panic is a test failure, its fixtures literal (RULES.md R-TEST-4)"
)]

use turborel::kernels::{DistanceTerm, Kernels, Packed};
use turborel::storage::Mask;
use turborel::value::CompOp;

/// Every comparison operator.
const OPS: [CompOp; 6] = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge];

/// The longest vector tried: past two words of a mask.
const LONGEST: usize = 130;

/// Whether `op` holds of `ordering`, none if unordered.
fn holds(op: CompOp, ordering: Option<std::cmp::Ordering>) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (op, ordering) {
        (_, None) => false,
        (CompOp::Eq, Some(o)) => o == Equal,
        (CompOp::Ne, Some(o)) => o != Equal,
        (CompOp::Lt, Some(o)) => o == Less,
        (CompOp::Gt, Some(o)) => o == Greater,
        (CompOp::Le, Some(o)) => o != Greater,
        (CompOp::Ge, Some(o)) => o != Less,
    }
}

/// Integers of `len`, the extremes among them.
fn ints(len: usize) -> Vec<i64> {
    (0..len)
        .map(|n| match n % 7 {
            0 => i64::MIN,
            1 => i64::MAX,
            _ => (n as i64 * 7919) % 41 - 20,
        })
        .collect()
}

/// Approximate numbers of `len`, NaN, infinities and both zeros among them.
fn floats(len: usize) -> Vec<f64> {
    let special = [f64::NAN, -0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY];
    (0..len).map(|n| if n % 5 == 0 { special[(n / 5) % special.len()] } else { ((n * 31) % 17) as f64 - 8.5 }).collect()
}

/// The ways of giving rows of a vector of `len`: none, every other null and
/// the first of the second word (so no two words of nulls are alike), a
/// selection of every third, both.
fn givens(len: usize) -> Vec<(Option<Mask>, Option<Vec<u32>>)> {
    let nulls = Mask::from_bools(&(0..len).map(|n| n % 2 == 1 || n == 64).collect::<Vec<_>>());
    let sel: Vec<u32> = (0..len as u32).filter(|r| r % 3 != 1).collect();
    vec![(None, None), (Some(nulls.clone()), None), (None, Some(sel.clone())), (Some(nulls), Some(sel))]
}

/// The rows a plain loop keeps: of `sel` or all `len`, not null, `truth`.
fn expected(len: usize, (nulls, sel): (Option<&Mask>, Option<&[u32]>), truth: impl Fn(usize) -> bool) -> Vec<u32> {
    let rows: Vec<u32> = sel.map_or_else(|| (0..len as u32).collect(), <[u32]>::to_vec);
    rows.into_iter().filter(|&r| !nulls.is_some_and(|m| m.get(r as usize)) && truth(r as usize)).collect()
}

/// Checks every primitive of `kernels` against a plain loop.
pub fn contract(kernels: &dyn Kernels) {
    assert_ne!(kernels.name(), "");
    for len in 0..=LONGEST {
        selections(kernels, len);
        aggregates(kernels, len);
    }
    unpacking(kernels);
    texts(kernels);
    blooms(kernels);
    xxh3_stripes(kernels);
    normalizing(kernels);
    for width in [0, 1, 3, 8, 16, 33] {
        bounding(kernels, width);
    }
    distances(kernels);
    crc32c(kernels);
    finding(kernels);
    utf8s(kernels);
}

/// Checks the hashes of texts `kernels` makes against XXH3-64 of each
/// text's bytes less its trailing spaces, of every length to past the
/// long inputs' 240 bytes, of seeds 0 and not, of a selection, rows past
/// the end among them, and of none.
fn texts(kernels: &dyn Kernels) {
    use turborel::hash::xxhash::{Seeded, xxh3_64_with_seed};
    let words: Vec<String> = (0..300_usize).map(|n| format!("{}{}", "x".repeat(n % 7).repeat(n / 7 + 1), " ".repeat(n % 3))).collect();
    let (mut text, mut ends) = (String::new(), Vec::new());
    for w in &words {
        text.push_str(w);
        ends.push(text.len());
    }
    let sel: Vec<u32> = (0..320).step_by(3).collect();
    for seed in [0, 0x5EED_u64, u64::MAX] {
        let seeded = Seeded::new(seed);
        let want =
            |n: usize| words.get(n).map_or(xxh3_64_with_seed(b"", seed), |w| xxh3_64_with_seed(w.trim_end_matches(' ').as_bytes(), seed));
        let mut out = vec![3];
        kernels.hash_texts((text.as_bytes(), &ends), &seeded, None, &mut out);
        assert_eq!(out[1..], (0..words.len()).map(want).collect::<Vec<_>>()[..], "hash_texts of all, seed {seed}");
        let mut out = Vec::new();
        kernels.hash_texts((text.as_bytes(), &ends), &seeded, Some(&sel), &mut out);
        assert_eq!(out, sel.iter().map(|&r| want(r as usize)).collect::<Vec<_>>(), "hash_texts of a selection, seed {seed}");
        let mut out = Vec::new();
        kernels.hash_texts((b"", &[]), &seeded, None, &mut out);
        assert_eq!(out, Vec::<u64>::new());
    }
}

/// Bytes of a fixed pattern, each of the low byte of an odd multiple.
fn pattern(len: usize, salt: u64) -> Vec<u8> {
    (0..len as u64).map(|n| (n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(salt) >> 56) as u8).collect()
}

/// Checks XXH3's long inputs' loops of `kernels` against the portable ones
/// (RULES.md D460): of 0 to 40 stripes, of secrets of the least length, the
/// default's and longer, from every accumulator state; and the hashes they
/// make of every length to past two blocks.
fn xxh3_stripes(kernels: &dyn Kernels) {
    use turborel::hash::xxhash::{self, ACC_NB, SECRET_SIZE_MIN, STRIPE_LEN, Scalar, Stripes};
    let input = pattern(40 * STRIPE_LEN, 1);
    let (stripes, _) = input.as_chunks::<STRIPE_LEN>();
    for secret_len in [SECRET_SIZE_MIN, 192, 400] {
        let secret = pattern(secret_len, 2);
        let usable = (secret_len - STRIPE_LEN) / 8 + 1;
        for count in 0..=usable.min(stripes.len()) {
            let start: [u64; ACC_NB] = std::array::from_fn(|n| u64::MAX.rotate_left(n as u32 * 9) ^ count as u64);
            let (mut want, mut got) = (start, start);
            Scalar.accumulate(&mut want, &stripes[..count], &secret);
            kernels.xxh3_accumulate(&mut got, &stripes[..count], &secret);
            assert_eq!(got, want, "xxh3_accumulate of {count} stripes, secret {secret_len}");
            let key = secret.last_chunk::<STRIPE_LEN>().unwrap();
            Scalar.scramble(&mut want, key);
            kernels.xxh3_scramble(&mut got, key);
            assert_eq!(got, want, "xxh3_scramble after {count} stripes, secret {secret_len}");
        }
    }
    let long = pattern(2 * 1024 + 300, 3);
    for len in (0..=long.len()).filter(|n| *n < 300 || n % 61 == 0 || n % 1024 < 2) {
        for seed in [0, 0x9E37_79B1] {
            let data = &long[..len];
            assert_eq!(xxhash::xxh3_64_on(kernels, data, seed), xxhash::xxh3_64_with_seed(data, seed), "XXH3-64 of {len}");
            assert_eq!(xxhash::xxh3_128_on(kernels, data, seed), xxhash::xxh3_128_with_seed(data, seed), "XXH3-128 of {len}");
        }
    }
}

/// Checks the selections of `kernels` of vectors of `len`.
fn selections(kernels: &dyn Kernels, len: usize) {
    let (i, f) = (ints(len), floats(len));
    let bits = Mask::from_bools(&(0..len).map(|n| n % 5 < 2).collect::<Vec<_>>());
    let codes: Vec<u32> = (0..len as u32).map(|n| n % 9).collect();
    let keep = Mask::from_bools(&[true, false, false, true, true, false, false, false, true]);
    for (nulls, sel) in givens(len) {
        let given = (nulls.as_ref(), sel.as_deref());
        for op in OPS {
            for c in [i64::MIN, -3, 0, 7, i64::MAX] {
                let mut out = vec![99];
                kernels.select_i64(&i, (op, c), given, &mut out);
                let mut want = vec![99];
                want.extend(expected(len, given, |n| holds(op, Some(i[n].cmp(&c)))));
                assert_eq!(out, want, "select_i64 {op:?} {c} of {len}");
            }
            for c in [f64::NAN, -0.0, 0.5, f64::INFINITY] {
                let mut out = Vec::new();
                kernels.select_f64(&f, (op, c), given, &mut out);
                assert_eq!(out, expected(len, given, |n| holds(op, f[n].partial_cmp(&c))), "select_f64 {op:?} {c} of {len}");
            }
        }
        let mut out = Vec::new();
        kernels.select_mask(bits.words(), given, &mut out);
        assert_eq!(out, expected(len, given, |n| bits.get(n)), "select_mask of {len}");
        let mut out = Vec::new();
        kernels.select_codes(&codes, keep.words(), given, &mut out);
        assert_eq!(out, expected(len, given, |n| keep.get(codes[n] as usize)), "select_codes of {len}");
    }
}

/// Checks the sums, extremes, gathers and hashes of `kernels` of vectors
/// of `len`.
fn aggregates(kernels: &dyn Kernels, len: usize) {
    let i = ints(len);
    for (nulls, sel) in givens(len) {
        let rows = expected(len, (nulls.as_ref(), sel.as_deref()), |_| true);
        if nulls.is_none() {
            let sum: i128 = sel
                .as_deref()
                .map_or_else(|| i.iter().map(|&v| i128::from(v)).sum(), |s| s.iter().map(|&r| i128::from(i[r as usize])).sum());
            assert_eq!(kernels.sum_i64(&i, sel.as_deref()), sum, "sum_i64 of {len}");
        }
        let held: Vec<i64> = rows.iter().map(|&r| i[r as usize]).collect();
        let extremes = held.iter().min().copied().zip(held.iter().max().copied());
        assert_eq!(kernels.min_max_i64(&i, (nulls.as_ref(), sel.as_deref())), extremes, "min_max_i64 of {len}");
        let mut out = vec![5];
        kernels.gather_i64(&i, &rows, &mut out);
        assert_eq!(out[1..], held[..], "gather_i64 of {len}");
        let seed = 0x5EED_u64.wrapping_mul(len as u64 + 1);
        let given: Vec<i64> = sel.as_deref().map_or_else(|| i.clone(), |s| s.iter().map(|&r| i[r as usize]).collect());
        let hashes: Vec<u64> = given.iter().map(|&v| turborel::hash::xxhash::xxh3_64_with_seed(&v.to_le_bytes(), seed)).collect();
        let mut out = vec![7];
        kernels.hash_i64(&i, seed, sel.as_deref(), &mut out);
        assert_eq!(out[1..], hashes[..], "hash_i64 of {len}");
        let next: Vec<u64> = hashes.iter().map(|h| h.rotate_left(17) ^ len as u64).collect();
        let mut joined = hashes.clone();
        kernels.combine_i64(&mut joined, &next, seed);
        for ((j, h), n) in joined.iter().zip(&hashes).zip(&next) {
            let bytes: Vec<u8> = h.to_le_bytes().into_iter().chain(n.to_le_bytes()).collect();
            assert_eq!(*j, turborel::hash::xxhash::xxh3_64_with_seed(&bytes, seed), "combine_i64 of {len}");
        }
    }
}

/// `count` values of `bits` bits, each the low bits of a pattern that sets
/// both ends of the width, bit-packed one after another from the least
/// significant bit, as a chunk's frame of reference writes them.
fn packed(count: usize, bits: u32) -> (Vec<u64>, Vec<u8>) {
    let low = if bits == 64 { u64::MAX } else { (1_u64 << bits) - 1 };
    let values: Vec<u64> = (0..count as u64).map(|n| n.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(7) & low).collect();
    let mut bytes = vec![0_u8; (count * bits as usize).div_ceil(8)];
    for (n, v) in values.iter().enumerate() {
        for b in 0..bits as usize {
            if (v >> b) & 1 == 1 {
                let at = n * bits as usize + b;
                bytes[at / 8] |= 1 << (at % 8);
            }
        }
    }
    (values, bytes)
}

/// Checks the unpacking of `kernels` of every width, of ranges on and off
/// a block's edges, against the bits read one at a time; none of too few
/// bytes or a width past 64.
fn unpacking(kernels: &dyn Kernels) {
    for bits in 0..=64_u32 {
        let (values, bytes) = packed(200, bits);
        for (from, to) in [(0, 200), (0, 64), (1, 129), (63, 65), (64, 192), (130, 130), (199, 200)] {
            for least in [0, -5, i64::MIN] {
                let mut out = vec![3];
                assert_eq!(kernels.unpack(&bytes, (bits, least), (from, to), &mut out), Some(()), "{bits} bits");
                let want: Vec<i64> = values[from..to].iter().map(|&v| least.wrapping_add(v as i64)).collect();
                assert_eq!(out[1..], want[..], "unpack of {bits} bits, {from} to {to}, plus {least}");
            }
        }
        if bits > 0 {
            assert_eq!(kernels.unpack(&bytes, (bits, 0), (0, 201 + 64), &mut Vec::new()), None, "{bits} bits, too few bytes");
        }
    }
    assert_eq!(kernels.unpack(&[0; 1024], (65, 0), (0, 64), &mut Vec::new()), None, "wider than 64 bits");
}

/// Checks the Bloom filter probe of `kernels` against each hash's bits
/// read one at a time: of filters of 1, 3 and 8 blocks, every hash put in
/// one found, and of hashes of no structure the same places as the
/// portable probe gives, an empty filter none.
fn blooms(kernels: &dyn Kernels) {
    use turborel::kernels::portable::bloom::{BLOCK_WORDS, bits_of, block_of, bloom_probe};
    for blocks in [1_usize, 3, 8] {
        let mut words = vec![0_u64; blocks * BLOCK_WORDS];
        let hash = |n: u64| turborel::hash::murmur::fmix64(n.wrapping_add(blocks as u64));
        for n in 0..40 {
            let h = hash(n);
            for (w, bit) in bits_of(h) {
                words[block_of(h, blocks) * BLOCK_WORDS + w] |= bit;
            }
        }
        let hashes: Vec<u64> = (0..LONGEST as u64).map(hash).collect();
        let (mut out, mut want) = (vec![9], vec![9]);
        kernels.bloom_probe(&words, &hashes, &mut out);
        bloom_probe(&words, &hashes, &mut want);
        assert_eq!(out, want, "bloom_probe of {blocks} blocks");
        assert!((0..40).all(|n| out.contains(&n)), "every hash put in is found, of {blocks} blocks");
        let mut none = Vec::new();
        kernels.bloom_probe(&vec![0; blocks * BLOCK_WORDS], &hashes, &mut none);
        assert!(none.is_empty(), "an empty filter holds none");
    }
}

/// Checks the normal keys `kernels` writes of integers against each
/// value's distance above the base written byte by byte, of every width,
/// inverted and not, beside other fields left as they were; none of a
/// value below the base or past the width, or a field past a key.
fn normalizing(kernels: &dyn Kernels) {
    let values: Vec<i64> = vec![i64::MIN, -300, -1, 0, 1, 255, 256, 65_535, 1 << 40, i64::MAX];
    let rows: Vec<u32> = vec![9, 0, 3, 3, 7, 1];
    for bytes in 1..=8_usize {
        for invert in [false, true] {
            let (width, at) = (bytes + 3, 2);
            let base = i64::MIN;
            let mut keys = vec![0xAB_u8; rows.len() * width];
            let fits = rows.iter().all(|&r| bytes == 8 || values[r as usize].wrapping_sub(base).cast_unsigned() >> (bytes * 8) == 0);
            let made = kernels.normalize_i64((&values, &rows), (base, bytes, invert), (width, at), &mut keys);
            if !fits {
                assert_eq!(made, None, "{bytes} bytes");
                continue;
            }
            assert_eq!(made, Some(()), "{bytes} bytes");
            for (key, &r) in keys.chunks(width).zip(&rows) {
                let be = values[r as usize].wrapping_sub(base).cast_unsigned().to_be_bytes();
                let want: Vec<u8> = be[8 - bytes..].iter().map(|b| if invert { !b } else { *b }).collect();
                assert_eq!(&key[at..at + bytes], &want[..], "{bytes} bytes, inverted {invert}");
                assert!(key[..at].iter().chain(&key[at + bytes..]).all(|&b| b == 0xAB), "other fields kept");
            }
        }
    }
    let mut keys = vec![0_u8; 8];
    assert_eq!(kernels.normalize_i64((&[5, -1], &[1]), (0, 2, false), (8, 0), &mut keys), None, "below the base");
    assert_eq!(kernels.normalize_i64((&[70_000], &[0]), (0, 2, false), (8, 0), &mut keys), None, "past the width");
    assert_eq!(kernels.normalize_i64((&[1], &[0]), (0, 4, false), (8, 5), &mut keys), None, "past the key");
    assert_eq!(kernels.normalize_i64((&[1], &[4]), (0, 4, false), (8, 0), &mut keys), None, "past the end");
    assert_eq!(kernels.normalize_i64((&[0, 300], &[0, 1]), (0, 2, false), (2, 0), &mut keys[..4]), Some(()));
    assert_eq!(&keys[..4], &[0, 0, 1, 44], "two keys of two bytes");
}

/// Checks the boundaries `kernels` finds of keys of `width` bytes against
/// a plain comparison of each key with the one before it, over every span
/// of their bytes, at every count of keys up to past two words, after
/// words already in the output, which stay as they were.
fn bounding(kernels: &dyn Kernels, width: usize) {
    for count in 0..=LONGEST {
        // Runs of equal keys of varied lengths, differing in one byte each.
        let keys: Vec<u8> = (0..count)
            .flat_map(|r| {
                (0..width).map(move |b| if b == (r / 3) % width.max(1) { ((r / 3) % 5) as u8 } else { u8::from(r % 7 == 0) * b as u8 })
            })
            .collect();
        for (from, to) in [(0, width), (0, width / 2), (width / 2, width), (1, width), (0, 0), (width, width + 1)] {
            let mut out = vec![0xFFFF_u64];
            kernels.boundaries(&keys, width, (from, to), &mut out);
            let n = if width == 0 { 0 } else { count };
            assert_eq!(out.len(), 1 + n.div_ceil(64), "{count} keys of {width}");
            assert_eq!(out[0], 0xFFFF, "words before kept");
            for r in 0..n.div_ceil(64) * 64 {
                let set = out[1 + r / 64] >> (r % 64) & 1 == 1;
                let span = |k: usize| &keys[k * width + from..k * width + to];
                let want = r >= 1 && r < n && from < to && to <= width && span(r) != span(r - 1);
                assert_eq!(set, want, "{count} keys of {width}, bytes {from}..{to}, row {r}");
            }
        }
    }
}

/// The sixteen lanes' sum of `term` over `a` and `b` as the contract
/// orders it (RULES.md D571), lane `i % 16` of each whole chunk, the lanes
/// in order, then the rest one pair at a time.
fn lanes_sum(a: &[f32], b: &[f32], term: impl Fn(f32, f32) -> f32) -> f32 {
    let whole = a.len().min(b.len()) / 16 * 16;
    let mut lanes = [0.0_f32; 16];
    for i in 0..whole {
        lanes[i % 16] += term(a[i], b[i]);
    }
    let tail = (whole..a.len().min(b.len())).fold(0.0_f32, |s, i| s + term(a[i], b[i]));
    lanes.iter().fold(0.0_f32, |s, x| s + x) + tail
}

/// A distance term and a plain function of it.
type Term = (DistanceTerm, fn(f32, f32) -> f32);

/// Checks the distance sums of `kernels` against the lanes' sums, of every
/// length to `LONGEST`, every term and format.
fn distances(kernels: &dyn Kernels) {
    let terms: [Term; 3] =
        [(DistanceTerm::Dot, |p, q| p * q), (DistanceTerm::Square, |p, q| (p - q) * (p - q)), (DistanceTerm::Gap, |p, q| (p - q).abs())];
    for len in 0..=LONGEST {
        let a: Vec<f32> = (0..len).map(|n| ((n * 37) % 23) as f32 * 0.37 - 4.1).collect();
        let b: Vec<f32> = (0..len).map(|n| ((n * 53) % 29) as f32 * -0.21 + 2.9).collect();
        let bytes = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        for (term, f) in terms {
            let want = lanes_sum(&a, &b, f);
            assert_eq!(kernels.distance_f32(&a, &b, term).to_bits(), want.to_bits(), "distance_f32 {term:?} of {len}");
            assert_eq!(
                kernels.distance_packed(&bytes(&a), &bytes(&b), (Packed::Single, term)).to_bits(),
                want.to_bits(),
                "{term:?} of {len}"
            );
            let (x, y): (Vec<u8>, Vec<u8>) = (0..len).map(|n| ((n * 37 % 256) as u8, (n * 91 % 256) as u8)).unzip();
            let int = |v: u8| f32::from(v as i8);
            let whole: i64 = x.iter().zip(&y).map(|(p, q)| f(int(*p), int(*q)) as i64).sum();
            assert_eq!(kernels.distance_i8(&x, &y, term), whole, "distance_i8 {term:?} of {len}");
        }
        // FLOAT16 of whole numbers and halves, exact in both formats.
        let halves: Vec<u8> = (0..len).flat_map(|n| [0x3C00_u16, 0xC100, 0x3800, 0x0000][n % 4].to_le_bytes()).collect();
        let as_f32: Vec<f32> = (0..len).map(|n| [1.0, -2.5, 0.5, 0.0][n % 4]).collect();
        let want = lanes_sum(&as_f32, &as_f32, |p, q| p * q);
        assert_eq!(
            kernels.distance_packed(&halves, &halves, (Packed::Half, DistanceTerm::Dot)).to_bits(),
            want.to_bits(),
            "FLOAT16 of {len}"
        );
        let (aa, bb, ab) = kernels.cosine_f32(&a, &b);
        let dots = (lanes_sum(&a, &a, |p, q| p * q), lanes_sum(&b, &b, |p, q| p * q), lanes_sum(&a, &b, |p, q| p * q));
        assert_eq!((aa.to_bits(), bb.to_bits(), ab.to_bits()), (dots.0.to_bits(), dots.1.to_bits(), dots.2.to_bits()), "cosine of {len}");
    }
}

/// Checks CRC-32C of `kernels`, the catalogue's check value of "123456789",
/// and of every split of a long input the register carried across.
fn crc32c(kernels: &dyn Kernels) {
    assert_eq!(kernels.crc32c(u32::MAX, b"123456789") ^ u32::MAX, 0xE306_9283);
    let input = pattern(70_000, 3);
    let whole = kernels.crc32c(u32::MAX, &input);
    for split in [0, 1, 7, 8, 255, 769, 24_576, 24_577, 69_999] {
        let (head, rest) = input.split_at(split);
        assert_eq!(kernels.crc32c(kernels.crc32c(u32::MAX, head), rest), whole, "CRC-32C split at {split}");
    }
}

/// The first place of `haystack` that holds `needle`, a place at a time.
fn plainly(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Checks the substring search of `kernels` (RULES.md D1119) against a
/// search a place at a time: of needles of 0 to 70 bytes, a needle's first
/// and last bytes alike, at every place of haystacks of every length to
/// past two steps of 64, the haystack of one byte repeated, a needle
/// found only past a near miss, and none found.
fn finding(kernels: &dyn Kernels) {
    let text: Vec<u8> = pattern(300, 7).iter().map(|b| b'a' + b % 4).collect();
    for len in 0..=150 {
        let haystack = &text[..len];
        for width in [0, 1, 2, 3, 7, 8, 9, 31, 32, 33, 63, 64, 65, 70] {
            for at in (0..len.saturating_sub(width)).step_by(5) {
                let needle = &text[at..at + width];
                assert_eq!(kernels.find_bytes(haystack, needle), plainly(haystack, needle), "{width} bytes from {at} of {len}");
            }
            let missing = vec![b'z'; width];
            assert_eq!(kernels.find_bytes(haystack, &missing), plainly(haystack, &missing), "{width} absent bytes of {len}");
        }
    }
    let same = vec![b'q'; 200];
    assert_eq!(kernels.find_bytes(&same, b"qq"), Some(0));
    assert_eq!(kernels.find_bytes(&same, b"qxq"), None);
    let mut near = vec![b'q'; 200];
    near[150] = b'x';
    assert_eq!(kernels.find_bytes(&near, b"qqxqq"), Some(148));
    let mut late = b"abcab".repeat(40);
    late.extend_from_slice(b"abcabd");
    assert_eq!(kernels.find_bytes(&late, b"abcabd"), Some(200));
}

/// Places of two to four bytes among others: either side of a lane's and
/// a step's edges, of the first, second and third steps.
const PLACES: [usize; 14] = [0, 1, 13, 14, 15, 16, 29, 30, 31, 32, 33, 63, 64, 95];

/// Third and fourth bytes tried: either side of each range a byte of a
/// character may take.
const LATER: [u8; 10] = [0x00, 0x7F, 0x80, 0x8F, 0x90, 0x9F, 0xA0, 0xBF, 0xC0, 0xFF];

/// Checks that `kernels` judge `bytes` as `str::from_utf8` does, the
/// text compared as bytes, so a text wrongly judged is never printed.
fn same(kernels: &dyn Kernels, bytes: &[u8]) {
    let ok = std::str::from_utf8(bytes).is_ok().then_some(bytes);
    assert_eq!(kernels.utf8(bytes).map(str::as_bytes), ok, "{bytes:02X?}");
}

/// `bytes` at `at` of 99 bytes of `a`.
fn among(at: usize, bytes: &[u8]) -> Vec<u8> {
    let mut all = vec![b'a'; 99];
    all[at..at + bytes.len()].copy_from_slice(bytes);
    all
}

/// Checks `lead`, of three or four bytes, and `second` after it, with
/// each of `LATER` third and, of a lead of four, each fourth.
fn leads(kernels: &dyn Kernels, [lead, second]: [u8; 2]) {
    for third in LATER {
        same(kernels, &among(15, &[lead, second, third]));
        same(kernels, &among(30, &[lead, second, third]));
        let fourths = LATER.into_iter().filter(|_| lead >= 0xF0);
        for (fourth, at) in fourths.flat_map(|fourth| [13, 14, 29, 30, 31].map(|at| (fourth, at))) {
            same(kernels, &among(at, &[lead, second, third, fourth]));
        }
    }
}

/// Checks the UTF-8 `kernels` judge (RULES.md D1175) against
/// `str::from_utf8`: of text of characters of one to four bytes, from
/// every place of a step to every length past three steps, cut within
/// characters among them; of every pair of bytes, and every lead of three
/// and four bytes with every second byte and a few third and fourth, at
/// places on and across a lane's and a step's edges; of text with one
/// byte changed at each place; and of bytes of no structure.
fn utf8s(kernels: &dyn Kernels) {
    let text = "aé€😀b z\u{7FF}\u{800}\u{FFFF}\u{10000}\u{10FFFF}".repeat(8);
    let bytes = text.as_bytes();
    for start in 0..32 {
        for end in start..bytes.len().min(start + 110) {
            same(kernels, &bytes[start..end]);
        }
    }
    for first in 0..=u8::MAX {
        for second in 0..=u8::MAX {
            for at in PLACES {
                same(kernels, &among(at, &[first, second]));
            }
        }
    }
    for lead in 0xE0..=0xF4_u8 {
        for second in 0..=u8::MAX {
            leads(kernels, [lead, second]);
        }
    }
    for at in 0..bytes.len().min(200) {
        for changed in [0x00, b'a', 0x80, 0xBF, 0xC0, 0xC2, 0xE0, 0xED, 0xEF, 0xF0, 0xF4, 0xF5, 0xFF] {
            let mut bytes = bytes[..200].to_vec();
            bytes[at] = changed;
            same(kernels, &bytes);
        }
    }
    for len in 0..300 {
        same(kernels, &pattern(len, 11));
        let high: Vec<u8> = pattern(len, 3).iter().map(|b| b & 0x7F).collect();
        same(kernels, &high);
    }
    assert_eq!(kernels.utf8(bytes).map(str::as_bytes), Some(bytes));
}
