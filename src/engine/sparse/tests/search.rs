//! Block-Max WAND against brute force: every quantization, rescored and
//! not, positive and signed weights, filters, range searches, and the
//! completeness a search reports.

use super::*;
use crate::error::Code;

/// The rows and scores of a search.
fn scored(found: &Found) -> Vec<(u64, f64)> {
    found.hits.iter().map(|h| (h.row, h.score)).collect()
}

/// The exact score of `row`'s vector of `corpus` with `query`.
fn exact(corpus: &[(u64, SparseVector)], query: &SparseVector, row: u64) -> Option<f64> {
    corpus.iter().find(|(r, _)| *r == row).map(|(_, v)| v.dot(query))
}

/// Verifies: [EXTENSION] (§10.4 "exact on the sparse dot product") For
/// every quantization, positive and signed weights and k from 1 to past
/// the rows, the top k by Block-Max WAND with a rescoring are exactly a
/// brute force's, scores and ties to the lower row; and complete exactly
/// when the k-th score is above 0.
#[test]
fn rescored_top_k_equals_brute_force() {
    for signed in [false, true] {
        let data = corpus(if signed { 31 } else { 37 }, 2_500, signed);
        for layout in layouts() {
            let files = Files::new();
            let (index, ..) = files.create(layout);
            insert_all(&index, &data);
            let mut random = Random(41);
            for k in [1, 7, 50, 3_000] {
                let query = vector(&mut random, signed);
                let mut rescore = |row: u64| Ok(exact(&data, &query, row));
                let found = index.top_k(&query, k, Ask { filter: None, rescore: Some(&mut rescore) }, &files.budget).unwrap();
                let expected = brute(&data, &query, k, &|_| true);
                if found.complete {
                    assert_eq!(scored(&found), expected, "{layout:?} k={k} signed={signed}");
                } else {
                    let positive: Vec<_> = expected.iter().copied().filter(|(_, s)| *s > 0.0_f64).collect();
                    assert_eq!(scored(&found).into_iter().filter(|(_, s)| *s > 0.0_f64).collect::<Vec<_>>(), positive);
                    assert!(expected.len() < k || expected.last().is_some_and(|(_, s)| *s <= 0.0_f64));
                }
            }
        }
    }
}

/// Verifies: [DESIGN] With FLOAT32 weights and no rescoring, scores are
/// the weights' own, so the top k equal brute force's rows and scores to
/// within the order of summation; with UINT8 and FLOAT16 weights, every
/// score is within the blocks' errors of the true one.
#[test]
fn unrescored_scores_are_exact_or_within_their_error() {
    let data = corpus(43, 2_000, false);
    for layout in layouts() {
        let files = Files::new();
        let (index, ..) = files.create(layout);
        insert_all(&index, &data);
        let mut random = Random(47);
        for _ in 0_u32..10 {
            let query = vector(&mut random, false);
            let found = index.top_k(&query, 20, Ask::default(), &files.budget).unwrap();
            let tolerance = match layout.quantization {
                Quantization::Float32 => 1.0e-9_f64,
                Quantization::Float16 => 2.0e-2_f64,
                Quantization::Uint8 => 2.0e-1_f64,
            };
            for hit in &found.hits {
                let truth = exact(&data, &query, hit.row).unwrap();
                assert!((hit.score - truth).abs() <= tolerance * truth.abs().max(1.0), "{layout:?} {hit:?} {truth}");
                assert_eq!(hit.exact, layout.quantization == Quantization::Float32);
            }
            if layout.quantization == Quantization::Float32 {
                let rows: Vec<_> = found.hits.iter().map(|h| h.row).collect();
                assert_eq!(rows, brute(&data, &query, 20, &|_| true).iter().map(|(r, _)| *r).collect::<Vec<_>>());
            }
        }
    }
}

