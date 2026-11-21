//! Unit tests of a probe's two passes: the hits and their rows are those
//! a probe of each hash finds, of a table in a cache and past one, hashed
//! and direct; directory words are asked for past `PREFETCH_BYTES` alone
//! (RULES.md D602, R-TEST-1).

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::kernels::Portable;

/// Kernels that count the directory words a probe asks for, each of a
/// place in the directory.
#[derive(Default)]
struct Counting(AtomicUsize);

impl Kernels for Counting {
    fn name(&self) -> &'static str {
        "COUNTING"
    }
    fn prefetch(&self, words: &[u64], places: &[u32]) {
        assert!(places.iter().all(|&p| usize::try_from(p).unwrap() < words.len()), "a place in the directory");
        self.0.fetch_add(places.len(), Ordering::Relaxed);
    }
}

/// A hash of `n` spread over every slot.
fn spread(n: u64) -> u64 {
    crate::hash::murmur::fmix64(n.wrapping_add(1))
}

/// The rows each probe of `probes` finds of `table` through its hits,
/// against those `candidates` finds, a miss finding none.
fn found_alike(table: &JoinTable, probes: &[u64], misses: &[bool], kernels: &dyn Kernels) {
    let mut hits = Vec::new();
    table.locate((probes, misses), kernels, &mut hits);
    let mut by_hit: Vec<Vec<usize>> = vec![Vec::new(); probes.len()];
    for hit in &hits {
        by_hit[hit.place()] = table.found(*hit, probes[hit.place()]).collect();
    }
    for (n, (&h, &miss)) in probes.iter().zip(misses).enumerate() {
        let want: Vec<usize> = if miss { Vec::new() } else { table.candidates(h).collect() };
        assert_eq!(by_hit[n], want, "probe {n}");
    }
    assert!(hits.windows(2).all(|w| w[0].place() < w[1].place()), "hits in order");
}

/// Verifies: [DESIGN] RULES.md D602: a table in a cache and one past
/// `PREFETCH_BYTES` give through their hits the rows each probe's
/// candidates are, a probe of a built hash, of none, and a miss among
/// them; the words of each probe's slot after the first group are asked
/// for past the bound alone.
#[test]
fn hits_find_a_probes_candidates() {
    for rows in [1_000_u64, 300_000] {
        let built: Vec<u64> = (0..rows).map(|n| spread(n % rows.div_euclid(3))).collect();
        let table = JoinTable::of(&built, &vec![false; built.len()]);
        let probes: Vec<u64> = (0..5_000_u64).map(|n| if n % 2 == 0 { spread(n) } else { spread(n.wrapping_mul(7_919)) }).collect();
        let misses: Vec<bool> = (0..probes.len()).map(|n| n % 11 == 3).collect();
        let counting = Counting::default();
        found_alike(&table, &probes, &misses, &counting);
        found_alike(&table, &probes, &misses, &Portable);
        let asked = counting.0.load(Ordering::Relaxed);
        if table.directory.len() * 8 > PREFETCH_BYTES {
            assert_eq!(asked, probes.len() - GROUP, "{rows} rows");
        } else {
            assert_eq!(asked, 0, "{rows} rows");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D601, D602: a direct table's hits are the
/// probes whose number has rows, finding those rows.
#[test]
fn a_direct_tables_hits_are_its_keys() {
    let numbers: Vec<u64> = [3.0_f64, 5.0_f64, 3.0_f64, 4.0_f64].into_iter().map(f64::to_bits).collect();
    let table = JoinTable::direct(&numbers, &[false; 4]).unwrap();
    let probes: Vec<u64> = [3.0_f64, 6.0_f64, 4.0_f64, 2.5_f64, 5.0_f64].into_iter().map(f64::to_bits).collect();
    found_alike(&table, &probes, &[false, false, false, false, true], &Portable);
    let mut hits = Vec::new();
    table.locate((&probes, &[false; 5]), &Portable, &mut hits);
    assert_eq!(hits.iter().map(|h| h.place()).collect::<Vec<_>>(), [0, 2, 4]);
}
