//! How fast each kernel runs at each level this processor has, beside the
//! portable bodies (RULES.md D570, R-BENCH-2): nanoseconds a row, the
//! median of repeated runs, of vectors of a few sizes, selections at a
//! low and a middle selectivity. Run with `cargo run --release -p
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
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

use std::hint::black_box;
use std::time::Instant;

use turborel::kernels::{AlpScale, Kernels, Portable};
use turborel::storage::Mask;
use turborel::value::CompOp;
use turborel_simd::{Level, Simd};

/// The sizes measured.
const SIZES: [usize; 4] = [64, 2048, 16_384, 1 << 20];

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

fn main() {
    let only = std::env::var("ONLY").ok();
    let wanted = |name: &str| only.as_deref().is_none_or(|o| o == name);
    let sets = sets();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    println!("kernel       case             rows  ns a row at each set (speed against portable)");
    for size in SIZES {
        let ints: Vec<i64> = (0..size).map(|_| (rng.next() % 1000) as i64).collect();
        let floats: Vec<f64> = ints.iter().map(|&v| v as f64 + 0.5).collect();
        let nulls = Mask::from_bools(&(0..size).map(|_| rng.next() % 10 == 0).collect::<Vec<_>>());
        let sel: Vec<u32> = (0..size as u32).filter(|_| rng.next() % 2 == 0).collect();
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
        if wanted("sum_i64") {
            line("sum_i64", "all", size, &sets, |k| {
                black_box(k.sum_i64(black_box(&ints), None));
            });
        }
        if wanted("min_max_i64") {
            line("min_max_i64", "all", size, &sets, |k| {
                black_box(k.min_max_i64(black_box(&ints), (None, None)));
            });
            line("min_max_i64", "nulls", size, &sets, |k| {
                black_box(k.min_max_i64(black_box(&ints), (Some(&nulls), None)));
            });
        }
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
}
