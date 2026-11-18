//! IVF requirements on seeded clustered data: exact search untrained,
//! recall@10 against brute force once trained, probes, persistence
//! across reopening, deletes never returned, filtered and range
//! searches, quantized codes with re-ranking, and retraining.

use std::sync::Arc;

use super::*;
use crate::engine::hnsw::tests::{clustered, euclidean, float, mean_recall, pages, row};
use crate::engine::hnsw::{Search, Sq8};
use crate::error::Code;

/// The configuration of the tests' indexes: lists sqrt(N), probes one
/// list in eight.
fn config(dims: usize) -> Config {
    Config { dims, lists: 0, probes: 0, sample_per_list: 256, seed: 42, rerank: true }
}

/// An index of `flat` inserted one by one, rows numbered from 0.
fn built(flat: &[f32], dims: usize, quantizer: Option<Arc<dyn Quantizer>>) -> (Ivf, Arc<MemoryBudget>) {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let ivf = Ivf::create(pool, file, &config(dims), (euclidean(), quantizer), &budget).unwrap();
    for (i, v) in flat.chunks_exact(dims).enumerate() {
        ivf.insert(v, row(i)).unwrap();
    }
    (ivf, budget)
}

/// The index of `ivf`'s pages opened again, as a database would.
fn reopened(ivf: &Ivf, budget: &Arc<MemoryBudget>, quantizer: Option<Arc<dyn Quantizer>>) -> Result<Ivf> {
    Ivf::open(Arc::clone(&ivf.inner.pool), (ivf.inner.file, ivf.meta()), &config(ivf.dims()), (euclidean(), quantizer), budget)
}

/// The ten nearest by `probes` lists, or the index's.
fn probing(ivf: &Ivf, probes: Option<usize>) -> impl Fn(&[f32]) -> Vec<(f32, u64)> + '_ {
    move |q| ivf.nearest(q, 10, Search { probes, ..Search::default() }).unwrap()
}

/// Verifies: [EXTENSION] an index too small to train is one list,
/// searched exactly (vector design document R10.5): recall@10 is 1.
#[test]
fn ivf_untrained_is_exact() {
    let dims = 8;
    let flat = clustered(400, dims, 4, 1);
    let (ivf, _budget) = built(&flat, dims, None);
    assert!(!ivf.maintenance_due());
    assert_eq!(ivf.shape().unwrap(), (1, false));
    assert!((mean_recall(&probing(&ivf, None), &flat, dims, &|_| true) - 1.0_f64).abs() < 1e-9_f64);
}

/// Verifies: [EXTENSION] NEIGHBOR PARTITIONS (§10.4): once it holds enough
/// rows, maintenance trains sqrt(N) lists by seeded k-means; probing one
/// list in eight finds at least 90% of the 10 true nearest; a query's own
/// NEIGHBOR PARTITION PROBES (§9.2) trades recall, every list exact.
#[test]
fn ivf_recall_at_10_once_trained() {
    let dims = 32;
    let flat = clustered(3_000, dims, 20, 3);
    let (ivf, _budget) = built(&flat, dims, None);
    assert!(ivf.maintenance_due());
    assert!(ivf.maintain().unwrap());
    assert_eq!(ivf.shape().unwrap(), (54, false));
    assert!(!ivf.maintenance_due());
    let r = mean_recall(&probing(&ivf, None), &flat, dims, &|_| true);
    assert!(r >= 0.9_f64, "recall@10 {r}");
    let one = mean_recall(&probing(&ivf, Some(1)), &flat, dims, &|_| true);
    let all = mean_recall(&probing(&ivf, Some(54)), &flat, dims, &|_| true);
    assert!(one <= r && (all - 1.0_f64).abs() < 1e-9_f64, "one list {one}, all {all}");
    let found = ivf.nearest(&flat[..dims], 10, Search::default()).unwrap();
    assert_eq!(found[0].1, 0);
    assert!(found.windows(2).all(|w| w[0].0 <= w[1].0));
}

