//! Counted entries, deletes by tombstone, maintenance's repair, and the
//! quantizer trained by maintenance.

use std::sync::atomic::Ordering::Relaxed;

use super::*;

/// Verifies: [EXTENSION] entries are counted: a pair inserted twice stays
/// after one remove and goes after the second; removing a pair not held
/// changes nothing (the index contract of `index.rs`).
#[test]
fn hnsw_entries_are_counted() {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(3), (euclidean(), None), &budget).unwrap();
    let v = [1.0, 2.0, 3.0];
    hnsw.insert(&v, 5).unwrap();
    hnsw.insert(&v, 5).unwrap();
    hnsw.insert(&v, 6).unwrap();
    assert_eq!(hnsw.rows(&v).unwrap(), vec![5, 6]);
    assert_eq!(hnsw.counts().unwrap(), (2, 0));
    hnsw.remove(&v, 5).unwrap();
    assert_eq!(hnsw.rows(&v).unwrap(), vec![5, 6]);
    hnsw.remove(&v, 5).unwrap();
    hnsw.remove(&v, 5).unwrap();
    hnsw.remove(&[9.0, 9.0, 9.0], 6).unwrap();
    assert_eq!(hnsw.rows(&v).unwrap(), vec![6]);
    assert_eq!(hnsw.counts().unwrap(), (1, 1));
    hnsw.insert(&v, 5).unwrap();
    assert_eq!(hnsw.counts().unwrap(), (2, 0), "a tombstone inserted again lives again, as an undo needs");
}

/// Verifies: [EXTENSION] removed rows are never returned, before or after
/// maintenance (R10.10 past 20% tombstones) repairs and drops them, and
/// recall over the rows left stays at least 95% after it.
#[test]
fn hnsw_deletes_are_never_returned() {
    let dims = 16;
    let flat = clustered(2_000, dims, 10, 13);
    let (hnsw, _budget) = built(&flat, dims, None);
    let gone = |r: u64| r % 10 < 3;
    for (i, v) in flat.chunks_exact(dims).enumerate().filter(|(i, _)| gone(row(*i))) {
        hnsw.remove(v, row(i)).unwrap();
    }
    assert!(hnsw.maintenance_due());
    let search = |q: &[f32]| {
        let found = hnsw.nearest(q, 10, Search::default()).unwrap();
        assert!(found.iter().all(|(_, r)| !gone(*r)), "a removed row was returned");
        found
    };
    let before = mean_recall(&search, &flat, dims, &|r| !gone(r));
    assert!(hnsw.maintain().unwrap());
    assert_eq!(hnsw.counts().unwrap(), (1_400, 0));
    assert!(!hnsw.maintenance_due());
    let after = mean_recall(&search, &flat, dims, &|r| !gone(r));
    assert!(before >= 0.9_f64 && after >= 0.95_f64, "recall {before} before repair, {after} after");
}

/// Verifies: [DESIGN] maintenance makes progress under constant writes
/// (`fence.rs`): while a writer is past the fence, a repair is not swapped
/// in, each lost swap counted; after `MOST_LOST` it raises the fence,
/// waits for the writer, and swaps its repair in, the count cleared.
#[test]
fn hnsw_maintenance_makes_progress_under_constant_writes() {
    let dims = 16;
    let flat = clustered(1_000, dims, 10, 17);
    let (hnsw, _budget) = built(&flat, dims, None);
    for (i, v) in flat.chunks_exact(dims).enumerate().filter(|(i, _)| row(*i).is_multiple_of(3)) {
        hnsw.remove(v, row(i)).unwrap();
    }
    let writer = hnsw.inner.fence.pass().unwrap();
    for lost in 1..=fence::MOST_LOST {
        assert!(!hnsw.maintain().unwrap(), "a writer is past the fence");
        assert_eq!(hnsw.inner.lost.load(Relaxed), lost);
    }
    let done = std::thread::scope(|scope| {
        let fenced = scope.spawn(|| hnsw.maintain().unwrap());
        while !hnsw.inner.fence.is_raised() && !fenced.is_finished() {
            std::thread::yield_now();
        }
        drop(writer);
        fenced.join().unwrap()
    });
    assert!(done && !hnsw.maintenance_due(), "the fenced repair was swapped in");
    assert_eq!(hnsw.inner.lost.load(Relaxed), 0);
}

/// Verifies: [EXTENSION] an index naming a quantizer keeps full vectors
/// until it holds enough rows, then maintenance trains its SQ8 codec
/// (R7.2) and the nodes keep codes; with RERANK (R7.1) re-scoring its
/// candidates exactly, recall@10 stays at least 95%.
#[test]
fn hnsw_quantized_by_maintenance_with_rerank() {
    let dims = 32;
    let flat = clustered(2_000, dims, 20, 17);
    let quantizer: Arc<dyn Quantizer> = Arc::new(Sq8);
    let (hnsw, _budget) = built(&flat, dims, Some(quantizer));
    assert!(!hnsw.quantized().unwrap());
    assert!(hnsw.maintenance_due());
    assert!(hnsw.maintain().unwrap());
    assert!(hnsw.quantized().unwrap());
    let metric = euclidean();
    let exact_of = |q: &[f32], r: u64| {
        let at = usize::try_from(r).unwrap().wrapping_mul(dims);
        Some(metric.distance(q, &flat[at..at.wrapping_add(dims)]))
    };
    let search = |q: &[f32]| {
        let mut exact = |r: u64| exact_of(q, r);
        let found = hnsw.nearest(q, 10, Search { exact: Some(&mut exact), ..Search::default() }).unwrap();
        assert_eq!(found.len(), 10);
        found
    };
    let r = mean_recall(&search, &flat, dims, &|_| true);
    assert!(r >= 0.95_f64, "quantized recall@10 {r}");
    let rough = hnsw.nearest(&flat[..dims], 10, Search::default()).unwrap();
    assert_eq!(rough.len(), 10_usize.saturating_mul(RERANK_FACTOR), "unranked, RERANK_FACTOR candidates a row");
    hnsw.insert(&flat[..dims], 9_999).unwrap();
    assert!(hnsw.rows(&flat[..dims]).unwrap().contains(&9_999));
}

