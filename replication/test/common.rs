//! Fixtures of replication's tests: groups of VR replicas, on the group of
//! `turborel-test-support`.

use turborel_replication::{Output, ReplicaId, Vr, VrOptions};
use turborel_test_support::replication::Group;

/// Replicas 1 to `count`.
pub fn ids(count: u32) -> Vec<ReplicaId> {
    (1..=count).map(ReplicaId).collect()
}

/// The options of the tests: a heartbeat every 2 ticks, a view change after
/// 10, 8 records sent again.
pub fn options() -> VrOptions {
    VrOptions { heartbeat_ticks: 2, view_timeout_ticks: 10, resend_records: 8 }
}

/// A group of `count` VR replicas, numbered from 1, with instant disks.
pub fn vr_group(count: u32) -> Group {
    Group::new(&ids(count), Box::new(|members, me, _| Box::new(Vr::new(members, me, options()).unwrap())))
}

/// A group of `count` VR replicas whose first has begun to serve as
/// primary of view 0.
pub fn serving(count: u32) -> Group {
    let mut group = vr_group(count);
    group.tick();
    group.settle();
    group
}

/// The `Committed` positions among `outputs`.
pub fn committed(outputs: &[Output]) -> Vec<u64> {
    outputs.iter().filter_map(|o| if let Output::Committed(at) = o { Some(at.0) } else { None }).collect()
}

/// A record of the tests, `text` as bytes.
pub fn record(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}
