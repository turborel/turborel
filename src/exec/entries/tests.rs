//! A new projected probe renews locality without discarding read state.

use super::*;
use crate::txn::NO_TRANSACTION;

struct Identity;

impl KeyProjection for Identity {
    fn project(&self, key: &Key) -> Result<Key> {
        Ok(key.clone())
    }
}

/// Verifies: [DESIGN] D31, D85: independent projected probes renew the
/// storage leaf trial after a sparse probe disabled it. The previous
/// leaf, reusable allocation and MVCC bookkeeping remain available;
/// ordinary ordered index probes retain their existing miss history.
#[test]
fn projected_probes_renew_locality_without_dropping_read_state() {
    let bookkeeping: Arc<dyn std::any::Any + Send + Sync> = Arc::new(17_u32);
    for projected in [false, true] {
        let identity: Arc<dyn KeyProjection> = Arc::new(Identity);
        let projection = projected.then_some(identity);
        let mut reading = Reading {
            columns: Some(vec![0]),
            near: Near {
                leaf: Some((7, 9)),
                misses: 8,
                at: Some(3),
                bytes: vec![1, 2, 3],
                open: Some((NO_TRANSACTION, 4, Arc::clone(&bookkeeping))),
            },
            projection,
        };
        let allocation = reading.near.bytes.as_ptr();
        reading.next_probe();
        assert_eq!(reading.near.misses, if projected { 0 } else { 8 });
        assert_eq!(reading.near.leaf, Some((7, 9)));
        assert_eq!(reading.near.at, Some(3));
        assert_eq!(reading.near.bytes, [1, 2, 3]);
        assert_eq!(reading.near.bytes.as_ptr(), allocation);
        let (tx, table, held) = reading.near.open.unwrap();
        assert_eq!((tx, table), (NO_TRANSACTION, 4));
        assert!(Arc::ptr_eq(&held, &bookkeeping));
    }
}
