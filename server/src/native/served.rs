//! \[DESIGN\] Where a connection's requests run (RULES.md D767): each with
//! a worker's permit taken first, on the connection's home
//! (`native/home.rs`) while it has one, else on any free thread of the
//! blocking pool, as every request ran before homes. Its session's end, once
//! a connection, runs on any free thread (`native.rs`).

use super::home::{Home, Job, KEPT};
use super::lent::{Loan, Running};
use super::requests::Work;
use crate::server::{Permit, Services};

/// A request of a connection's, its permit taken, as `Loan::run` does it.
pub(super) struct Served {
    pub(super) loan: Loan,
    pub(super) work: Work,
    pub(super) running: Running,
    pub(super) permit: Permit,
    /// SERVER.BATCH_BYTES, SERVER.MAX_STATEMENTS and SERVER.LOB_INLINE_BYTES.
    pub(super) limits: (usize, usize, usize),
}

impl Job for Served {
    fn run(self) {
        self.run_on(true);
    }
}

impl Served {
    /// Runs it, on its connection's home if `homed`.
    fn run_on(self, homed: bool) {
        let Served { loan, work, mut running, permit, limits } = self;
        running.at_home(homed);
        loan.run(work, &running, permit, limits);
    }

    /// Runs it on any free thread of the blocking pool. Not waited for: the
    /// loan comes back as it is done; a runtime stopping drops it unrun,
    /// which gives the loan back failed.
    fn apart(self) {
        drop(tokio::task::spawn_blocking(move || self.run_on(false)));
    }
}

/// Runs `served` on the connection's `home`; on a new home if it has none,
/// or its home retired, and a place is free (`Services::hot`); else apart.
pub(super) fn homed(home: &mut Option<Home<Served>>, services: &Services, served: Served) {
    let Some(served) = sent(home.as_ref(), served) else { return };
    *home = services.hot().map(|place| Home::open(place, KEPT));
    if let Some(served) = sent(home.as_ref(), served) {
        served.apart();
    }
}

/// `served` back unless `home` took it.
fn sent(home: Option<&Home<Served>>, served: Served) -> Option<Served> {
    match home {
        Some(home) => home.send(served).err(),
        None => Some(served),
    }
}
