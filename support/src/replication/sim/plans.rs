//! \[TOOL\] The plans the simulator's sweeps run (RULES.md D57, R-TEST-5),
//! each named: the tests of `turborel-replication` run a bounded number
//! of seeds of each, so `verify` stays quick, and `cargo xtask simulate`
//! as many as asked. Each is a group through faults at random: frames
//! delayed, lost, duplicated and reordered, partitions, crashes and
//! restarts, slow disks losing writes not yet durable and failing,
//! journals lost; some keep little of the log, so replicas fetch state;
//! one reconfigures the group, spares joining as members leave.

use super::{Faults, Plan, Weather};

/// A plan's name, and what makes it.
pub type Named = (&'static str, fn() -> Plan);

/// A plan by name, as `cargo xtask simulate --plan NAME` picks it.
pub const PLANS: [Named; 7] = [
    ("three", three),
    ("five", five),
    ("storm", storm),
    ("retaining", retaining),
    ("losing", losing),
    ("databases", databases),
    ("reconfiguring", reconfiguring),
];

/// The plan named `name`, if one is.
pub fn named(name: &str) -> Option<Plan> {
    PLANS.iter().find(|(known, _)| *known == name).map(|(_, plan)| plan())
}

/// A run of `replicas` replicas: 3000 moments of faults, 2000 calm; a
/// record appended at one moment in ten; a replica ticking every 5. Of
/// the frames, 5 % are lost, 2 % delivered twice, 1 % held up to 300
/// moments, the rest taking up to 10; a partition begins at 3 moments in
/// a thousand, a replica crashes at 4 in a thousand and stays down up to
/// 60, a disk fails on 2 writes in a thousand.
fn of(replicas: u32) -> Plan {
    let weather = Weather { drop: 50, duplicate: 20, delay: 10, late: 10, late_delay: 300 };
    let faults = Faults {
        weather,
        partition: 3,
        partition_moments: 400,
        crash: 4,
        down_moments: 60,
        disk_error: 2,
        lose_journal: 0,
        write_moments: 6,
        reconfigure: 0,
    };
    Plan { replicas, spares: 0, faulty: 3000, calm: 2000, append: 100, tick_moments: 5, retain_bytes: u64::MAX, faults }
}

/// A group of three.
pub fn three() -> Plan {
    of(3)
}

/// A group of five, whose quorums are three.
pub fn five() -> Plan {
    of(5)
}

/// A group of three through storms: a quarter of frames lost, a tenth
/// twice, partitions and crashes several times as often, a disk failing
/// on one write in a hundred.
pub fn storm() -> Plan {
    let mut storm = of(3);
    storm.faults = Faults {
        weather: Weather { drop: 250, duplicate: 100, delay: 30, late: 30, late_delay: 500 },
        partition: 10,
        partition_moments: 300,
        crash: 8,
        down_moments: 100,
        disk_error: 10,
        lose_journal: 0,
        write_moments: 20,
        reconfigure: 0,
    };
    storm
}

/// Three replicas whose logs keep 200 bytes, crashing at 6 moments in a
/// thousand and down up to 400: a replica down a while is behind what
/// the others retain, and fetches the state.
pub fn retaining() -> Plan {
    let weather = Weather { drop: 20, duplicate: 10, delay: 5, late: 5, late_delay: 100 };
    let faults = Faults {
        weather,
        partition: 2,
        partition_moments: 300,
        crash: 6,
        down_moments: 400,
        disk_error: 0,
        lose_journal: 0,
        write_moments: 3,
        reconfigure: 0,
    };
    Plan { replicas: 3, spares: 0, faulty: 3000, calm: 3000, append: 300, tick_moments: 5, retain_bytes: 200, faults }
}

/// Three replicas keeping 200 bytes of the log, crashing often, half of
/// the crashes losing the journal.
pub fn losing() -> Plan {
    let weather = Weather { drop: 20, duplicate: 10, delay: 5, late: 5, late_delay: 100 };
    let faults = Faults {
        weather,
        partition: 2,
        partition_moments: 200,
        crash: 6,
        down_moments: 100,
        disk_error: 0,
        lose_journal: 500,
        write_moments: 3,
        reconfigure: 0,
    };
    Plan { replicas: 3, spares: 0, faulty: 3000, calm: 3000, append: 300, tick_moments: 5, retain_bytes: 200, faults }
}

/// Three replicas with real databases: 2000 moments of faults, 2000
/// calm; the primary's database acting at one moment in five; faults as
/// `three` has them, but a replica down up to 60 moments and a journal
/// lost at one crash in four, the log keeping 4096 bytes.
pub fn databases() -> Plan {
    let weather = Weather { drop: 50, duplicate: 20, delay: 10, late: 10, late_delay: 300 };
    let faults = Faults {
        weather,
        partition: 3,
        partition_moments: 400,
        crash: 4,
        down_moments: 60,
        disk_error: 2,
        lose_journal: 250,
        write_moments: 6,
        reconfigure: 0,
    };
    Plan { replicas: 3, spares: 0, faulty: 2000, calm: 2000, append: 200, tick_moments: 5, retain_bytes: 4096, faults }
}

/// Three replicas and four spares, through the faults of `three` but
/// with journals lost at one crash in four, the log keeping 300 bytes;
/// the primary's database asked a reconfiguration at 3 moments in a
/// thousand while a spare waits: a spare joins, a member leaves, the
/// group of three moving over the seven nodes, new members recovering
/// from the group, fetching the state when behind what it keeps.
pub fn reconfiguring() -> Plan {
    let mut plan = of(3);
    plan.spares = 4;
    plan.retain_bytes = 300;
    plan.faults.lose_journal = 250;
    plan.faults.reconfigure = 3;
    plan
}
