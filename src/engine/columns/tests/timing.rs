//! Timing only, never merged: what reading a chunk of 8-byte values as
//! words costs a row, by encoding.

use std::time::Instant;

use super::super::encoded::{self, Body, Plan};
use super::super::fields::Cells;
use super::super::packed::bits_for;
use super::super::*;
use super::{int, records};
use crate::kernels::Portable;
use crate::value::Spelled;

const T: DataType = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };
const ROWS: usize = 3731;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn unit(&mut self) -> f64 {
        (self.next() % (1 << 30)) as f64 / (1_u64 << 30) as f64
    }
}

/// `ROWS` values in runs whose lengths are geometric of mean `mean`, each run's value one of `distinct` if given, spread over `bits`.
fn values(mean: f64, bits: u32, distinct: Option<usize>, seed: u64) -> Vec<i64> {
    let mut rng = Rng(seed);
    let pool: Vec<i64> = (0..distinct.unwrap_or(0)).map(|_| (rng.next() % (1_u64 << bits.min(52))) as i64 * (1_i64 << bits.saturating_sub(52))).collect();
    let mut out = Vec::with_capacity(ROWS);
    let mut last = i64::MIN;
    while out.len() < ROWS {
        let mut v = last;
        while v == last {
            v = match distinct {
                Some(d) => pool[(rng.next() as usize) % d],
                None => (rng.next() % (1_u64 << bits.min(52))) as i64 * (1_i64 << bits.saturating_sub(52)),
            };
        }
        last = v;
        out.push(v);
        while out.len() < ROWS && rng.unit() > 1.0 / mean {
            out.push(v);
        }
    }
    out
}

/// The chunk of `values` written by `plan`, whatever the sizer would choose.
fn chunk_of(values: &[i64], plan: impl Fn(i64, u32, usize) -> Plan) -> Vec<u8> {
    let recs = records(&T, &values.iter().map(|&v| int(i128::from(v))).collect::<Vec<_>>());
    let (lo, hi) = (*values.iter().min().unwrap(), *values.iter().max().unwrap());
    let bits = bits_for(u64::try_from(i128::from(hi) - i128::from(lo)).unwrap());
    let mut seen = values.to_vec();
    seen.sort_unstable();
    seen.dedup();
    let plan = plan(lo, bits, seen.len());
    let mut out = vec![encoded::kind_of(plan)];
    out.extend_from_slice(&word(values.len()).unwrap());
    out.extend_from_slice(&vec![0_u8; nulls_len(values.len())]);
    encoded::encode(plan, Cells::whole(&recs), (None, false), &mut out).unwrap();
    out
}

/// Nanoseconds a row of `read`, the least of five passes of `REPS` calls.
fn timed(mut read: impl FnMut(&mut Vec<i64>)) -> f64 {
    const REPS: usize = 4000;
    let mut out: Vec<i64> = Vec::with_capacity(ROWS);
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut sum = 0_i64;
        for _ in 0..REPS {
            out.clear();
            read(&mut out);
            sum = sum.wrapping_add(out[ROWS / 2]);
        }
        std::hint::black_box(sum);
        best = best.min(t0.elapsed().as_nanos() as f64 / (REPS * ROWS) as f64);
    }
    best
}

#[test]
fn timing_decoders() {
    let check = |bytes: &[u8], values: &[i64]| {
        let chunk = Chunk::parse(bytes).unwrap();
        let mut out = Vec::new();
        chunk.body.ints((0, ROWS), &mut out, (None, &Portable)).unwrap();
        assert_eq!(out, values);
    };
    for bits in [62_u32, 20] {
        for mean in [1.3_f64, 2.0, 3.0, 5.0, 8.0, 16.0, 64.0] {
            let v = values(mean, bits, None, 7);
            let runs = 1 + v.windows(2).filter(|w| w[0] != w[1]).count();
            let bytes = chunk_of(&v, |lo, b, _| Plan::Runs(lo, b));
            check(&bytes, &v);
            let chunk = Chunk::parse(&bytes).unwrap();
            let present = timed(|out| chunk.body.ints((0, ROWS), out, (None, &Portable)).unwrap());
            let Body::Runs(r) = &chunk.body else { panic!("runs") };
            let mut scratch = (Vec::new(), Vec::new());
            let mut filled = Vec::new();
            r.ints_fill((0, ROWS), &mut filled, &mut scratch).unwrap();
            assert_eq!(filled, v);
            let fill = timed(|out| r.ints_fill((0, ROWS), out, &mut scratch).unwrap());
            let run = ROWS as f64 / runs as f64;
            eprintln!("GRID RUNS bits={bits} mean_run={run:.2} bytes_a_row={:.2} present={present:.2} fill={fill:.2} ns/row", bytes.len() as f64 / ROWS as f64);
        }
        let v = values(1.0, bits, None, 7);
        for (name, plan) in [("FRAME", 0_u8), ("FIXED", 1)] {
            let bytes = chunk_of(&v, |lo, b, _| if plan == 0 { Plan::Frame(lo, b) } else { Plan::Fixed(8) });
            check(&bytes, &v);
            let chunk = Chunk::parse(&bytes).unwrap();
            let ns = timed(|out| chunk.body.ints((0, ROWS), out, (None, &Portable)).unwrap());
            eprintln!("GRID {name} bits={bits} bytes_a_row={:.2} present={ns:.2} ns/row", bytes.len() as f64 / ROWS as f64);
        }
    }
    for (mean, distinct) in [(1.3_f64, 200_usize), (1.3, 500), (1.3, 1000), (2.0, 500), (5.0, 300), (5.0, 800)] {
        let v = values(mean, 62, Some(distinct), 11);
        let bytes = chunk_of(&v, |_, _, entries| Plan::Dictionary(bits_for(u64::try_from(entries - 1).unwrap())));
        check(&bytes, &v);
        let chunk = Chunk::parse(&bytes).unwrap();
        let words = chunk.body.entry_words().unwrap();
        let given = timed(|out| chunk.body.ints((0, ROWS), out, (Some(&words), &Portable)).unwrap());
        let made = timed(|out| chunk.body.ints((0, ROWS), out, (None, &Portable)).unwrap());
        let runs = 1 + v.windows(2).filter(|w| w[0] != w[1]).count();
        eprintln!(
            "GRID DICTIONARY entries={} code_bits={} mean_run={:.2} bytes_a_row={:.2} entries_given={given:.2} entries_made_each_call={made:.2} ns/row",
            words.len(),
            bits_for(u64::try_from(words.len() - 1).unwrap()),
            ROWS as f64 / runs as f64,
            bytes.len() as f64 / ROWS as f64
        );
    }
}
