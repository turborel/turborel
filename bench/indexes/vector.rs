//! The vector index methods "diskann", "hnsw" and "ivf" through `Index`,
//! apart from SQL. Each is built of the same clustered vectors one insert
//! at a time, as CREATE INDEX fills one while writers go on, then searched
//! approximately with a sweep of each method's knob: DiskANN's search
//! list L and beam width W, HNSW's efSearch, IVF's NPROBE. Each sweep
//! point reports its queries a second and, as a note, its recall@10
//! against the exact ten nearest by COSINE. Set TURBOREL_VECTOR_ROWS for
//! scale (default 20000) and TURBOREL_VECTOR_METHOD to one method. The
//! build's time includes the maintenance due as it goes.

use std::collections::HashSet;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use super::harness::{self, Bench, Case};
use turborel::functions::ScalarFunction;
use turborel::index::{Approximation, DiskAnnMethod, FilterHint, HnswMethod, Index, IndexMethod, IvfMethod, Key, NearestQuery};
use turborel::memory::MemoryBudget;
use turborel::value::{DataType, Decimal, Value};
use turborel::vector::{Element, Layout, Shape, functions, typed};

const ROWS: u64 = 20_000;
const DIMS: usize = 64;
const QUERIES: usize = 200;
const K: usize = 10;
const CENTRES: u64 = 32;
/// Queries use a part of the number stream the rows do not.
const QUERY_OFFSET: u64 = 1_u64 << 40;
const DISKANN_LISTS: [u32; 5] = [10, 20, 40, 80, 160];
const DISKANN_WIDTHS: [u32; 2] = [1, 4];
const HNSW_EF: [u32; 5] = [10, 20, 40, 80, 160];
const IVF_PROBES: [u32; 5] = [1, 2, 4, 8, 16];

/// The build and the sweep of each method, or of TURBOREL_VECTOR_METHOD's
/// alone.
pub fn workloads(bench: &Bench) {
    let rows = std::env::var("TURBOREL_VECTOR_ROWS").ok().map_or(ROWS, |s| s.parse().unwrap());
    let selected = std::env::var("TURBOREL_VECTOR_METHOD").ok();
    let data: Vec<Vec<f32>> = (0..rows).map(vector_of).collect();
    let queries: Vec<Vec<f32>> = (0..QUERIES as u64).map(|n| vector_of(QUERY_OFFSET + n)).collect();
    let exact: Vec<HashSet<u64>> = queries.iter().map(|q| exact_nearest(&data, q)).collect();
    let methods: [(&str, Box<dyn IndexMethod>); 3] = [
        ("diskann", Box::new(DiskAnnMethod::in_memory())),
        ("hnsw", Box::new(HnswMethod::in_memory())),
        ("ivf", Box::new(IvfMethod::in_memory())),
    ];
    for (name, method) in methods {
        if selected.as_deref().is_some_and(|s| s != name) {
            continue;
        }
        let index = build(name, &*method, &data);
        let sweep: Vec<(String, Approximation)> = match name {
            "diskann" => DISKANN_LISTS
                .iter()
                .flat_map(|l| DISKANN_WIDTHS.iter().map(move |w| (format!("L={l} W={w}"), asked(Some(*l), Some(*w), None))))
                .collect(),
            "hnsw" => HNSW_EF.iter().map(|ef| (format!("ef={ef}"), asked(Some(*ef), None, None))).collect(),
            _ => IVF_PROBES.iter().map(|p| (format!("probes={p}"), asked(None, None, Some(*p)))).collect(),
        };
        for (label, approximation) in sweep {
            let recall = search(&index, &queries, approximation, Some(&exact));
            bench.measure(Case::new(format!("{name} {label}, {rows}x{DIMS}"), "queries"), || {
                black_box(search(&index, &queries, approximation, None));
                QUERIES as u64
            });
            harness::note(&format!("{name} {label}: recall@{K} {recall:.3}"));
        }
    }
}