/// Verifies: [EXTENSION] a filtered search gives only rows its filter
/// takes, and widens to more lists in step with the filter's selectivity
/// (R9.3): ten rows at a filter keeping one in fifty, recall at least 90%.
#[test]
fn ivf_filtered_search_widens() {
    let dims = 16;
    let flat = clustered(3_000, dims, 20, 5);
    let (ivf, _budget) = built(&flat, dims, None);
    ivf.maintain().unwrap();
    for share in [2_u64, 50] {
        let keep = move |r: u64| r.is_multiple_of(share);
        let search = |q: &[f32]| {
            let mut filter = keep;
            let found = ivf.nearest(q, 10, Search { filter: Some(&mut filter), ..Search::default() }).unwrap();
            assert!(found.len() == 10 && found.iter().all(|(_, r)| keep(*r)));
            found
        };
        let r = mean_recall(&search, &flat, dims, &keep);
        assert!(r >= 0.9_f64, "filtered recall {r} keeping one in {share}");
    }
}

/// Verifies: [EXTENSION] a range search (R9.1) gives only rows within its
/// radius, nearest first, and at least 90% of those brute force finds.
#[test]
fn ivf_range_search() {
    let dims = 16;
    let flat = clustered(3_000, dims, 10, 9);
    let (ivf, _budget) = built(&flat, dims, None);
    ivf.maintain().unwrap();
    let metric = euclidean();
    let (mut found_all, mut truth_all) = (0_usize, 0_usize);
    for q in clustered(20, dims, 10, 11).chunks_exact(dims) {
        let radius = 0.3;
        let found = ivf.range(q, radius, Search::default()).unwrap();
        assert!(found.iter().all(|(d, _)| *d <= radius) && found.windows(2).all(|w| w[0].0 <= w[1].0));
        found_all = found_all.wrapping_add(found.len());
        truth_all = truth_all.wrapping_add(flat.chunks_exact(dims).filter(|v| metric.distance(q, v) <= radius).count());
    }
    assert!(truth_all > 0);
    assert!(float(found_all) >= 0.9_f64 * float(truth_all), "range found {found_all} of {truth_all}");
}

/// Verifies: [EXTENSION] every change is written through to pages: an
/// index opened again, trained or not, with no save, answers as before;
/// the directory a save writes reads back.
#[test]
fn ivf_survives_reopen() {
    let dims = 16;
    let flat = clustered(2_500, dims, 10, 13);
    let (ivf, budget) = built(&flat[..dims * 500], dims, None);
    let answers =
        |index: &Ivf| -> Vec<Vec<(f32, u64)>> { clustered(10, dims, 10, 7).chunks_exact(dims).map(|q| probing(index, None)(q)).collect() };
    assert_eq!(answers(&reopened(&ivf, &budget, None).unwrap()), answers(&ivf));
    for (i, v) in flat.chunks_exact(dims).enumerate().skip(500) {
        ivf.insert(v, row(i)).unwrap();
    }
    ivf.maintain().unwrap();
    ivf.remove(&flat[..dims], 0).unwrap();
    ivf.insert(&flat[dims..dims * 2], 1).unwrap();
    let again = reopened(&ivf, &budget, None).unwrap();
    assert_eq!(answers(&again), answers(&ivf));
    assert_eq!(again.counts().unwrap(), ivf.counts().unwrap());
    assert_eq!(again.shape().unwrap(), ivf.shape().unwrap());
    again.save().unwrap();
    let third = reopened(&again, &budget, None).unwrap();
    assert_eq!(third.rows(&flat[dims..dims * 2]).unwrap(), vec![1]);
    assert_eq!(third.rows(&flat[..dims]).unwrap(), Vec::<u64>::new());
}

