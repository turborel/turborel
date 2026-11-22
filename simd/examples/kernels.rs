//! How fast each kernel runs at each level this processor has, beside the
//! portable bodies (RULES.md D570, R-BENCH-2): nanoseconds a row, the
//! median of repeated runs, of vectors of a few sizes, selections at a
//! low and a middle selectivity; distances of vectors of a few
//! dimensions, a row an element, and CRC-32C of a few lengths, a row a
//! byte. Run with `cargo run --release -p
//! turborel-simd --example kernels`; `ONLY` set to a kernel's name runs
//! that one alone.
#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::default_numeric_fallback,
    clippy::too_many_lines,
    clippy::shadow_unrelated,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

use std::hint::black_box;
use std::time::Instant;

use turborel::hash::xxhash::Seeded;
use turborel::kernels::{AlpScale, DistanceTerm, Kernels, Packed, Portable};
use turborel::storage::Mask;
use turborel::value::CompOp;
use turborel_simd::{Level, Simd};

/// The sizes measured.
const SIZES: [usize; 4] = [64, 2048, 16_384, 1 << 20];

/// The dimensions of the vectors measured.
const DIMS: [usize; 4] = [96, 128, 768, 1536];

/// The lengths of the inputs CRC-32C is measured on.
const CRC_BYTES: [usize; 4] = [64, 4096, 8192, 1 << 20];

/// Rows run per measurement, about: small sizes repeat.
const ROWS_A_RUN: usize = 1 << 22;

/// Runs of each measurement, the median taken.
const RUNS: usize = 9;

/// A xorshift64* generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Nanoseconds a row of `run`, which reads `rows` rows a call: the median
/// of `RUNS` runs of enough calls to read about `ROWS_A_RUN` rows.
fn timed(rows: usize, mut run: impl FnMut()) -> f64 {
    let calls = (ROWS_A_RUN / rows.max(1)).max(1);
    let mut each: Vec<f64> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..calls {
                run();
            }
            start.elapsed().as_nanos() as f64 / (calls * rows.max(1)) as f64
        })
        .collect();
    each.sort_by(f64::total_cmp);
    each[RUNS / 2]
}

/// The kernels measured: the portable ones, then each level.
fn sets() -> Vec<(String, Box<dyn Kernels>)> {
    let mut sets: Vec<(String, Box<dyn Kernels>)> = vec![("PORTABLE".to_owned(), Box::new(Portable))];
    for level in Simd::levels().into_iter().filter(|l| *l != Level::Portable) {
        sets.push((format!("{level:?}"), Box::new(Simd::with_level(level).unwrap())));
    }
    sets
}

/// Prints a line of a kernel's case: its time a row at each set, and each
/// set's speed against the portable one.
fn line(name: &str, case: &str, size: usize, sets: &[(String, Box<dyn Kernels>)], mut run: impl FnMut(&dyn Kernels)) {
    let times: Vec<f64> = sets.iter().map(|(_, k)| timed(size, || run(k.as_ref()))).collect();
    let cells: Vec<String> = sets.iter().zip(&times).map(|((n, _), t)| format!("{n} {t:.3} ({:.2}x)", times[0] / t)).collect();
    println!("{name:<12} {case:<14} {size:>8}  {}", cells.join("  "));
}

/// The hashes' lines: of integers all and a half selected, joins of a
/// key's columns, and texts short (8 bytes) and long (300).
fn hashes(wanted: &dyn Fn(&str) -> bool, (size, ints, sel): (usize, &[i64], &[u32]), sets: &[(String, Box<dyn Kernels>)], rng: &mut Rng) {
    let mut out = Vec::with_capacity(size + 16);
    if wanted("hash_i64") {
        line("hash_i64", "all", size, sets, |k| {
            out.clear();
            k.hash_i64(black_box(ints), 0x5EED, None, &mut out);
            black_box(&out);
        });
        line("hash_i64", "half", size, sets, |k| {
            out.clear();
            k.hash_i64(black_box(ints), 0x5EED, Some(sel), &mut out);
            black_box(&out);
        });
    }
    if wanted("combine_i64") {
        let next: Vec<u64> = (0..size).map(|_| rng.next()).collect();
        let mut hashes: Vec<u64> = (0..size).map(|_| rng.next()).collect();
        line("combine_i64", "all", size, sets, |k| {
            k.combine_i64(black_box(&mut hashes), &next, 0x5EED);
            black_box(&hashes);
        });
    }
    if wanted("hash_texts") && size <= 1 << 14 {
        let seeded = Seeded::new(0x5EED);
        for (case, len) in [("short", 8), ("long", 300)] {
            let text: Vec<u8> = (0..size * len).map(|_| b'a' + (rng.next() % 26) as u8).collect();
            let ends: Vec<usize> = (1..=size).map(|n| n * len).collect();
            line("hash_texts", case, size, sets, |k| {
                out.clear();
                k.hash_texts((black_box(&text), &ends), &seeded, None, &mut out);
                black_box(&out);
            });
        }
    }
}

