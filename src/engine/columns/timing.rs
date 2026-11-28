//! Timing only, never merged: what reading a chunk of 8-byte values as
//! words costs a row, by encoding, with the kernels given, and what a byte
//! read from memory far larger than the caches costs.
#![allow(missing_docs, clippy::all, clippy::pedantic, clippy::restriction)]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::encoded::{self, Body, Plan};
use super::fields::{Cells, Fields};
use super::packed::bits_for;
use super::runs::{Runs, Unpacked};
use super::{Chunk, nulls_len, word};
use crate::kernels::Kernels;
use crate::storage::lanes::Records;
use crate::value::{CompOp, DataType, Spelled, Value};

const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
/// A vector of rows, as a scan reads at a call.
const ROWS: usize = 2048;
/// How many chunks each case reads in turn, so that no branch of a decoder
/// is learned of one chunk read again and again.
const CHUNKS: usize = 128;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn unit(&mut self) -> f64 {
        (self.next() % (1 << 30)) as f64 / (1_u64 << 30) as f64
    }
    /// A value of `bits` bits, any of them set.
    fn of_bits(&mut self, bits: u32) -> i64 {
        let wide = (self.next() << 32) ^ self.next() ^ (self.next() << 53);
        if bits >= 64 { wide as i64 } else { (wide & ((1_u64 << bits) - 1)) as i64 }
    }
}

/// `rows` values in runs whose lengths are geometric of mean `mean`, each
/// run's value one of `distinct` if given, spread over `bits`; the least
/// and the greatest of `bits` are both among them.
fn values(rows: usize, mean: f64, bits: u32, distinct: Option<usize>, seed: u64) -> Vec<i64> {
    let mut rng = Rng(seed);
    let (least, most) = if bits >= 64 { (i64::MIN, i64::MAX) } else { (0, ((1_u64 << bits) - 1) as i64) };
    let mut pool: Vec<i64> = (0..distinct.unwrap_or(0)).map(|_| rng.of_bits(bits)).collect();
    if pool.len() >= 2 {
        pool[0] = least;
        pool[1] = most;
        pool.sort_unstable();
        pool.dedup();
        let mut fill = 1_i64;
        while pool.len() < distinct.unwrap_or(0) {
            if !pool.contains(&fill) {
                pool.push(fill);
            }
            fill += 1;
        }
    }
    let mut out = Vec::with_capacity(rows);
    let mut last = i64::MIN.wrapping_add(7);
    let mut n = 0_usize;
    while out.len() < rows {
        let mut v = last;
        while v == last {
            v = match distinct {
                Some(_) if n < pool.len() => pool[n],
                Some(_) => pool[(rng.next() as usize) % pool.len()],
                None if n == 0 => least,
                None if n == 1 => most,
                None => rng.of_bits(bits),
            };
            n += 1;
        }
        last = v;
        out.push(v);
        while out.len() < rows && mean > 1.0 && rng.unit() > 1.0 / mean {
            out.push(v);
        }
    }
    out
}

fn records_of(values: &[i64], wide: bool) -> Records {
    let mut records = Records { nulls: vec![0; values.len().div_ceil(8)], ..Default::default() };
    for &v in values {
        if wide {
            records.bytes.extend_from_slice(&i128::from(v).to_le_bytes());
        } else {
            records.bytes.extend_from_slice(&v.to_le_bytes());
        }
        records.ends.push(u32::try_from(records.bytes.len()).unwrap());
    }
    records
}

/// The chunk of `values` written by `plan`, whatever the sizer would choose.
fn chunk_of(values: &[i64], plan: impl Fn(i64, u32, usize) -> Plan) -> Vec<u8> {
    let (lo, hi) = (*values.iter().min().unwrap(), *values.iter().max().unwrap());
    let bits = bits_for(u64::try_from(i128::from(hi) - i128::from(lo)).unwrap());
    let mut seen = values.to_vec();
    seen.sort_unstable();
    seen.dedup();
    let plan = plan(lo, bits, seen.len());
    let recs = records_of(values, plan == Plan::Fixed(16));
    let mut out = vec![encoded::kind_of(plan)];
    out.extend_from_slice(&word(values.len()).unwrap());
    out.extend_from_slice(&vec![0_u8; nulls_len(values.len())]);
    encoded::encode(plan, Cells::whole(&recs), (None, false), &mut out).unwrap();
    out
}

