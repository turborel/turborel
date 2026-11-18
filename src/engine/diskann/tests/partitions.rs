//! Local partitioned indexes: one graph per partition key.

use super::*;

/// Verifies: [EXTENSION] A partitioned index keeps one graph a key: a
/// search of one partition returns only its rows, a search of all merges
/// them nearest first and keeps the k nearest overall, and the index
/// reopens with every partition.
#[test]
fn partitions_search_alone_or_merged_and_reopen() {
    let points = clustered(900, 8, 9, 10);
    let (pool, file, budget) = pool();
    let index = Partitions::create(Arc::clone(&pool), file, layout(8, 16), (tuning(), plugins(false)), Arc::clone(&budget)).unwrap();
    let key = |row: u64| vec![u8::try_from(row.checked_rem(3).unwrap()).unwrap()];
    let mut entries = points.iter().cloned().zip(0_u64..).map(|(p, r)| Ok((key(r), p, r)));
    index.load(&mut entries).unwrap();
    index.insert(&key(1), &points[0], 5000).unwrap();
    assert_eq!(index.live(), 901);
    let q = points[4].clone();
    let (one, _) = index.search(&q, &ten(), Some(&[key(1)])).unwrap();
    assert!(one.iter().all(|(k, c)| *k == key(1) && (c.row.checked_rem(3) == Some(1) || c.row == 5000)));
    let (all, stats) = index.search(&q, &ten(), None).unwrap();
    assert_eq!(all.len(), 10);
    assert_eq!(all[0].1.row, 4);
    assert!(all.windows(2).all(|w| w[0].1.distance <= w[1].1.distance));
    assert_eq!(stats.rounds, 3);
    let expected = truth(&points, &q, 10);
    assert!(all.iter().filter(|(_, c)| expected.contains(&c.row)).count() >= 9);
    assert_eq!(index.rows(&key(1), &points[0]).unwrap(), [5000]);
    index.remove(&key(1), &points[0], 5000).unwrap();
    index.flush().unwrap();
    let reopened = Partitions::open(Arc::clone(&pool), file, (index.meta(), layout(8, 16)), (tuning(), plugins(false)), budget).unwrap();
    assert_eq!(reopened.all().unwrap().len(), 3);
    assert_eq!(reopened.search(&q, &ten(), None).unwrap().0, all);
}