/// The sums' and extremes' lines: of all rows, nulls, a half picked,
/// picks either side of the share a masked run takes, and a few picks.
fn aggregates(
    wanted: &dyn Fn(&str) -> bool,
    (size, ints, sel): (usize, &[i64], &[u32]),
    nulls: &Mask,
    sets: &[(String, Box<dyn Kernels>)],
) {
    if wanted("sum_i64") {
        line("sum_i64", "all", size, sets, |k| {
            black_box(k.sum_i64(black_box(ints), None));
        });
        line("sum_i64", "half picked", size, sets, |k| {
            black_box(k.sum_i64(black_box(ints), Some(sel)));
        });
        for share in [3_u32, 4, 5, 8] {
            let picks: Vec<u32> = (0..size as u32).filter(|r| r % share == 0).collect();
            line("sum_i64", &format!("1 in {share} picked"), picks.len(), sets, |k| {
                black_box(k.sum_i64(black_box(ints), Some(&picks)));
            });
        }
        for few in [16_usize, 64, 256] {
            let picks: Vec<u32> = sel.iter().copied().step_by(7).take(few).collect();
            line("sum_i64", &format!("{} picked", picks.len()), picks.len(), sets, |k| {
                black_box(k.sum_i64(black_box(ints), Some(&picks)));
            });
        }
    }
    if wanted("min_max_i64") {
        line("min_max_i64", "all", size, sets, |k| {
            black_box(k.min_max_i64(black_box(ints), (None, None)));
        });
        line("min_max_i64", "nulls", size, sets, |k| {
            black_box(k.min_max_i64(black_box(ints), (Some(nulls), None)));
        });
        line("min_max_i64", "half picked", size, sets, |k| {
            black_box(k.min_max_i64(black_box(ints), (Some(nulls), Some(sel))));
        });
        for share in [3_u32, 4, 5, 8] {
            let picks: Vec<u32> = (0..size as u32).filter(|r| r % share == 0).collect();
            line("min_max_i64", &format!("1 in {share} picked"), picks.len(), sets, |k| {
                black_box(k.min_max_i64(black_box(ints), (Some(nulls), Some(&picks))));
            });
        }
        for few in [16_usize, 64, 256] {
            let picks: Vec<u32> = sel.iter().copied().step_by(7).take(few).collect();
            line("min_max_i64", &format!("{} picked", picks.len()), picks.len(), sets, |k| {
                black_box(k.min_max_i64(black_box(ints), (Some(nulls), Some(&picks))));
            });
        }
    }
}