/// Nanoseconds a row of `read`, given which of `CHUNKS` chunks to read:
/// the least and the median of seven passes over them all.
fn timed(mut read: impl FnMut(usize, &mut Vec<i64>)) -> (f64, f64) {
    const REPS: usize = 24;
    let mut out: Vec<i64> = Vec::with_capacity(ROWS + 64);
    let mut all = Vec::new();
    for _ in 0..7 {
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
        all.push(t0.elapsed().as_nanos() as f64 / (REPS * CHUNKS * ROWS) as f64);
    }
    all.sort_by(f64::total_cmp);
    (all[0], all[3])
}

fn runs_of(bytes: &[u8]) -> Runs<'_> {
    let Body::Runs(r) = Chunk::parse(bytes).unwrap().body else { panic!("runs") };
    r
}

fn seeds() -> impl Iterator<Item = u64> {
    (0..CHUNKS).map(|c| 7 + 1000 * u64::try_from(c).unwrap())
}

fn size_a_row(bytes: &[Vec<u8>]) -> f64 {
    bytes.iter().map(Vec::len).sum::<usize>() as f64 / (CHUNKS * ROWS) as f64
}

fn grid_runs(_kernels: &dyn Kernels) {
    for bits in [8_u32, 20, 32, 62] {
        for mean in [1.0_f64, 1.15, 1.3, 1.6, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 24.0, 32.0, 64.0, 128.0, 512.0] {
            let all: Vec<Vec<i64>> = seeds().map(|s| values(ROWS, mean, bits, None, s)).collect();
            let runs: usize = all.iter().map(|v| 1 + v.windows(2).filter(|w| w[0] != w[1]).count()).sum();
            let bytes: Vec<Vec<u8>> = all.iter().map(|v| chunk_of(v, |lo, b, _| Plan::Runs(lo, b))).collect();
            let mut check = Vec::new();
            runs_of(&bytes[3]).ints_old((0, ROWS), &mut check).unwrap();
            assert_eq!(check, all[3]);
            let old = timed(|c, out| runs_of(&bytes[c]).ints_old((0, ROWS), out).unwrap());
            let mut kept = Unpacked::default();
            let new = timed(|c, out| runs_of(&bytes[c]).ints_ends((0, ROWS), (out, None), &mut kept).unwrap());
            println!(
                "GRID RUNS bits={bits} mean_run={:.2} bytes_a_row={:.3} today={:.3} today_median={:.3} d1315={:.3} d1315_median={:.3}",
                (CHUNKS * ROWS) as f64 / runs as f64,
                size_a_row(&bytes),
                old.0,
                old.1,
                new.0,
                new.1
            );
        }
    }
}

fn grid_frames(kernels: &dyn Kernels) {
    for bits in 1..=64_u32 {
        let all: Vec<Vec<i64>> = seeds().map(|s| values(ROWS, 1.0, bits, None, s)).collect();
        let bytes: Vec<Vec<u8>> = all.iter().map(|v| chunk_of(v, |lo, b, _| Plan::Frame(lo, b))).collect();
        let mut check = Vec::new();
        Chunk::parse(&bytes[5]).unwrap().body.ints((0, ROWS), &mut check, (None, kernels)).unwrap();
        assert_eq!(check, all[5]);
        let ns = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, kernels)).unwrap());
        println!("GRID FRAME bits={bits} bytes_a_row={:.3} ns={:.3} median={:.3}", size_a_row(&bytes), ns.0, ns.1);
    }
    let all: Vec<Vec<i64>> = seeds().map(|s| values(ROWS, 1.0, 64, None, s)).collect();
    for width in [8_usize, 16] {
        let bytes: Vec<Vec<u8>> = all.iter().map(|v| chunk_of(v, |_, _, _| Plan::Fixed(width))).collect();
        let mut check = Vec::new();
        Chunk::parse(&bytes[5]).unwrap().body.ints((0, ROWS), &mut check, (None, kernels)).unwrap();
        assert_eq!(check, all[5]);
        let ns = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, kernels)).unwrap());
        println!("GRID FIXED width={width} bytes_a_row={:.3} ns={:.3} median={:.3}", size_a_row(&bytes), ns.0, ns.1);
    }
    let one: Vec<Vec<u8>> = seeds().map(|s| chunk_of(&vec![s as i64; ROWS], |_, _, _| Plan::Constant)).collect();
    let ns = timed(|c, out| Chunk::parse(&one[c]).unwrap().body.ints((0, ROWS), out, (None, kernels)).unwrap());
    println!("GRID CONSTANT bytes_a_row={:.3} ns={:.3} median={:.3}", size_a_row(&one), ns.0, ns.1);
}