/// Verifies: [EXTENSION] (§9.3 R9.3, §13 R13.2 in-filter) A filtered top
/// k equals brute force over the rows the filter keeps, at selectivities
/// of 1 in 2, 1 in 10 and 1 in 100, and a rescoring that refuses a row
/// (one not visible) leaves it out.
#[test]
fn filtered_top_k_equals_brute_force_over_rows_kept() {
    let data = corpus(53, 3_000, false);
    let files = Files::new();
    let (index, ..) = files.create(Layout::default());
    insert_all(&index, &data);
    let mut random = Random(59);
    for every in [2_u64, 10, 100] {
        let keep = move |row: u64| row.checked_div(3).unwrap().rem_euclid(every) == 1;
        let query = vector(&mut random, false);
        let mut rescore = |row: u64| Ok(exact(&data, &query, row).filter(|_| row != every.wrapping_add(1).wrapping_mul(3)));
        let mut filter = |row: u64| Ok(keep(row));
        let ask = Ask { filter: Some(&mut filter), rescore: Some(&mut rescore) };
        let found = index.top_k(&query, 10, ask, &files.budget).unwrap();
        let mut expected = brute(&data, &query, 10, &|row| keep(row) && row != every.wrapping_add(1).wrapping_mul(3));
        if !found.complete {
            expected.retain(|(_, s)| *s > 0.0_f64);
        }
        assert_eq!(scored(&found), expected, "1 in {every}");
    }
}

/// Verifies: [EXTENSION] (§9.4) A range search gives every row of a dot
/// product of at least its least score, best first, equal to brute force,
/// for every quantization under a rescoring; complete only for a least
/// score above 0.
#[test]
fn range_search_equals_brute_force() {
    let data = corpus(61, 2_000, true);
    for layout in layouts() {
        let files = Files::new();
        let (index, ..) = files.create(layout);
        insert_all(&index, &data);
        let query = vector(&mut Random(67), false);
        for least in [8.0_f64, 3.0_f64, 0.5_f64] {
            let mut rescore = |row: u64| Ok(exact(&data, &query, row));
            let found = index.within(&query, least, Ask { filter: None, rescore: Some(&mut rescore) }, &files.budget).unwrap();
            let expected: Vec<_> = brute(&data, &query, usize::MAX, &|_| true).into_iter().filter(|(_, s)| *s >= least).collect();
            assert!(found.complete);
            assert_eq!(scored(&found), expected, "{layout:?} {least}");
        }
        assert!(!index.within(&query, 0.0, Ask::default(), &files.budget).unwrap().complete);
    }
}

/// Verifies: [DESIGN] A query of no dimension the index holds, and k of
/// 0, find nothing; a search whose cursors and heap pass the memory
/// budget is refused with SQLCODE -918.
#[test]
fn empty_searches_and_the_memory_budget() {
    let files = Files::new();
    let (index, ..) = files.create(Layout::default());
    insert_all(&index, &corpus(71, 50, false));
    let none = SparseVector::new(VOCABULARY + 10, [(VOCABULARY + 5, 1.0)]).unwrap();
    assert_eq!(index.top_k(&none, 5, Ask::default(), &files.budget).unwrap().hits, []);
    let query = vector(&mut Random(73), false);
    assert_eq!(index.top_k(&query, 0, Ask::default(), &files.budget).unwrap().hits, []);
    let small = MemoryBudget::new(1_000);
    let refused = index.top_k(&query, 5, Ask::default(), &small).unwrap_err();
    assert_eq!(refused.code, Code::Resources);
}

/// Verifies: [DESIGN] Writers on several threads at once, inserting and
/// removing rows of shared dimensions, leave every count and posting as
/// one thread would: searches then equal brute force over the rows left,
/// whatever the interleaving (R-TEST-5).
#[test]
fn concurrent_writers_leave_every_posting() {
    let data = corpus(79, 2_400, false);
    let files = Files::new();
    let (index, ..) = files.create(Layout { quantization: Quantization::Float16, block: 32 });
    std::thread::scope(|scope| {
        for part in data.chunks(600) {
            let index = index.clone();
            scope.spawn(move || {
                for (row, v) in part {
                    index.insert(v, *row).unwrap();
                }
                for (row, v) in part.iter().filter(|(row, _)| row % 2 == 1) {
                    index.remove(v, *row).unwrap();
                }
            });
        }
    });
    let left: Vec<_> = data.iter().filter(|(r, _)| r % 2 == 0).cloned().collect();
    assert_eq!(index.documents(), Some(u64::try_from(left.len()).unwrap()));
    let mut random = Random(83);
    for _ in 0_u32..10 {
        let query = vector(&mut random, false);
        let mut rescore = |row: u64| Ok(exact(&left, &query, row));
        let found = index.top_k(&query, 25, Ask { filter: None, rescore: Some(&mut rescore) }, &files.budget).unwrap();
        assert_eq!(scored(&found), brute(&left, &query, 25, &|_| true));
    }
}