fn main() {
    let only = std::env::var("ONLY").ok();
    let wanted = |name: &str| only.as_deref().is_none_or(|o| o == name);
    let sets = sets();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    println!("kernel       case             rows  ns a row at each set (speed against portable)");
    for size in SIZES {
        let ints: Vec<i64> = (0..size).map(|_| (rng.next() % 1000) as i64).collect();
        let sel: Vec<u32> = (0..size as u32).filter(|_| rng.next().is_multiple_of(2)).collect();
        hashes(&wanted, (size, &ints, &sel), &sets, &mut rng);
    }
    for size in SIZES {
        let ints: Vec<i64> = (0..size).map(|_| (rng.next() % 1000) as i64).collect();
        let floats: Vec<f64> = ints.iter().map(|&v| v as f64 + 0.5).collect();
        let nulls = Mask::from_bools(&(0..size).map(|_| rng.next().is_multiple_of(10)).collect::<Vec<_>>());
        let sel: Vec<u32> = (0..size as u32).filter(|_| rng.next().is_multiple_of(2)).collect();
        let mut out = Vec::with_capacity(size + 16);
        for (case, c) in [("1%", 10_i64), ("50%", 500)] {
            if wanted("select_i64") {
                line("select_i64", case, size, &sets, |k| {
                    out.clear();
                    k.select_i64(black_box(&ints), (CompOp::Lt, c), (None, None), &mut out);
                    black_box(&out);
                });
                line("select_i64", &format!("{case} nulls"), size, &sets, |k| {
                    out.clear();
                    k.select_i64(black_box(&ints), (CompOp::Lt, c), (Some(&nulls), None), &mut out);
                    black_box(&out);
                });
            }
            if wanted("select_f64") {
                line("select_f64", case, size, &sets, |k| {
                    out.clear();
                    k.select_f64(black_box(&floats), (CompOp::Lt, c as f64), (None, None), &mut out);
                    black_box(&out);
                });
            }
        }
        let mut out = Vec::with_capacity(size + 16);
        if wanted("select_mask") {
            let words: Vec<u64> = (0..size.div_ceil(64)).map(|_| rng.next()).collect();
            line("select_mask", "50%", size, &sets, |k| {
                out.clear();
                k.select_mask(black_box(&words), (Some(&nulls), None), &mut out);
                black_box(&out);
            });
        }
        if wanted("select_codes") {
            let codes: Vec<u32> = ints.iter().map(|&v| (v % 100) as u32).collect();
            let keep = [rng.next(), rng.next()];
            line("select_codes", "50%", size, &sets, |k| {
                out.clear();
                k.select_codes(black_box(&codes), &keep, (None, None), &mut out);
                black_box(&out);
            });
        }
        aggregates(&wanted, (size, &ints, &sel), &nulls, &sets);
        if wanted("gather_i64") {
            let mut gathered = Vec::with_capacity(size);
            line("gather_i64", "half", size, &sets, |k| {
                gathered.clear();
                k.gather_i64(black_box(&ints), &sel, &mut gathered);
                black_box(&gathered);
            });
        }
        if wanted("alp_decode") {
            let mut words = ints.clone();
            line("alp_decode", "factored", size, &sets, |k| {
                words.copy_from_slice(&ints);
                k.alp_decode(black_box(&mut words), AlpScale::Factored(100.0, 0.01));
                black_box(&words);
            });
        }
    }
    vectors_and_crcs(&sets, &mut rng, &wanted);
}

/// The lines of the distance kernels and of CRC-32C.
fn vectors_and_crcs(sets: &[(String, Box<dyn Kernels>)], rng: &mut Rng, wanted: &impl Fn(&str) -> bool) {
    for dims in DIMS {
        distances(sets, rng, dims, wanted);
    }
    if wanted("crc32c") {
        for bytes in CRC_BYTES {
            let data: Vec<u8> = (0..bytes).map(|_| rng.next() as u8).collect();
            line("crc32c", "bytes", bytes, sets, |k| {
                black_box(k.crc32c(u32::MAX, black_box(&data)));
            });
        }
    }
}

/// The lines of the distance kernels, of vectors of `dims` elements.
fn distances(sets: &[(String, Box<dyn Kernels>)], rng: &mut Rng, dims: usize, wanted: &impl Fn(&str) -> bool) {
    let mut float = || (rng.next() % 2000) as f32 / 1000.0 - 1.0;
    let (a, b): (Vec<f32>, Vec<f32>) = (0..dims).map(|_| (float(), float())).unzip();
    let packed = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let halves = |v: &[f32]| v.iter().flat_map(|x| turborel::vector::half_bits(f64::from(*x)).to_le_bytes()).collect::<Vec<u8>>();
    let bytes = |v: &[f32]| v.iter().map(|x| (x * 127.0) as i8 as u8).collect::<Vec<u8>>();
    let (pa, pb, ha, hb, ia, ib) = (packed(&a), packed(&b), halves(&a), halves(&b), bytes(&a), bytes(&b));
    for (case, term) in [("dot", DistanceTerm::Dot), ("square", DistanceTerm::Square), ("gap", DistanceTerm::Gap)] {
        if wanted("distance_f32") {
            line("distance_f32", case, dims, sets, |k| {
                black_box(k.distance_f32(black_box(&a), black_box(&b), term));
            });
        }
        if wanted("distance_packed") {
            line("packed_f32", case, dims, sets, |k| {
                black_box(k.distance_packed(black_box(&pa), black_box(&pb), (Packed::Single, term)));
            });
            line("packed_f16", case, dims, sets, |k| {
                black_box(k.distance_packed(black_box(&ha), black_box(&hb), (Packed::Half, term)));
            });
        }
        if wanted("distance_i8") {
            line("distance_i8", case, dims, sets, |k| {
                black_box(k.distance_i8(black_box(&ia), black_box(&ib), term));
            });
        }
    }
    if wanted("cosine_f32") {
        line("cosine_f32", "three sums", dims, sets, |k| {
            black_box(k.cosine_f32(black_box(&a), black_box(&b)));
        });
    }
}