/// An index of `method` holding `data`, timed.
fn build(name: &str, method: &dyn IndexMethod, data: &[Vec<f32>]) -> Arc<dyn Index> {
    let index: Arc<dyn Index> = Arc::from(method.create(&[column()]).unwrap());
    let started = Instant::now();
    for (row, v) in data.iter().enumerate() {
        index.insert(Key(vec![vector_value(v)]), row as u64).unwrap();
        // Maintenance as the database's job does it while writers go on,
        // so DiskANN's codes are trained, and its cache made, as it grows.
        if index.maintenance_due() {
            index.maintain().unwrap();
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    harness::note(&format!("{name} built of {} inserts in {seconds:.2} s, {:.0} a second", data.len(), data.len() as f64 / seconds));
    while index.maintenance_due() {
        if !index.maintain().unwrap() {
            break;
        }
    }
    index
}

fn asked(search_list: Option<u32>, beam_width: Option<u32>, probes: Option<u32>) -> Approximation {
    Approximation { first: K, search_list, beam_width, probes, accuracy: None, widen: 1, radius: None, filter: FilterHint::None }
}

/// The share of the exact nearest found over every query, if `exact` is
/// given; else 0.
fn search(index: &Arc<dyn Index>, queries: &[Vec<f32>], approximation: Approximation, exact: Option<&[HashSet<u64>]>) -> f64 {
    let budget = Arc::new(MemoryBudget::new(1 << 30));
    let mut found = 0_usize;
    for (n, q) in queries.iter().enumerate() {
        let query = NearestQuery {
            function: turborel::functions::ScalarRef(cosine()),
            column: 0,
            argument: 0,
            value: vector_value(q),
            descending: false,
            statistics: None,
            metric: Some("COSINE".to_owned()),
            approximate: Some(approximation),
        };
        let entries = Arc::clone(index).approximate(&query, &budget, None).unwrap().unwrap();
        let rows: Vec<u64> = entries.take(K).map(|e| e.unwrap().2).collect();
        if let Some(exact) = exact {
            found += rows.iter().filter(|r| exact[n].contains(r)).count();
        }
    }
    found as f64 / (queries.len() * K) as f64
}

fn column() -> DataType {
    typed(Shape { dims: Some(DIMS as u32), element: Some(Element::Float32), layout: Some(Layout::Dense) }).unwrap()
}

fn function(name: &str) -> Arc<dyn ScalarFunction> {
    functions().into_iter().find(|f| f.name() == name).unwrap()
}

fn cosine() -> Arc<dyn ScalarFunction> {
    function("COSINE_DISTANCE")
}

/// `v` as a VECTOR value, by TO_VECTOR.
fn vector_value(v: &[f32]) -> Value {
    let text: Vec<String> = v.iter().map(f32::to_string).collect();
    let args = [Value::Char(format!("[{}]", text.join(","))), Value::Exact(Decimal::new(DIMS as i128, 0)), Value::Char("FLOAT32".into())];
    function("TO_VECTOR").call(&args).unwrap()
}

/// The `K` rows of `data` nearest `q` by COSINE.
fn exact_nearest(data: &[Vec<f32>], q: &[f32]) -> HashSet<u64> {
    let mut all: Vec<(f32, u64)> = data.iter().enumerate().map(|(row, v)| (cosine_distance(v, q), row as u64)).collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    all.into_iter().take(K).map(|(_, row)| row).collect()
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norms = a.iter().map(|x| x * x).sum::<f32>().sqrt() * b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norms > 0.0 { 1.0 - dot / norms } else { 1.0 }
}

/// SplitMix64's finalizer: the same numbers each run, no RNG.
fn unit(mut n: u64) -> f32 {
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    n ^= n >> 31_u32;
    ((n >> 11) as f64 / (1_u64 << 53) as f64) as f32
}

/// Vector `n`: near one of `CENTRES` centres, as embeddings cluster.
fn vector_of(n: u64) -> Vec<f32> {
    let centre = n % CENTRES;
    (0..DIMS as u64).map(|d| unit(centre * 1_000_003 + d) * 2.0 - 1.0 + (unit(n * 7919 + d + 17) - 0.5) * 0.6).collect()
}
