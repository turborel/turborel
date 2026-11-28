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
/// A vector of rows, as a scan reads at a call.
const ROWS: usize = 2048;

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

/// How many chunks apart each case reads in turn, so that no branch of a
/// decoder is learned of one chunk read again and again.
const CHUNKS: usize = 128;

/// Nanoseconds a row of `read`, given which of `CHUNKS` chunks to read, the least of five passes over them all.
fn timed(mut read: impl FnMut(usize, &mut Vec<i64>)) -> f64 {
    const REPS: usize = 32;
    let mut out: Vec<i64> = Vec::with_capacity(ROWS + 64);
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t0 = Instant::now();
        let mut sum = 0_i64;
        for _ in 0..REPS {
            for c in 0..CHUNKS {
                out.clear();
                read(c, &mut out);
                sum = sum.wrapping_add(out[ROWS / 2]);
            }
        }
        std::hint::black_box(sum);
        best = best.min(t0.elapsed().as_nanos() as f64 / (REPS * CHUNKS * ROWS) as f64);
    }
    best
}

fn runs_of(bytes: &[u8]) -> super::super::runs::Runs<'_> {
    let Body::Runs(r) = Chunk::parse(bytes).unwrap().body else { panic!("runs") };
    r
}

#[test]
fn timing_decoders() {
    let seeds = || (0..CHUNKS).map(|c| 7 + 1000 * u64::try_from(c).unwrap());
    for bits in [62_u32, 20] {
        for mean in [1.3_f64, 2.0, 3.0, 5.0, 8.0, 16.0, 64.0] {
            let all: Vec<Vec<i64>> = seeds().map(|s| values(mean, bits, None, s)).collect();
            let runs: usize = all.iter().map(|v| 1 + v.windows(2).filter(|w| w[0] != w[1]).count()).sum();
            let bytes: Vec<Vec<u8>> = all.iter().map(|v| chunk_of(v, |lo, b, _| Plan::Runs(lo, b))).collect();
            let mut check = Vec::new();
            runs_of(&bytes[3]).ints_old((0, ROWS), &mut check).unwrap();
            assert_eq!(check, all[3]);
            let old = timed(|c, out| runs_of(&bytes[c]).ints_old((0, ROWS), out).unwrap());
            let mut kept = super::super::runs::Unpacked::default();
            let new = timed(|c, out| runs_of(&bytes[c]).ints_ends((0, ROWS), (out, None), &mut kept).unwrap());
            let size: usize = bytes.iter().map(Vec::len).sum();
            eprintln!(
                "GRID RUNS bits={bits} mean_run={:.2} bytes_a_row={:.2} old={old:.2} d1315_kept={new:.2} ns/row",
                (CHUNKS * ROWS) as f64 / runs as f64,
                size as f64 / (CHUNKS * ROWS) as f64
            );
        }
        let all: Vec<Vec<i64>> = seeds().map(|s| values(1.0, bits, None, s)).collect();
        for (name, plan) in [("FRAME", 0_u8), ("FIXED", 1)] {
            let bytes: Vec<Vec<u8>> = all.iter().map(|v| chunk_of(v, |lo, b, _| if plan == 0 { Plan::Frame(lo, b) } else { Plan::Fixed(8) })).collect();
            let ns = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, &Portable)).unwrap());
            eprintln!("GRID {name} bits={bits} present={ns:.2} ns/row");
        }
    }
    for (mean, distinct) in [(1.3_f64, 200_usize), (1.3, 500), (1.3, 1000), (5.0, 300)] {
        let all: Vec<Vec<i64>> = seeds().map(|s| values(mean, 62, Some(distinct), s)).collect();
        let bytes: Vec<Vec<u8>> = all
            .iter()
            .map(|v| chunk_of(v, |_, _, entries| Plan::Dictionary(bits_for(u64::try_from(entries - 1).unwrap()))))
            .collect();
        let words: Vec<Vec<i64>> = bytes.iter().map(|b| Chunk::parse(b).unwrap().body.entry_words().unwrap()).collect();
        let given = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (Some(&words[c]), &Portable)).unwrap());
        let made = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, &Portable)).unwrap());
        eprintln!("GRID DICTIONARY entries={} code_bits={} mean_run={mean} entries_given={given:.2} entries_made_each_call={made:.2} ns/row", words[0].len(), bits_for(u64::try_from(words[0].len() - 1).unwrap()));
    }
}
