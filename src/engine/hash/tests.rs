//! Unit tests of the linear hash table: against a model under a long run of
//! inserts and removes, in a pool far smaller than the table; growing and
//! shrinking; the order of a key's values; reopening; writers on many
//! threads; malformed pages refused (RULES.md R-TEST-1, R-TEST-2). Split
//! from hash.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::path::Path;
use std::thread;

use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};

/// A pager file of 4 KiB pages in memory, in a pool of `frames` frames.
fn pool(frames: usize) -> (Arc<BufferPool>, FileId) {
    let pager =
        Pager::create(&MemoryFileSystem::new(), Path::new("/t"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    (pool, file)
}

fn key(n: u64) -> Vec<u8> {
    format!("key-{n}").into_bytes()
}

/// A number from `state`, a step of a xorshift generator: the run is the
/// same each time.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    *state
}

/// Verifies: [DESIGN] `HashTable`: under 20,000 random inserts and removes
/// of 500 keys, in a pool of 8 frames, so most of the table is in its file,
/// it holds what a model holds, each key's values in the order added; it
/// grows as it fills and shrinks back to one bucket as it empties.
#[test]
fn it_holds_what_a_model_holds() {
    let (pool, file) = pool(8);
    let table = HashTable::create(pool, file).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut most = 0;
    for step in 0..20_000_u64 {
        let k = key(next(&mut state) % 500);
        let values = model.entry(k.clone()).or_default();
        if next(&mut state).is_multiple_of(3) && !values.is_empty() {
            let v = values.remove(0);
            assert!(table.remove(&k, &v).unwrap());
        } else {
            let v = step.to_le_bytes().repeat(4);
            table.insert(&k, &v).unwrap();
            values.push(v);
        }
        most = most.max(table.buckets());
    }
    for (k, values) in &model {
        assert_eq!(&table.get(k).unwrap(), values);
    }
    assert_eq!(table.len(), u64::try_from(model.values().map(Vec::len).sum::<usize>()).unwrap());
    assert_eq!(u64::try_from(table.scan().unwrap().len()).unwrap(), table.len());
    assert!(most > 8, "it grew: {most} buckets");
    for (k, values) in &model {
        for v in values {
            assert!(table.remove(k, v).unwrap());
        }
    }
    assert!(!table.remove(&key(1), b"none").unwrap());
    assert_eq!((table.len(), table.buckets()), (0, 1), "it shrank back");
}

/// Verifies: [DESIGN] `HashTable::open`: a table persisted and opened again
/// holds its entries, counts and shape; one of a page that is not a meta
/// page is refused (SQLCODE -916); an entry larger than a page is refused
/// (SQLCODE -910); freeing it gives back every page.
#[test]
fn it_reopens_and_frees() {
    let (pool, file) = pool(16);
    let table = HashTable::create(Arc::clone(&pool), file).unwrap();
    for n in 0..2_000 {
        table.insert(&key(n), &n.to_le_bytes()).unwrap();
    }
    table.persist().unwrap();
    let again = HashTable::open(Arc::clone(&pool), file, table.meta()).unwrap();
    assert_eq!((again.len(), again.buckets()), (table.len(), table.buckets()));
    assert_eq!(again.get(&key(1_234)).unwrap(), vec![1_234_u64.to_le_bytes().to_vec()]);
    let head = again.head(0).unwrap();
    assert_eq!(HashTable::open(Arc::clone(&pool), file, head).err().map(|e| e.code), Some(Code::Corrupt));
    let too_big = vec![0_u8; again.max_entry().saturating_add(1)];
    assert_eq!(again.insert(&too_big, b"").err().map(|e| e.code), Some(Code::Api));
    again.insert(&too_big[1..], b"").unwrap();
    again.free().unwrap();
}

/// Verifies: [DESIGN] `HashTable`, concurrency: writers on eight threads,
/// each its own keys, while the table splits under them, leave each key
/// with its values in order.
#[test]
fn writers_on_many_threads() {
    let (pool, file) = pool(64);
    let table = Arc::new(HashTable::create(pool, file).unwrap());
    let writers: Vec<_> = (0..8_u64)
        .map(|t| {
            let table = Arc::clone(&table);
            thread::spawn(move || {
                for n in 0..1_000_u64 {
                    table.insert(&key(t * 10_000 + n % 100), &n.to_le_bytes()).unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    assert_eq!(table.len(), 8_000);
    for t in 0..8_u64 {
        let found = table.get(&key(t * 10_000 + 7)).unwrap();
        let wanted: Vec<Vec<u8>> = (0..1_000_u64).filter(|n| n % 100 == 7).map(|n| n.to_le_bytes().to_vec()).collect();
        assert_eq!(found, wanted);
    }
}

/// Verifies: [DESIGN] `HashTable`, readers lock nothing: readers on four
/// threads, while writers on four more grow the table through hundreds of
/// splits and then shrink it, always find the keys inserted before they
/// began, each with its one value.
#[test]
fn readers_see_every_key_through_splits_and_merges() {
    let (pool, file) = pool(64);
    let table = Arc::new(HashTable::create(pool, file).unwrap());
    for n in 0..500_u64 {
        table.insert(&key(n), &n.to_le_bytes()).unwrap();
    }
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..4_u64)
        .map(|t| {
            let (table, done) = (Arc::clone(&table), Arc::clone(&done));
            thread::spawn(move || {
                let mut reads = 0_u64;
                while !done.load(Ordering::Acquire) {
                    let n = (reads * 7 + t) % 500;
                    assert_eq!(table.get(&key(n)).unwrap(), vec![n.to_le_bytes().to_vec()], "key {n}");
                    reads += 1;
                }
                reads
            })
        })
        .collect();
    let writers: Vec<_> = (0..4_u64)
        .map(|t| {
            let table = Arc::clone(&table);
            thread::spawn(move || {
                let mine: Vec<u64> = (0..3_000_u64).map(|n| 1_000_000 + t * 10_000 + n).collect();
                for n in &mine {
                    table.insert(&key(*n), b"w").unwrap();
                }
                for n in &mine {
                    assert!(table.remove(&key(*n), b"w").unwrap());
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    done.store(true, Ordering::Release);
    for r in readers {
        assert!(r.join().unwrap() > 0);
    }
    assert_eq!(table.len(), 500);
}

/// Verifies: [DESIGN] `hash/bucket.rs`: a page cut short, or whose entry
/// runs past its end, is refused (SQLCODE -916).
#[test]
fn malformed_pages_are_refused() {
    assert_eq!(bucket::next(&[1_u8]).err().map(|e| e.code), Some(Code::Corrupt));
    let entry = (b"k".to_vec(), b"v".to_vec());
    let mut page = bucket::build(&[(fragment(hash(&entry.0)), &entry)], None).unwrap();
    page.truncate(page.len() - 1);
    assert_eq!(bucket::entries(&page).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] CONFORMANCE.md, Limits: the longest entry a hash
/// table holds is 8,136 bytes at 8 KiB pages and 65,480 at 64 KiB, with
/// CRC-32C page checksums.
#[test]
fn the_longest_entry_is_as_documented() {
    for (page_size, most) in [(8_192_u32, 8_136_usize), (65_536, 65_480)] {
        let pager =
            Pager::create(&MemoryFileSystem::new(), Path::new("/t"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), page_size)
                .unwrap();
        let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
        let file = pool.add_pager(Arc::new(pager));
        assert_eq!(HashTable::create(pool, file).unwrap().max_entry(), most);
    }
}

/// Verifies: [DESIGN] `HashTable::counted`: a table not persisted since it
/// last changed, as a checkpoint leaves it, opened with the counts taken
/// with its pages, counts the entries it holds, and removing them all
/// leaves it empty, shrunk back to one bucket.
#[test]
fn it_reopens_counting_what_it_holds() {
    let (pool, file) = pool(16);
    let table = HashTable::create(Arc::clone(&pool), file).unwrap();
    for n in 0..2_000_u64 {
        table.insert(&key(n), &n.to_le_bytes()).unwrap();
    }
    let again = HashTable::open(Arc::clone(&pool), file, table.meta()).unwrap().counted(table.counts());
    assert_eq!(again.counts(), table.counts());
    assert_eq!(again.len(), 2_000);
    for n in 0..2_000_u64 {
        assert!(again.remove(&key(n), &n.to_le_bytes()).unwrap());
    }
    assert_eq!((again.len(), again.buckets()), (0, 1));
}
