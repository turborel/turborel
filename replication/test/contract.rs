//! The contract tests of replication's extension points (R-SAFE-3), run
//! against each implementation: VR, and the simulator's transport.

use turborel_replication::ReplicaId;
use turborel_test_support::replication::Make;
use turborel_test_support::replication::contract::{check_consensus, check_transport};
use turborel_test_support::replication::sim::Link;

use crate::common::{ids, make_vr};

/// Verifies: [DESIGN] RULES.md D57, R-SAFE-3 VR keeps the `Consensus`
/// contract: a new group of one serves; appends are given the next
/// positions and persisted, and committed once durable, never going back;
/// frames malformed or from no member are refused, changing nothing; it is
/// deterministic.
#[test]
fn vr_keeps_the_consensus_contract() {
    let make: Make = Box::new(make_vr);
    check_consensus(&make);
}

/// Verifies: [DESIGN] RULES.md D57, R-SAFE-3 the simulator's link keeps the
/// `Transport` contract: its members in order, a send to each taken, a send
/// to no member refused (SQLCODE -910).
#[test]
fn the_simulators_link_keeps_the_transport_contract() {
    let link = Link::new(&ids(3));
    check_transport(&link, ReplicaId(9));
    assert_eq!(link.take().len(), 3);
}
