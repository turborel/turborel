//! The graph kept in pages: its image, its change log replayed, a log
//! record cut short, a bulk load, and pages freed.

use super::*;

/// The results of seeded queries, for comparing two indexes.
fn answers(hnsw: &Hnsw, dims: usize) -> Vec<Vec<(f32, u64)>> {
    clustered(20, dims, 20, 7).chunks_exact(dims).map(|q| hnsw.nearest(q, 10, Search::default()).unwrap()).collect()
}

/// The index of `hnsw`'s pages opened again, as a database would.
fn reopened(hnsw: &Hnsw, budget: &Arc<MemoryBudget>, quantizer: Option<Arc<dyn Quantizer>>) -> Result<Hnsw> {
    let (pool, file) = log::pages(hnsw);
    Hnsw::open(pool, (file, hnsw.meta()), &config(hnsw.dims()), (euclidean(), quantizer), budget)
}

/// Verifies: [EXTENSION] the INMEMORY NEIGHBOR GRAPH survives reopening
/// (R14.6): opened from its pages after a save, the index answers every
/// query as before, its counts the same.
#[test]
fn hnsw_image_survives_reopen() {
    let dims = 16;
    let flat = clustered(1_500, dims, 10, 21);
    let (hnsw, budget) = built(&flat, dims, None);
    hnsw.remove(&flat[..dims], 0).unwrap();
    hnsw.save().unwrap();
    let again = reopened(&hnsw, &budget, None).unwrap();
    assert_eq!(answers(&again, dims), answers(&hnsw, dims));
    assert_eq!(again.counts().unwrap(), hnsw.counts().unwrap());
}

/// Verifies: [EXTENSION] changes after the last image are replayed from
/// the change log at open, so a checkpoint the method did not take loses
/// none: inserts and removes since the save are there after reopening.
#[test]
fn hnsw_log_replays_changes_since_the_image() {
    let dims = 8;
    let flat = clustered(600, dims, 5, 23);
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(dims), (euclidean(), None), &budget).unwrap();
    for (i, v) in flat.chunks_exact(dims).enumerate().take(300) {
        hnsw.insert(v, row(i)).unwrap();
    }
    hnsw.save().unwrap();
    for (i, v) in flat.chunks_exact(dims).enumerate().skip(300) {
        hnsw.insert(v, row(i)).unwrap();
    }
    hnsw.remove(&flat[..dims], 0).unwrap();
    hnsw.remove(&flat[dims * 400..dims * 401], 400).unwrap();
    let again = reopened(&hnsw, &budget, None).unwrap();
    assert_eq!(again.counts().unwrap(), (598, 2));
    assert_eq!(answers(&again, dims), answers(&hnsw, dims));
    assert_eq!(again.rows(&flat[dims * 400..dims * 401]).unwrap(), Vec::<u64>::new());
    assert_eq!(again.rows(&flat[dims * 599..]).unwrap(), vec![599]);
    again.insert(&flat[..dims], 0).unwrap();
    let third = reopened(&again, &budget, None).unwrap();
    assert_eq!(third.rows(&flat[..dims]).unwrap(), vec![0], "appends go on after a replayed log");
}

/// Verifies: [DESIGN] a log record cut short at the log's end, as a
/// checkpoint taken during an append holds, is dropped at open and cut
/// off, and the log goes on after the last whole record.
#[test]
fn hnsw_log_record_cut_short_is_dropped() {
    let dims = 4;
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(Arc::clone(&pool), file, &config(dims), (euclidean(), None), &budget).unwrap();
    hnsw.insert(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
    hnsw.insert(&[0.0, 1.0, 0.0, 0.0], 2).unwrap();
    let last = lock(&hnsw.inner.log).unwrap().0.last;
    {
        let mut page = pool.write(file, last).unwrap();
        let cut = page.len() - 5;
        page.truncate(cut);
    }
    let again = reopened(&hnsw, &budget, None).unwrap();
    assert_eq!(again.counts().unwrap(), (1, 0));
    assert_eq!(again.rows(&[0.0, 1.0, 0.0, 0.0]).unwrap(), Vec::<u64>::new());
    again.insert(&[0.0, 0.0, 1.0, 0.0], 3).unwrap();
    let third = reopened(&again, &budget, None).unwrap();
    assert_eq!(third.counts().unwrap(), (2, 0));
    assert_eq!(third.rows(&[0.0, 0.0, 1.0, 0.0]).unwrap(), vec![3]);
}

/// Verifies: [EXTENSION] a bulk load trains the quantizer when it is
/// given enough rows, links every entry, and writes the image, so the
/// index opens again quantized with no log to replay.
#[test]
fn hnsw_load_builds_and_saves() {
    let dims = 16;
    let flat = clustered(1_200, dims, 10, 29);
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let quantizer: Arc<dyn Quantizer> = Arc::new(Sq8);
    let hnsw = Hnsw::create(pool, file, &config(dims), (euclidean(), Some(Arc::clone(&quantizer))), &budget).unwrap();
    let mut entries = flat.chunks_exact(dims).enumerate().map(|(i, v)| Ok((v.to_vec(), row(i))));
    hnsw.load(&mut entries).unwrap();
    assert!(hnsw.quantized().unwrap());
    assert_eq!(lock(&hnsw.inner.log).unwrap().1, 0);
    let again = reopened(&hnsw, &budget, Some(quantizer)).unwrap();
    assert!(again.quantized().unwrap());
    assert_eq!(answers(&again, dims), answers(&hnsw, dims));
    assert_eq!(reopened(&hnsw, &budget, None).err().map(|e| e.code), Some(Code::Corrupt), "opened with another quantizer");
}

/// Verifies: [DESIGN] the image is compact: neighbour lists as varint
/// gaps take fewer bytes than fixed four-byte ids would.
#[test]
fn hnsw_image_lists_are_compressed() {
    let dims = 4;
    let flat = clustered(2_000, dims, 10, 31);
    let (hnsw, _budget) = built(&flat, dims, None);
    hnsw.save().unwrap();
    let (pool, file) = log::pages(&hnsw);
    let head = *lock(&hnsw.inner.image).unwrap();
    let image_pages = ChainReader::new(&pool, file, head).unwrap().pages().unwrap();
    let image_bytes: usize = image_pages.iter().map(|p| pool.read(file, p.0).unwrap().len()).sum();
    let graph = read(&hnsw.inner.graph).unwrap();
    let links: usize = graph
        .levels
        .iter()
        .enumerate()
        .map(|(id, top)| (0..=*top).map(|l| graph.links(u32::try_from(id).unwrap(), l).len()).sum::<usize>())
        .sum();
    let fixed = 2_000 * (8 + 4 + 1 + 8 + dims * 4) + links * 4;
    assert!(image_bytes.saturating_mul(4) < fixed.saturating_mul(3), "image {image_bytes} bytes against {fixed} fixed");
}

/// Verifies: [DESIGN] freeing a dropped index frees its image, log and
/// meta pages, so the meta page can no longer be opened.
#[test]
fn hnsw_free_releases_pages() {
    let dims = 4;
    let (hnsw, budget) = built(&clustered(300, dims, 3, 37), dims, None);
    hnsw.save().unwrap();
    hnsw.insert(&[9.0, 9.0, 9.0, 9.0], 9_000).unwrap();
    hnsw.free().unwrap();
    assert!(reopened(&hnsw, &budget, None).is_err());
}

use crate::sync::{lock, read};
