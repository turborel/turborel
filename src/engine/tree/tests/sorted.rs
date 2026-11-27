//! Unit tests of keys written in key order, a leaf's run at a time
//! (RULES.md D1235, `sorted.rs`): each written as `update_alone_into`
//! writes one, alone as it says, while readers scan and splits run.

use super::*;
use std::sync::atomic::AtomicBool;

/// The key of `n` and its prefix, the first 4 bytes: of `n / 2`, so two
/// keys share each.
fn halved(n: u64) -> Vec<u8> {
    let mut key = u32::try_from(n.div_euclid(2)).unwrap().to_be_bytes().to_vec();
    key.extend_from_slice(&n.to_be_bytes());
    key
}

/// Verifies: [DESIGN] RULES.md D1235: keys written in order, some beside
/// a key of their prefix already there, some sharing one with another
/// written, are each put once, with the value its change wrote, alone
/// exactly where no other key of its prefix is beside it as it goes in,
/// the leaves split as they fill, every key found after.
#[test]
fn keys_in_order_are_each_written_alone_as_one_would_be() {
    let (tree, _scratch) = tree("sorted-alone");
    // Odd keys there first: a key 2k shares its prefix with 2k + 1.
    for n in (1..4000_u64).step_by(4) {
        assert!(tree.insert(&halved(n), &VALUE).unwrap());
    }
    let written: Vec<Vec<u8>> = (0..4000_u64).filter(|n| n % 4 != 1).map(halved).collect();
    let pairs: Vec<(&[u8], usize)> = written.iter().map(|k| (k.as_slice(), 4)).collect();
    let mut seen = Vec::new();
    let alone = tree
        .update_sorted(&pairs, &mut |n, old, into| {
            assert!(old.is_none());
            seen.push(n);
            into.extend_from_slice(&VALUE);
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, (0..written.len()).collect::<Vec<_>>());
    for (key, lone) in written.iter().zip(alone) {
        let n = u64::from_be_bytes(key[4..].try_into().unwrap());
        // 4k + 0 is beside 4k + 1, there before; 4k + 3 beside 4k + 2, written just before it.
        assert_eq!(lone, n % 4 == 2, "key {n}");
    }
    let all = keys(&tree, Direction::Forward);
    assert_eq!(all, (0..4000_u64).map(halved).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] RULES.md D1235, B-link reads (Lehman and Yao §5):
/// readers scanning while a writer puts runs of keys in order between
/// keys already there, splitting leaf after leaf, each find every key
/// there before, once, in order, whatever they find of those written.
#[test]
fn readers_find_every_key_while_runs_split_leaves() {
    let (tree, _scratch) = tree("sorted-readers");
    for n in (0..20_000_u64).step_by(2) {
        assert!(tree.insert(&key(n), &VALUE).unwrap());
    }
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..3_u8 {
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    let found = keys(&tree, Direction::Forward);
                    assert!(found.windows(2).all(|w| w[0] < w[1]), "in order, each once");
                    let before = found.iter().filter(|k| u64::from_be_bytes(k.as_slice().try_into().unwrap()) % 2 == 0).count();
                    assert_eq!(before, 10_000, "every key there before");
                }
            });
        }
        for run in 0..10_u64 {
            // In order already: n * 10 + run grows with n.
            let sorted: Vec<[u8; 8]> = (0..1000).map(|n| key((n * 10 + run) * 2 + 1)).collect();
            let pairs: Vec<(&[u8], usize)> = sorted.iter().map(|k| (k.as_slice(), 8)).collect();
            tree.update_sorted(&pairs, &mut |_, _, into| {
                into.extend_from_slice(&VALUE);
                Ok(())
            })
            .unwrap();
        }
        done.store(true, Ordering::Release);
    });
    assert_eq!(keys(&tree, Direction::Forward).len(), 20_000);
}
