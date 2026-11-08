//! \[DESIGN\] A connection's session and wire, lent to the worker that
//! runs each request and given back once its replies are written
//! (RULES.md D42, D45). A fast client's next request the worker takes
//! itself: it waits on the wire, apart from the runtime, as long as
//! `WAITED`, its permit to run statements given up meanwhile for another
//! connection's, so a request answered wakes no thread of the server's
//! but the one that runs it, and a reply is written by the thread that
//! made it, from its own cache. A request that comes later, the
//! connection's task reads through the runtime, and lends the session and
//! wire again. A worker that cannot give the loan back as it should,
//! having panicked or never run, gives it back failed, so the connection
//! ends rather than waits for it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::{Notify, watch};
use turborel::error::{Result, SqlError};
use turborel_protocol::{Reply, broken};

use super::requests::{Held, Outgoing, Work};
use super::wire::{Arrived, Wire};
use crate::server::{Permit, Services};
use crate::shown::Present;

/// \[DESIGN\] How long a worker waits for a client's next request before
/// the connection's task does: a client in a tight loop answers within
/// microseconds, one that thinks within milliseconds or more, and a worker
/// is not kept from the connection's task for one. The system may round
/// it up to its timer's tick. Fixed, not a setting (RULES.md D36): the
/// shape of a hand-off between threads, which no work of the server's
/// tunes.
pub(super) const WAITED: Duration = Duration::from_millis(1);

/// What a worker is lent: the session and what it holds between requests,
/// the wire of the connection's messages, why the connection failed, if a
/// read or a write did, and whether its client is fast.
pub(super) struct Lent {
    pub(super) held: Held,
    pub(super) wire: Wire,
    pub(super) failed: Option<SqlError>,
    /// Whether the client's last request came within `WAITED` of being
    /// waited for, so a worker waits for its next.
    pub(super) fast: bool,
}

/// What says whether a worker waits on its client for the next request.
struct MayWait {
    /// The client's last request came quickly (`Lent::fast`).
    fast: bool,
    /// The wire is read apart from the runtime (`Wire::is_direct`).
    direct: bool,
    /// A statement of another connection's waits for a worker
    /// (`Services::waited_for`).
    waited_for: bool,
}

impl MayWait {
    /// Whether the worker waits.
    fn allowed(&self) -> bool {
        self.fast && self.direct && !self.waited_for
    }
}

impl Lent {
    /// `held` and `wire`, to be lent; the client taken as fast until it
    /// is slow.
    pub(super) fn new(held: Held, wire: Wire) -> Lent {
        Lent { held, wire, failed: None, fast: true }
    }

    /// Does `work` (`Held::handle`), its replies written through `running`
    /// in batches of about `batch` bytes, within `max_statements` prepared
    /// statements; shown running meanwhile.
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone.
    fn answer(&mut self, work: Work, running: &Running, (batch, max_statements): (usize, usize)) -> Result<()> {
        let Lent { held, wire, .. } = self;
        running.present.running(true);
        let mut sink = |replies: Vec<Reply>| running.send(wire, &replies);
        let mut out = Outgoing::new(&mut sink, batch);
        let written = held.handle(work, &mut out, max_statements).and_then(|()| out.finish());
        running.present.running(false);
        written
    }

    /// The client's next request, waited for `WAITED` apart from the
    /// runtime, if the client is fast and the wire can be read so, no
    /// statement waits for a worker (`MayWait`) and the connection may
    /// keep one (`Services::hot`); `permit` given up while it waits, and
    /// taken again once a request to run came. None, the client slow from
    /// now, if none came in time; none, too, for a request the connection's
    /// task is to read: one that is no work, or malformed, or that came
    /// once the connection is no longer served.
    ///
    /// # Errors
    ///
    /// The read failed: the connection is gone.
    fn next(&mut self, running: &Running, permit: &mut Permit) -> Result<Option<Work>> {
        let may = MayWait { fast: self.fast, direct: self.wire.is_direct(), waited_for: running.services.waited_for() };
        if !may.allowed() {
            return Ok(None);
        }
        let Some(kept) = running.services.hot() else { return Ok(None) };
        permit.release();
        let began = Instant::now();
        let next = loop {
            match self.wire.arrived(|request| super::work_of(request).ok().flatten().filter(|_| running.still())) {
                Arrived::Taken(work) => break Some(work),
                Arrived::Left => break None,
                Arrived::Awaited => {}
            }
            let left = WAITED.saturating_sub(began.elapsed());
            if left.is_zero() {
                self.fast = false;
                break None;
            }
            let came = self.wire.fill_within(left)?;
            if !came {
                self.fast = false;
                break None;
            }
        };
        drop(kept);
        if next.is_some() {
            permit.retake(&running.handle);
        }
        Ok(next)
    }
}

/// Where the loan is while no worker has it.
pub(super) struct Slot {
    lent: Mutex<Option<Lent>>,
    /// Told once each time the loan is given back.
    back: Notify,
}

impl Slot {
    /// An empty slot: the loan is the connection's task's.
    pub(super) fn new() -> Arc<Slot> {
        Arc::new(Slot { lent: Mutex::new(None), back: Notify::new() })
    }

    /// The loan, once it is given back.
    pub(super) async fn take(&self) -> Lent {
        loop {
            if let Some(lent) = self.locked().take() {
                return lent;
            }
            self.back.notified().await;
        }
    }