/// Verifies: [EXTENSION] entries are counted, removed rows are never
/// returned, and past 20% tombstones (R10.10) maintenance writes the
/// lists anew without them, recall kept at least 90%.
#[test]
fn ivf_deletes_are_never_returned() {
    let dims = 16;
    let flat = clustered(3_000, dims, 20, 17);
    let (ivf, _budget) = built(&flat, dims, None);
    ivf.maintain().unwrap();
    ivf.insert(&flat[..dims], 0).unwrap();
    ivf.remove(&flat[..dims], 0).unwrap();
    assert_eq!(ivf.rows(&flat[..dims]).unwrap(), vec![0], "counted twice, removed once");
    let gone = |r: u64| r % 10 < 3;
    for (i, v) in flat.chunks_exact(dims).enumerate().filter(|(i, _)| gone(row(*i))) {
        ivf.remove(v, row(i)).unwrap();
    }
    assert_eq!(ivf.counts().unwrap(), (2_100, 900));
    let search = |q: &[f32]| {
        let found = probing(&ivf, None)(q);
        assert!(found.iter().all(|(_, r)| !gone(*r)), "a removed row was returned");
        found
    };
    assert!(mean_recall(&search, &flat, dims, &|r| !gone(r)) >= 0.9_f64);
    assert!(ivf.maintenance_due());
    assert!(ivf.maintain().unwrap());
    assert_eq!(ivf.counts().unwrap(), (2_100, 0));
    let after = mean_recall(&search, &flat, dims, &|r| !gone(r));
    assert!(after >= 0.9_f64, "recall {after} after maintenance");
}

/// Verifies: [EXTENSION] a quantizer is trained with the first centroids
/// (R7.2) and the lists then hold SQ8 codes; RERANK (R7.1) re-scoring its
/// candidates exactly keeps recall@10 at least 90%; rows and removes
/// still find their entries; the codec reads back at reopening.
#[test]
fn ivf_quantized_with_rerank() {
    let dims = 32;
    let flat = clustered(3_000, dims, 20, 19);
    let quantizer: Arc<dyn Quantizer> = Arc::new(Sq8);
    let (ivf, budget) = built(&flat, dims, Some(Arc::clone(&quantizer)));
    ivf.maintain().unwrap();
    assert_eq!(ivf.shape().unwrap(), (54, true));
    let metric = euclidean();
    let search = |q: &[f32]| {
        let mut exact = |r: u64| {
            let at = usize::try_from(r).unwrap().wrapping_mul(dims);
            Some(metric.distance(q, &flat[at..at.wrapping_add(dims)]))
        };
        ivf.nearest(q, 10, Search { exact: Some(&mut exact), ..Search::default() }).unwrap()
    };
    let r = mean_recall(&search, &flat, dims, &|_| true);
    assert!(r >= 0.9_f64, "quantized recall@10 {r}");
    assert_eq!(ivf.nearest(&flat[..dims], 10, Search::default()).unwrap().len(), 40);
    assert_eq!(ivf.rows(&flat[dims * 7..dims * 8]).unwrap(), vec![7]);
    ivf.remove(&flat[dims * 7..dims * 8], 7).unwrap();
    assert_eq!(ivf.rows(&flat[dims * 7..dims * 8]).unwrap(), Vec::<u64>::new());
    let again = reopened(&ivf, &budget, Some(quantizer)).unwrap();
    assert_eq!(again.shape().unwrap(), (54, true));
    assert_eq!(again.counts().unwrap(), ivf.counts().unwrap());
}

/// Verifies: [EXTENSION] a bulk load of enough entries trains at once,
/// counting a pair given twice.
#[test]
fn ivf_load_trains_at_once() {
    let dims = 16;
    let flat = clustered(2_000, dims, 10, 23);
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let ivf = Ivf::create(pool, file, &config(dims), (euclidean(), None), &budget).unwrap();
    let twice = std::iter::once(Ok((flat[..dims].to_vec(), 0)));
    let mut entries = flat.chunks_exact(dims).enumerate().map(|(i, v)| Ok((v.to_vec(), row(i)))).chain(twice);
    ivf.load(&mut entries).unwrap();
    assert_eq!(ivf.shape().unwrap(), (44, false));
    assert_eq!(ivf.counts().unwrap(), (2_000, 0));
    ivf.remove(&flat[..dims], 0).unwrap();
    assert_eq!(ivf.rows(&flat[..dims]).unwrap(), vec![0]);
    assert!(mean_recall(&probing(&ivf, None), &flat, dims, &|_| true) >= 0.9_f64);
}