fn grid_dictionary(kernels: &dyn Kernels) {
    for distinct in [2_usize, 4, 8, 16, 32, 64, 128, 200, 256, 400, 512, 700, 1024, 1500, 2048] {
        for entry_bits in [62_u32, 20] {
            let all: Vec<Vec<i64>> = seeds().map(|s| values(ROWS, 1.0, entry_bits, Some(distinct), s)).collect();
            let bytes: Vec<Vec<u8>> = all
                .iter()
                .map(|v| chunk_of(v, |_, _, entries| Plan::Dictionary(bits_for(u64::try_from(entries - 1).unwrap()))))
                .collect();
            let words: Vec<Vec<i64>> = bytes.iter().map(|b| Chunk::parse(b).unwrap().body.entry_words().unwrap()).collect();
            let mut check = Vec::new();
            Chunk::parse(&bytes[5]).unwrap().body.ints((0, ROWS), &mut check, (Some(&words[5]), kernels)).unwrap();
            assert_eq!(check, all[5]);
            let given = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (Some(&words[c]), kernels)).unwrap());
            let made = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, kernels)).unwrap());
            let entries = words[0].len();
            println!(
                "GRID DICTIONARY entries={entries} code_bits={} entry_bits={entry_bits} bytes_a_row={:.3} gather={:.3} gather_median={:.3} with_entries_made={:.3} ns_an_entry={:.3}",
                bits_for(u64::try_from(entries - 1).unwrap()),
                size_a_row(&bytes),
                given.0,
                given.1,
                made.0,
                (made.0 - given.0) * ROWS as f64 / entries as f64
            );
        }
    }
}

fn grid_alp(kernels: &dyn Kernels) {
    for bits in [7_u32, 8, 12, 16, 20, 24, 32, 40] {
        let all: Vec<Vec<i64>> = seeds().map(|s| values(ROWS, 1.0, bits, None, s)).collect();
        let bytes: Vec<Vec<u8>> = all
            .iter()
            .map(|v| {
                let fields = Fields::of(&DOUBLE, v.iter().map(|&k| Value::Approx(k as f64 / 100.0))).unwrap();
                fields.chunk(&DOUBLE, 0, (fields.len(), true)).unwrap()
            })
            .collect();
        let kinds: Vec<u8> = bytes.iter().map(|b| b[0]).collect();
        let ns = timed(|c, out| Chunk::parse(&bytes[c]).unwrap().body.ints((0, ROWS), out, (None, kernels)).unwrap());
        println!(
            "GRID ALP digits_bits={bits} kind_first={} kinds_alike={} bytes_a_row={:.3} ns={:.3} median={:.3}",
            kinds[0],
            kinds.iter().all(|&k| k == kinds[0]),
            size_a_row(&bytes),
            ns.0,
            ns.1
        );
    }
}

/// One chunk a page of about 8 KiB of the encoding `what`, with its rows.
fn page(what: &str, seed: u64) -> (Vec<u8>, usize) {
    let made = |rows: usize, mean: f64, bits: u32, distinct: Option<usize>, plan: &dyn Fn(i64, u32, usize) -> Plan| {
        (chunk_of(&values(rows, mean, bits, distinct, seed), plan), rows)
    };
    match what {
        "fixed" => made(1000, 1.0, 64, None, &|_, _, _| Plan::Fixed(8)),
        "frame62" => made(1040, 1.0, 62, None, &|lo, b, _| Plan::Frame(lo, b)),
        "frame32" => made(2000, 1.0, 32, None, &|lo, b, _| Plan::Frame(lo, b)),
        "frame20" => made(3200, 1.0, 20, None, &|lo, b, _| Plan::Frame(lo, b)),
        "frame8" => made(7900, 1.0, 8, None, &|lo, b, _| Plan::Frame(lo, b)),
        "dict9" => made(1437, 1.0, 62, Some(500), &|_, _, e| Plan::Dictionary(bits_for(u64::try_from(e - 1).unwrap()))),
        "dict8" => made(4000, 1.0, 62, Some(256), &|_, _, e| Plan::Dictionary(bits_for(u64::try_from(e - 1).unwrap()))),
        "runs5" | "runs5new" => made(3731, 5.0, 62, None, &|lo, b, _| Plan::Runs(lo, b)),
        "runs13" | "runs13new" => made(1150, 1.3, 62, None, &|lo, b, _| Plan::Runs(lo, b)),
        other => panic!("no such encoding {other}"),
    }
}

