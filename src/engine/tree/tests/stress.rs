//! Concurrency stress tests of the B-link tree (RULES.md D31, D500):
//! threads insert, replace, delete and read keys of their own, each
//! checked against its own model as it goes, while every thread reads and
//! scans the others' keys as they change, so leaves split, are emptied and
//! taken out under readers and writers at once. Reads start from a leaf
//! noted for the key, often a stale one, as a table's reads do. At the end
//! the tree holds exactly the union of the models, in order, both ways.
//!
//! The work is fixed by a seed per thread. How the threads interleave is
//! not, but no interleaving may change what a thread reads of its own keys,
//! nor break the order of a scan, so every run checks the same answers
//! (RULES.md R-TEST-5).

use std::collections::BTreeMap;
use std::sync::Barrier;

use super::*;

const THREADS: u64 = 4;
const STEPS: u64 = 6_000;
/// Keys per thread: few enough that inserts and deletes meet often, and
/// leaves fill, split and empty again.
const KEYS: u64 = 1_500;
/// The keys each thread deletes at the end.
const THIRD: u64 = 500;

/// A fixed sequence of numbers (RULES.md R-TEST-5).
struct Seq(u64);

impl Seq {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0.wrapping_shr(33).checked_rem(below).unwrap()
    }
}

/// Key `i` of thread `t`: the threads' keys interleave, so they share
/// leaves.
fn owned(t: u64, i: u64) -> [u8; 8] {
    key(i.checked_mul(THREADS).unwrap().checked_add(t).unwrap())
}

/// The index among its thread's keys of `k`, as its value names it.
fn owner_of(k: [u8; 8]) -> [u8; 8] {
    u64::from_be_bytes(k).checked_div(THREADS).unwrap().to_be_bytes()
}

/// A value of `len` bytes that names its key and version.
fn value(k: u64, version: u64, len: u64) -> Vec<u8> {
    let mut v = k.to_be_bytes().to_vec();
    v.extend_from_slice(&version.to_be_bytes());
    v.resize(usize::try_from(len).unwrap(), 0xA5);
    v
}

/// A run of thread `t`'s keys from its `i`th, side by side in leaves, read
/// through `kept` and checked against `model` (D806).
fn read_run(tree: &BLinkTree, (t, i): (u64, u64), model: &BTreeMap<[u8; 8], Vec<u8>>, kept: &mut Near) {
    for j in i..i.saturating_add(4).min(KEYS) {
        let k = owned(t, j);
        let found = tree.get_near(&k, kept).unwrap();
        assert_eq!(found.then(|| kept.bytes.clone()), model.get(&k).cloned(), "thread {t} key {j}, kept");
    }
}

/// One thread's work: each step a write or a read of one of its keys,
/// checked against `model`, or a read of another thread's key, or a short
/// scan checked for order.
fn work(tree: &BLinkTree, t: u64, model: &mut BTreeMap<[u8; 8], Vec<u8>>) {
    let mut seq = Seq(t.checked_add(1).unwrap());
    // Kept from read to read, as a scan keeps it, so its leaf's copy is
    // read while other threads split, change and take out leaves (D806).
    let mut kept = Near::default();
    for step in 0..STEPS {
        let i = seq.next(KEYS);
        let k = owned(t, i);
        match seq.next(10) {
            0..=3 => {
                let v = value(i, step, seq.next(120).checked_add(16).unwrap());
                let inserted = tree.insert(&k, &v).unwrap();
                assert_eq!(inserted, !model.contains_key(&k));
                if inserted {
                    model.insert(k, v);
                } else {
                    assert_eq!(tree.replace(&k, &v).unwrap().as_ref(), model.get(&k));
                    model.insert(k, v);
                }
            }
            4..=5 => assert_eq!(tree.delete(&k).unwrap(), model.remove(&k)),
            6 => read_run(tree, (t, i), model, &mut kept),
            7 => {
                // From a leaf noted for another key: often not the key's.
                let other = owned(seq.next(THREADS), seq.next(KEYS));
                let noted = leaf_of(tree, u64::from_be_bytes(other));
                hints::note(u64::from_be_bytes(k), noted.0);
                let mut near = Near::default();
                let found = tree.get_near(&k, &mut near).unwrap();
                assert_eq!(found.then_some(near.bytes), model.get(&k).cloned(), "thread {t} key {i}");
            }
            8 => {
                let other = owned(seq.next(THREADS), seq.next(KEYS));
                if let Some(v) = tree.get(&other).unwrap() {
                    assert_eq!(v.get(..8), Some(&owner_of(other)[..]), "a value names its key");
                }
            }
            _ => {
                let from = key(seq.next(KEYS * THREADS));
                let found: Vec<Vec<u8>> =
                    tree.scan(Bound::Included(&from), Bound::Unbounded, Direction::Forward).take(50).map(|e| e.unwrap().0).collect();
                assert!(found.windows(2).all(|w| w[0] < w[1]), "a scan in order, each key once");
                assert!(found.first().is_none_or(|f| f.as_slice() >= from.as_slice()));
            }
        }
    }
    // A third of its keys gone, beside the others' reads: whole leaves
    // empty and are taken out.
    for i in 0..THIRD {
        let k = owned(t, i);
        assert_eq!(tree.delete(&k).unwrap(), model.remove(&k));
        let other = owned(seq.next(THREADS), seq.next(KEYS));
        let mut near = Near::default();
        let found = tree.get_near(&other, &mut near).unwrap();
        assert!(!found || near.bytes.get(..8) == Some(&owner_of(other)[..]));
    }
}

/// Verifies: [DESIGN] RULES.md D31, D500, Lehman and Yao §5-§6: four
/// threads inserting, replacing, deleting and reading interleaved keys of
/// their own, reading the others' and scanning, read their own keys as
/// their models say at every step, from noted leaves or by descent; scans
/// stay in order; and the tree ends holding exactly the models' entries,
/// scanned either way, having split leaves and taken some out.
#[test]
fn threads_writing_and_reading_at_once_agree_with_their_models() {
    let (tree, _scratch) = tree("stress");
    tree.note_rows();
    let start = Barrier::new(usize::try_from(THREADS).unwrap());
    let models: Vec<BTreeMap<[u8; 8], Vec<u8>>> = std::thread::scope(|scope| {
        let running: Vec<_> = (0..THREADS)
            .map(|t| {
                let (tree, start) = (&tree, &start);
                scope.spawn(move || {
                    let mut model = BTreeMap::new();
                    start.wait();
                    work(tree, t, &mut model);
                    model
                })
            })
            .collect();
        running.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let mut all: Vec<(Vec<u8>, Vec<u8>)> = models.into_iter().flatten().map(|(k, v)| (k.to_vec(), v)).collect();
    all.sort();
    let forward: Vec<(Vec<u8>, Vec<u8>)> = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).map(Result::unwrap).collect();
    assert_eq!(forward, all);
    let mut backward: Vec<(Vec<u8>, Vec<u8>)> =
        tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Backward).map(Result::unwrap).collect();
    backward.reverse();
    assert_eq!(backward, all);
    for (k, v) in &all {
        assert_eq!(tree.get(k).unwrap().as_ref(), Some(v));
    }
    let counts = tree.counts();
    assert!(counts.splits > 10 && counts.unlinked > 0 && counts.hinted > 0, "{counts:?}");
}