/// Verifies: [EXTENSION] an index trains as many lists as its rows can
/// train, and trains again as it grows fourfold, its automatic count of
/// lists following sqrt(N).
#[test]
fn ivf_retrains_as_it_grows() {
    let dims = 8;
    let flat = clustered(10_000, dims, 10, 29);
    let (ivf, _budget) = built(&flat[..dims * 700], dims, None);
    ivf.maintain().unwrap();
    assert_eq!(ivf.shape().unwrap().0, 17);
    for (i, v) in flat.chunks_exact(dims).enumerate().skip(700) {
        ivf.insert(v, row(i)).unwrap();
    }
    assert!(ivf.maintenance_due());
    ivf.maintain().unwrap();
    assert_eq!(ivf.shape().unwrap().0, 100);
}

/// Verifies: [DESIGN] seeded k-means finds clustered data's clusters: the
/// points of each cluster mostly share one centroid, and one seed gives
/// the same centroids every time.
#[test]
fn kmeans_finds_clusters() {
    let dims = 8;
    let clusters = 5;
    let flat = clustered(1_000, dims, clusters, 31);
    let metric = euclidean();
    let centres = kmeans::train(&metric, &flat, (dims, clusters), 7);
    assert_eq!(centres, kmeans::train(&metric, &flat, (dims, clusters), 7));
    let mut votes = vec![vec![0_usize; clusters]; clusters];
    for (i, v) in flat.chunks_exact(dims).enumerate() {
        votes[i.wrapping_mul(7919) % clusters][kmeans::nearest(&metric, &centres, dims, v).0] += 1;
    }
    let agree: usize = votes.iter().map(|v| v.iter().copied().max().unwrap()).sum();
    assert!(agree >= 950, "{agree} of 1000 share their cluster's centroid");
}

/// Verifies: [DESIGN] an entry too large for a page is refused at
/// creation with SQLCODE -910 (R10.7).
#[test]
fn ivf_refuses_entries_past_a_page() {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let error = Ivf::create(pool, file, &config(4_096), (euclidean(), None), &budget).err().unwrap();
    assert_eq!(error.code, Code::Api);
}

/// Verifies: [DESIGN] RULES.md R-ERR-4: a search's filter runs with no
/// lock of the index held, so a filter that reads the index again
/// (`rows`) and writes it (`insert`) completes. A regression test for the
/// R-ERR-4 deviation: the search held the state's read lock across the
/// filter, and an insert from it waited for that lock forever.
#[test]
fn ivf_filter_may_use_the_index() {
    let dims = 8;
    let flat = clustered(1_000, dims, 5, 41);
    let (ivf, _budget) = built(&flat, dims, None);
    assert!(ivf.maintain().unwrap());
    let other = ivf.clone();
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
    let found = ivf.nearest(&flat[..dims], 10, Search { filter: Some(&mut filter), ..Search::default() }).unwrap();
    assert!(!found.is_empty() && found.iter().all(|(_, r)| r.is_multiple_of(2)));
    let mut again = |r: u64| other.rows(&flat[..dims]).unwrap().contains(&r) || r.is_multiple_of(3);
    let within = ivf.range(&flat[..dims], 0.5, Search { filter: Some(&mut again), ..Search::default() }).unwrap();
    assert!(within.iter().all(|(d, _)| *d <= 0.5));
    assert!(ivf.rows(&flat[dims..dims * 2]).unwrap().len() > 1, "the filter's inserts were made");
}

/// Verifies: [DESIGN] `entries` gives the row of every live entry once,
/// tombstones left out, and keeps reading the lists it began in when
/// training replaces them meanwhile.
#[test]
fn ivf_entries_gives_each_live_entry() {
    let dims = 4;
    let flat = clustered(1_000, dims, 5, 43);
    let (ivf, _budget) = built(&flat, dims, None);
    ivf.remove(&flat[..dims], 0).unwrap();
    let mut reading = ivf.entries();
    let first = reading.next().unwrap().unwrap();
    assert!(ivf.maintain().unwrap(), "trained as the entries are read");
    let mut rows: Vec<u64> = std::iter::once(first).chain(reading.map(Result::unwrap)).collect();
    rows.sort_unstable();
    assert_eq!(rows, (1..1_000).map(row).collect::<Vec<_>>());
    let mut after: Vec<u64> = ivf.entries().map(Result::unwrap).collect();
    after.sort_unstable();
    assert_eq!(after, rows);
}