/// Verifies: [DESIGN] the SQ8 codec rounds each dimension to its nearest
/// of 256 steps between the sample's least and greatest, and its trained
/// state reads back as written.
#[test]
fn sq8_codes_round_trip() {
    let sample = [0.0_f32, -1.0, 1.0, 1.0, 0.5, 0.0];
    let codec = Sq8.train(&sample, 2, 0).unwrap();
    let mut code = [0_u8; 2];
    codec.encode(&[1.0, -1.0], &mut code);
    assert_eq!(code, [255, 0]);
    codec.encode(&[0.25, -0.5], &mut code);
    assert_eq!(code, [64, 64]);
    let mut out = [0.0_f32; 2];
    codec.decode(&code, &mut out);
    assert!((out[0] - 0.25).abs() < 0.01 && (out[1] + 0.5).abs() < 0.01);
    let mut bytes = Vec::new();
    codec.write(&mut bytes);
    let again = Sq8.read(&bytes, 2).unwrap();
    let mut out2 = [0.0_f32; 2];
    again.decode(&code, &mut out2);
    assert_eq!(out, out2);
    assert_eq!(Sq8.read(&bytes[..4], 2).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md R-ERR-4: a search's filter runs with no
/// lock of the index held, so a filter that reads the index again
/// (`rows`) and writes it (`insert`) completes. A regression test for the
/// R-ERR-4 deviation: the search held the graph's read lock across the
/// filter, and an insert from it waited for that lock forever.
#[test]
fn hnsw_filter_may_use_the_index() {
    let dims = 8;
    let flat = clustered(300, dims, 5, 41);
    let (hnsw, _budget) = built(&flat, dims, None);
    let other = hnsw.clone();
    let mut extra = 10_000_u64;
    let mut filter = |r: u64| {
        assert!(other.rows(&flat[..dims]).unwrap().contains(&0));
        if extra < 10_005 {
            // A few, as a scan reads on into what is appended meanwhile.
            other.insert(&flat[dims..dims * 2], extra).unwrap();
            extra += 1;
        }
        r.is_multiple_of(2)
    };
    let found = hnsw.nearest(&flat[..dims], 10, Search { filter: Some(&mut filter), ..Search::default() }).unwrap();
    assert!(!found.is_empty() && found.iter().all(|(_, r)| r.is_multiple_of(2)));
    let mut again = |r: u64| other.rows(&flat[..dims]).unwrap().contains(&r) || r.is_multiple_of(3);
    let within = hnsw.range(&flat[..dims], 0.5, Search { filter: Some(&mut again), ..Search::default() }).unwrap();
    assert!(within.iter().all(|(d, _)| *d <= 0.5));
    assert!(hnsw.rows(&flat[dims..dims * 2]).unwrap().len() > 1, "the filter's inserts were made");
}

/// Verifies: [DESIGN] `entries` gives the row of every live node once,
/// tombstones left out, whatever is inserted as it is read.
#[test]
fn hnsw_entries_gives_each_live_node() {
    let dims = 4;
    let flat = clustered(2_500, dims, 5, 43);
    let (hnsw, _budget) = built(&flat, dims, None);
    hnsw.remove(&flat[..dims], 0).unwrap();
    let mut rows: Vec<u64> = hnsw.entries().map(Result::unwrap).collect();
    rows.sort_unstable();
    assert_eq!(rows, (1..2_500).map(row).collect::<Vec<_>>());
    let mut reading = hnsw.entries();
    assert_eq!(reading.next().unwrap().unwrap(), 1);
    hnsw.insert(&[9.0, 9.0, 9.0, 9.0], 7_777).unwrap();
    assert_eq!(reading.count(), 2_499, "a node appended meanwhile is given too");
}

/// Verifies: [DESIGN] RULES.md D97: a node keeps the squared norm of the
/// vector a search reads, its floats' and, once coded, its code's decoded,
/// through compaction too.
#[test]
fn hnsw_nodes_keep_the_norms_searches_read() {
    use crate::vector::kernels::squared_norm;
    let mut graph = Graph::new(2, (4, 8), Store::Full(Vec::new()));
    let vectors = [[0.31_f32, -1.7], [2.25, 0.1], [0.0, 0.0]];
    for (n, v) in vectors.iter().enumerate() {
        graph.push((row(n), row(n), 1), 0, v);
    }
    let read = |g: &Graph, id: u32| {
        let mut buf = [0.0_f32; 2];
        squared_norm(g.vector(id, &mut buf)).to_bits()
    };
    assert_eq!(graph.norm(1).to_bits(), squared_norm(&vectors[1]).to_bits());
    let codec = Sq8.train(&vectors.concat(), 2, 0).unwrap();
    let mut each = [[0_u8; 2]; 3];
    for (v, out) in vectors.iter().zip(&mut each) {
        codec.encode(v, out);
    }
    graph.recode(Store::Coded { codec, codes: each.concat() });
    let compacted = graph.compacted();
    for id in 0..3 {
        assert_eq!(graph.norm(id).to_bits(), read(&graph, id), "{id}");
        assert_eq!(compacted.norm(id).to_bits(), read(&graph, id), "{id}");
    }
    assert_ne!(graph.norm(0).to_bits(), squared_norm(&vectors[0]).to_bits());
}