/// Reads the pages of `what` laid end to end over `mib` MiB, each as a scan
/// does (parsed, its values made words a vector at a time, a comparison
/// counted), from the second `start` for `secs` seconds; prints the least
/// and the median pass in nanoseconds a row.
fn scan(kernels: &dyn Kernels, what: &str, mib: usize, start: u64, secs: u64) {
    let distinct: Vec<(Vec<u8>, usize)> = (0..128_u64).map(|s| page(what, 11 + 977 * s)).collect();
    let total = mib << 20;
    let mut buffer: Vec<u8> = Vec::with_capacity(total + 16384);
    let mut pages: Vec<(usize, usize, usize)> = Vec::new();
    let mut n = 0_usize;
    while buffer.len() < total {
        let (bytes, rows) = &distinct[n % distinct.len()];
        pages.push((buffer.len(), bytes.len(), *rows));
        buffer.extend_from_slice(bytes);
        n += 1;
    }
    let rows: usize = pages.iter().map(|p| p.2).sum();
    let now = || SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    while now() < start {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (mut out, mut sel, mut kept) = (Vec::with_capacity(8192 + 64), Vec::with_capacity(8192), Unpacked::default());
    let new_runs = what.ends_with("new");
    let begun = Instant::now();
    let mut passes = Vec::new();
    let mut counted = 0_usize;
    while begun.elapsed() < Duration::from_secs(secs) || passes.len() < 3 {
        let t0 = Instant::now();
        let mut reps = 0_usize;
        // A small set of pages is read many times a pass, so a pass is long enough to time.
        while reps == 0 || reps.saturating_mul(rows) < 20_000_000 {
            for &(at, len, count) in &pages {
                let chunk = Chunk::parse(&buffer[at..at + len]).unwrap();
                let words = chunk.body.entry_words();
                let mut from = 0_usize;
                while from < count {
                    let to = (from + ROWS).min(count);
                    out.clear();
                    match &chunk.body {
                        Body::Runs(r) if new_runs => r.ints_ends((from, to), (&mut out, None), &mut kept).unwrap(),
                        Body::Runs(r) => r.ints_old((from, to), &mut out).unwrap(),
                        body => body.ints((from, to), &mut out, (words.as_deref(), kernels)).unwrap(),
                    }
                    sel.clear();
                    kernels.select_i64(&out, (CompOp::Ne, 12345), (None, None), &mut sel);
                    counted = counted.wrapping_add(sel.len());
                    from = to;
                }
            }
            reps += 1;
        }
        passes.push(t0.elapsed().as_nanos() as f64 / (reps * rows) as f64);
    }
    std::hint::black_box(counted);
    passes.sort_by(f64::total_cmp);
    println!(
        "SCAN what={what} mib={mib} bytes_a_row={:.3} rows={rows} passes={} least={:.3} median={:.3} ns_a_row",
        buffer.len() as f64 / rows as f64,
        passes.len(),
        passes[0],
        passes[passes.len() / 2]
    );
}

/// Runs the timing `mode` names with `kernels`: `grid`, or
/// `scan WHAT MIB START SECS`.
pub fn run(kernels: &dyn Kernels, mode: &str) {
    let words: Vec<&str> = mode.split_whitespace().collect();
    match words.first().copied() {
        Some("scan") => {
            let number = |n: usize| words.get(n).and_then(|w| w.parse::<u64>().ok()).unwrap_or(0);
            scan(kernels, words.get(1).copied().unwrap_or("fixed"), usize::try_from(number(2)).unwrap().max(1), number(3), number(4).max(1));
        }
        Some("runs") => grid_runs(kernels),
        Some("frames") => grid_frames(kernels),
        Some("dictionary") => grid_dictionary(kernels),
        Some("alp") => grid_alp(kernels),
        _ => {
            println!("GRID kernels={}", kernels.name());
            grid_frames(kernels);
            grid_dictionary(kernels);
            grid_alp(kernels);
            grid_runs(kernels);
        }
    }
}
