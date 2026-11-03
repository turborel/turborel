//! Fixtures of replication's tests: groups of VR replicas, on the group of
//! `turborel-test-support`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use turborel_replication::{Consensus, Member, Output, Recovered, ReplicaId, Vr, VrOptions};
use turborel_test_support::replication::{Group, configuration};

/// Replicas 1 to `count`.
pub fn ids(count: u32) -> Vec<ReplicaId> {
    (1..=count).map(ReplicaId).collect()
}

/// The configuration of a group of `count` replicas, numbered from 1, at
/// no address.
pub fn config(count: u32) -> Vec<Member> {
    configuration(&ids(count))
}

/// The options of the tests: a heartbeat every 2 ticks, a view change after
/// 10, 8 records sent again.
pub fn options() -> VrOptions {
    VrOptions { heartbeat_ticks: 2, view_timeout_ticks: 10, resend_records: 8 }
}

/// A group of `count` VR replicas, numbered from 1, with instant disks.
pub fn vr_group(count: u32) -> Group {
    Group::new(&ids(count), Arc::new(make_vr))
}

/// The nonces of the recoveries made here, each once.
static NONCES: AtomicU64 = AtomicU64::new(1);

/// A nonce no replica made here had.
pub fn nonce() -> u64 {
    NONCES.fetch_add(1, Ordering::Relaxed)
}

/// A VR replica, new or made again from its journal.
pub fn make_vr(members: &[Member], me: ReplicaId, recovered: Option<Recovered>) -> Box<dyn Consensus> {
    Box::new(match recovered {
        None => Vr::new(members, me, options()).unwrap(),
        Some(recovered) => Vr::restart(members, me, options(), recovered, nonce()).unwrap(),
    })
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

/// Ticks replicas `ids`, `times` times each, delivering frames between
/// ticks but those to or from `cut`, which are lost.
pub fn run(group: &mut Group, ids: &[ReplicaId], times: u32, cut: &[ReplicaId]) {
    for _ in 0..times {
        for &id in ids {
            group.step(id, turborel_replication::Input::Tick);
        }
        settle_without(group, cut);
    }
}

/// Delivers frames until none waits, those to or from `cut` lost.
pub fn settle_without(group: &mut Group, cut: &[ReplicaId]) {
    loop {
        group.drop_frames(|f| cut.contains(&f.from) || cut.contains(&f.to));
        if !group.deliver() {
            return;
        }
    }
}

/// The views replica outputs say it served as primary in.
pub fn served(outputs: &[Output]) -> Vec<u64> {
    outputs
        .iter()
        .filter_map(|o| if let Output::Role(turborel_replication::Role::Primary { view }) = o { Some(*view) } else { None })
        .collect()
}