    /// Gives `lent` back, and tells who waits for it.
    fn give_back(&self, lent: Lent) {
        *self.locked() = Some(lent);
        self.back.notify_one();
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Option<Lent>> {
        // The loan is put and taken whole, so a poisoned lock is whole.
        self.lent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The loan, on the worker: given back as this is dropped, failed if the
/// worker panicked or never ran, as a runtime stopping has it.
pub(super) struct Loan {
    slot: Arc<Slot>,
    lent: Option<Lent>,
    ran: bool,
}

impl Loan {
    /// `lent`, taken from `slot`, to be given back there.
    pub(super) fn new(slot: &Arc<Slot>, lent: Lent) -> Loan {
        Loan { slot: Arc::clone(slot), lent: Some(lent), ran: false }
    }

    /// Does `work` and writes its replies, then each next request of a
    /// fast client that comes within `WAITED` while the connection is
    /// still served (`Lent::next`), with `permit`, the worker's, given up
    /// while it waits; the replies written through `running`, within
    /// `limits`: SERVER.BATCH_BYTES and SERVER.MAX_STATEMENTS. A read or a
    /// write that fails ends the request, the loan given back failed.
    pub(super) fn run(mut self, work: Work, running: &Running, mut permit: Permit, limits: (usize, usize)) {
        self.ran = true;
        let Some(lent) = self.lent.as_mut() else { return };
        let mut work = Some(work);
        while let Some(doing) = work.take() {
            let answered = lent.answer(doing, running, limits).and_then(|()| lent.next(running, &mut permit));
            match answered {
                Ok(following) => work = following,
                Err(e) => lent.failed = Some(e),
            }
        }
    }
}

impl Drop for Loan {
    fn drop(&mut self) {
        if let Some(mut lent) = self.lent.take() {
            if !self.ran || std::thread::panicking() {
                lent.failed = Some(broken("[DESIGN] a worker failed"));
            }
            self.slot.give_back(lent);
        }
    }
}

/// The connection, as its worker runs for it: the runtime to send through,
/// what it shows, and whether it is still served.
#[derive(Clone)]
pub(super) struct Running {
    services: Services,
    handle: Handle,
    present: Present,
    /// Whether an operator ended the connection.
    ended: watch::Receiver<bool>,
    /// The user whose session it is, if a user's (RULES.md D43).
    user: Option<String>,
}

impl Running {
    /// The connection `present` shows, of `user` if a user's, on the
    /// runtime of the task calling this.
    pub(super) fn new(services: &Services, present: &Present, user: Option<&str>) -> Running {
        let (services, present, ended, user) = (services.clone(), present.clone(), present.watching(), user.map(str::to_owned));
        Running { services, handle: Handle::current(), present, ended, user }
    }

    /// Whether the connection is still served, as the task asks before
    /// each request it reads: the server taking statements, its operator
    /// not having ended it, and its user, if it is a user's, not dropped
    /// (RULES.md D43).
    fn still(&self) -> bool {
        let db = self.services.database();
        // The watch's guard is dropped before the database is asked, so an
        // operator ending the connection never waits on that call (R-ERR-4).
        let ended = *self.ended.borrow();
        self.services.is_serving() && !ended && self.user.as_deref().is_none_or(|name| db.is_user(name).unwrap_or(false))
    }

    /// Sends `replies` through `wire`: apart from the runtime, as far as
    /// the wire allows; the rest on the runtime, within
    /// SERVER.IDLE_TIMEOUT_MS (`Services::idle`), unless the operator ends
    /// the connection first.
    ///
    /// # Errors
    ///
    /// The write failed, or ran out of time; the operator ended the
    /// connection.
    fn send(&self, wire: &mut Wire, replies: &[Reply]) -> Result<()> {
        if wire.sent_now(replies)? {
            return Ok(());
        }
        self.handle.block_on(async {
            tokio::select! {
                sent = self.services.idle(wire.send_rest()) => sent,
                () = self.present.ended() => Err(super::ended()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: a worker waits on its client for
    /// the next request only while the client is fast, the wire is read
    /// apart from the runtime, and no statement waits for a worker. MC/DC
    /// pairs, from all allowed: `fast` false; `direct` false; `waited_for`
    /// true: each alone forbids it.
    #[test]
    fn a_worker_waits_on_a_fast_client_read_directly_while_none_waits_for_it() {
        assert!(MayWait { fast: true, direct: true, waited_for: false }.allowed());
        assert!(!MayWait { fast: false, direct: true, waited_for: false }.allowed(), "slow");
        assert!(!MayWait { fast: true, direct: false, waited_for: false }.allowed(), "through the runtime only");
        assert!(!MayWait { fast: true, direct: true, waited_for: true }.allowed(), "a statement waits for a worker");
    }

    /// Verifies: [DESIGN] RULES.md D42: the loan is taken once it is given
    /// back, whoever waits for it first; a loan dropped by a worker that
    /// never ran comes back failed, so the connection ends.
    #[test]
    fn a_loan_dropped_unrun_comes_back_failed() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let slot = Slot::new();
            let (near, _far) = tokio::io::duplex(16);
            let wire = Wire::new(Box::new(near), 16, turborel::value::types::TypeRegistry::default());
            let lent = Lent::new(Held::new(turborel::db::Database::new().session("X")), wire);
            let taking = tokio::spawn({
                let slot = Arc::clone(&slot);
                async move { slot.take().await }
            });
            tokio::task::yield_now().await;
            drop(Loan::new(&slot, lent));
            let back = taking.await.unwrap();
            assert!(back.failed.is_some_and(|e| e.msg.contains("a worker failed")));
            assert!(back.fast, "taken as fast until it is slow");
        });
    }
}
