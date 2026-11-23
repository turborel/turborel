//! \[DESIGN\] A connection's session and wire, lent to the worker that
//! runs each request and given back once its replies are written
//! (RULES.md D42, D45). A fast client's next request the worker takes
//! itself: it waits on the wire, apart from the runtime, as long as
//! `WAITED`, its permit to run statements given up meanwhile for another
//! connection's, so a request answered wakes no thread of the server's
//! but the one that runs it, and a reply is written by the thread that
//! made it, from its own cache. The connection's home waits on, as long
//! as `waiting::HOME_WAITED`, its permit given up once `WAITED` has
//! passed (RULES.md D839). A request that comes later, the connection's
//! task reads through the runtime, and lends the session and wire again. A request that has come whole behind the one answered the
//! worker takes at once, its replies joining the last one's in one write
//! (RULES.md D231); a client slow to take them it waits on in the write
//! itself (D232). A worker that cannot give the loan back as it should,
//! having panicked or never run, gives it back failed, so the connection
//! ends rather than waits for it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::{Notify, watch};
use turborel::db::Session;
use turborel::error::{Result, SqlError};
use turborel_protocol::{Reply, broken};

use super::requests::{Held, Outgoing, Work};
use super::seen::UserSeen;
use super::waiting::Waiting;
use super::wire::{Arrived, Wire};
use crate::server::{IDLE_WAITING, Permit, Services};
use crate::settings;
use crate::shown::Present;

/// \[DESIGN\] How long a worker waits for a client's next request before
/// the connection's task does: a client in a tight loop answers within
/// microseconds, one that thinks within milliseconds or more, and a worker
/// is not kept from the connection's task for one. The system may round
/// it up to its timer's tick. Fixed, not a setting (RULES.md D36): the
/// shape of a hand-off between threads, which no work of the server's
/// tunes.
pub(super) const WAITED: Duration = Duration::from_millis(1);

/// \[DESIGN\] How often a worker waiting on its client to take its replies
/// looks whether the operator ended the connection or the server hurries:
/// as often as an operator's end cancels a statement again
/// (`native.rs`'s `CANCEL_AGAIN`), so either is seen as soon. Fixed, not a
/// setting (RULES.md D36).
pub(super) const LOOKED: Duration = Duration::from_millis(20);

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
    /// When the worker that gave the client back to the connection's task
    /// began to wait on it, which the task's wait counts on from, so a
    /// client's idle time is the same whoever waits for it (RULES.md D839).
    pub(super) idle_since: Option<Instant>,
}

/// What says whether a worker waits on its client for the next request.
struct MayWait {
    /// The client's last request came quickly (`Lent::fast`).
    fast: bool,
    /// The worker is the connection's home (`Running::homed`).
    homed: bool,
    /// The wire is read apart from the runtime (`Wire::is_direct`).
    direct: bool,
}

impl MayWait {
    /// Whether the worker waits: a home whatever its client's pace, any
    /// other worker only for a fast client. A statement of another
    /// connection's waiting for a worker does not stop it: waiting, it
    /// holds no permit past `WAITED`.
    fn allowed(&self) -> bool {
        (self.fast || self.homed) && self.direct
    }
}

impl Lent {
    /// `held` and `wire`, to be lent; the client taken as fast until it
    /// is slow.
    pub(super) fn new(held: Held, wire: Wire) -> Lent {
        Lent { held, wire, failed: None, fast: true, idle_since: None }
    }

    /// Does `work` (`Held::handle`), its replies written through `running`
    /// in batches of about `batch` bytes, within `max_statements` prepared
    /// statements; shown running meanwhile. Its last replies are queued
    /// rather than written while the client's next request has come whole
    /// and fewer than `batch` bytes are queued: a client that pipelines its
    /// requests reads their replies in one write (RULES.md D231).
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone.
    fn answer(&mut self, work: Work, running: &Running, (batch, max_statements, inline): (usize, usize, usize)) -> Result<()> {
        let Lent { held, wire, .. } = self;
        running.present.running(true);
        let mut sink = |replies: Vec<Reply>| {
            wire.queue(&replies);
            running.flush(wire)
        };
        let mut out = Outgoing::new(&mut sink, batch).inlining(inline);
        let handled = held.handle(work, &mut out, max_statements);
        let last = out.finish();
        running.present.running(false);
        handled?;
        wire.queue(&last);
        let pipelined = Pipelined { whole: wire.has_whole(), room: wire.queued() < batch };
        if pipelined.held() {
            return Ok(());
        }
        let sent = running.flush(wire);
        // [DESIGN] RULES.md D450: the garbage of a transaction it ended,
        // collected once its reply is on its way.
        held.session.collect();
        sent
    }

    /// The client's next request, if it has come whole, or else, waited
    /// for apart from the runtime, if the client is fast or the worker is
    /// its home, and the wire can be read so (`MayWait`), and the
    /// connection may keep a worker (`Services::hot`): `WAITED`, and by a
    /// home `waiting::HOME_WAITED` (RULES.md D839); `permit` lent
    /// meanwhile, given up if another waits for one (`Permit::lend`, D815)
    /// or once `WAITED` has passed, and taken again once a request to run
    /// came, else given up. The replies queued are sent before it waits,
    /// and before it gives the client back to the connection's task. None,
    /// the client slow from now, if none came in time; none, too, for a
    /// request the connection's task is to read: one that is no work, or
    /// malformed, or that came once the connection is no longer served.
    ///
    /// # Errors
    ///
    /// The read or a write failed: the connection is gone.
    fn next(&mut self, running: &Running, permit: &mut Permit) -> Result<Option<Work>> {
        if !self.wire.has_whole() {
            running.flush(&mut self.wire)?;
        }
        permit.lend();
        let since = Instant::now();
        let next = self.arrived_or_waited(running, permit)?;
        self.idle_since = next.is_none().then_some(since);
        if next.is_some() {
            running.present.queued();
            permit.retake(&running.handle);
        } else {
            permit.release();
            running.flush(&mut self.wire)?;
        }
        Ok(next)
    }

    /// The client's next request, taken if it has come whole, or waited
    /// for as `next` says: by a home as long as `HOME_WAITED`, and by any
    /// other worker as long as `WAITED`, `permit` given up once `WAITED`
    /// has passed (RULES.md D839).
    ///
    /// # Errors
    ///
    /// The read failed: the connection is gone.
    fn arrived_or_waited(&mut self, running: &Running, permit: &mut Permit) -> Result<Option<Work>> {
        match self.taken(running) {
            Arrived::Taken(work) => return Ok(Some(work)),
            Arrived::Left => return Ok(None),
            Arrived::Awaited => {}
        }
        let may = MayWait { fast: self.fast, homed: running.homed, direct: self.wire.is_direct() };
        if !may.allowed() {
            return Ok(None);
        }
        // A home waits on its own thread, which holds a place already.
        let kept = if running.homed {
            None
        } else {
            let Some(place) = running.services.hot() else { return Ok(None) };
            Some(place)
        };
        let waiting = Waiting::new(running.homed, Instant::now());
        let mut waiting = if self.fast { waiting } else { waiting.of_slow_client() };
        let next = loop {
            let session = &self.held.session;
            let Some(slice) = waiting.slice(|| running.leaving(), || running.idle_bound(session)) else {
                self.fast = false;
                break None;
            };
            if waiting.slow() {
                permit.release();
            }
            if !self.wire.fill_within(slice)? {
                continue;
            }
            match self.taken(running) {
                Arrived::Taken(work) => {
                    self.fast = waiting.quick();
                    break Some(work);
                }
                Arrived::Left => break None,
                Arrived::Awaited => {}
            }
        };
        drop(kept);
        Ok(next)
    }

    /// The client's next request, as `Wire::arrived` finds it; work taken
    /// has its wait for a worker measured from now (`Held::arrived`).
    fn taken(&mut self, running: &Running) -> Arrived<Work> {
        let granted = self.held.granted;
        let arrived = self.wire.arrived(|request| super::next::work_of(request, granted).filter(|_| running.still()));
        if let Arrived::Taken(_) = arrived {
            self.held.arrived();
        }
        arrived
    }
}

/// What says whether a request's last replies wait for the next's.
struct Pipelined {
    /// The client's next request has come whole (`Wire::has_whole`).
    whole: bool,
    /// Fewer than SERVER.BATCH_BYTES are queued.
    room: bool,
}

impl Pipelined {
    /// Whether they wait: a reply is never held for a request not yet
    /// sent, nor past a batch.
    fn held(&self) -> bool {
        self.whole && self.room
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
    /// `limits`: SERVER.BATCH_BYTES, SERVER.MAX_STATEMENTS and
    /// SERVER.LOB_INLINE_BYTES. A read or a
    /// write that fails ends the request, the loan given back failed.
    pub(super) fn run(mut self, work: Work, running: &Running, mut permit: Permit, limits: (usize, usize, usize)) {
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
    /// Whether its worker is the connection's home (`native/home.rs`).
    homed: bool,
    /// Whether its user was a user, as last asked (`native/seen.rs`).
    user_seen: Arc<UserSeen>,
}

impl Running {
    /// The connection `present` shows, of `user` if a user's, on the
    /// runtime of the task calling this.
    pub(super) fn new(services: &Services, present: &Present, user: Option<&str>) -> Running {
        let (services, present, ended, user) = (services.clone(), present.clone(), present.watching(), user.map(str::to_owned));
        Running { services, handle: Handle::current(), present, ended, user, homed: false, user_seen: Arc::default() }
    }

    /// Its worker the connection's home if `homed`, else a thread of the
    /// pool's taken for this request alone.
    pub(super) fn at_home(&mut self, homed: bool) {
        self.homed = homed;
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
        self.services.is_serving()
            && !ended
            && self.user.as_deref().is_none_or(|name| self.user_seen.is(db.publications(), || db.is_user(name).unwrap_or(false)))
    }

    /// Whether the connection's task is to take the client back: its
    /// operator ended it, or the server no longer takes statements.
    pub(super) fn leaving(&self) -> bool {
        *self.ended.borrow() || !self.services.is_serving()
    }

    /// The longest a home waits on its client for the next request, from
    /// when it began: `HOME_WAITED`, or SERVER.IDLE_TIMEOUT_MS if less, or
    /// SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS if less while `session`'s
    /// transaction is active, so the connection's task, counting from
    /// then too, ends the connection as it would have.
    pub(super) fn idle_bound(&self, session: &Session) -> Duration {
        let db = self.services.database();
        let idle = settings::millis(db, settings::IDLE_TIMEOUT_MS);
        let active = session.transaction().is_some();
        let in_transaction = active.then(|| settings::millis(db, settings::IDLE_IN_TRANSACTION_TIMEOUT_MS)).flatten();
        super::waiting::bounded(idle, in_transaction)
    }

    /// Sends the replies queued in `wire`: apart from the runtime, waiting
    /// on the socket itself if the wire allows (`flush_waiting`); else the
    /// rest on the runtime, within SERVER.IDLE_TIMEOUT_MS
    /// (`Services::idle`), unless the operator ends the connection first.
    ///
    /// # Errors
    ///
    /// The write failed, or ran out of time; the operator ended the
    /// connection.
    fn flush(&self, wire: &mut Wire) -> Result<()> {
        if wire.queued() == 0 {
            return Ok(());
        }
        let all = wire.sent_now()?;
        if all {
            return Ok(());
        }
        if wire.is_direct() {
            return self.flush_waiting(wire);
        }
        self.handle.block_on(async {
            tokio::select! {
                sent = self.services.idle(wire.send_rest()) => sent,
                () = self.present.ended() => Err(super::ended()),
            }
        })
    }

    /// Sends the replies queued in `wire`, the worker waiting on the socket
    /// itself as the client takes them, so a client that reads as fast as
    /// they are made is fed at the socket's pace, its worker woken by the
    /// system alone (RULES.md D232). It looks every `LOOKED` whether the
    /// operator ended the connection or the server hurries, and gives up
    /// once SERVER.IDLE_TIMEOUT_MS passes before the client has taken them
    /// all, as `Services::idle` would.
    ///
    /// # Errors
    ///
    /// As `flush`'s; the server hurried.
    fn flush_waiting(&self, wire: &mut Wire) -> Result<()> {
        let limit = settings::millis(self.services.database(), settings::IDLE_TIMEOUT_MS);
        let began = Instant::now();
        loop {
            if *self.ended.borrow() {
                return Err(super::ended());
            }
            if self.services.is_hurried() {
                return Err(crate::server::hurrying());
            }
            let left = limit.map_or(LOOKED, |l| l.saturating_sub(began.elapsed()));
            if left.is_zero() {
                return Err(crate::server::went_past(settings::IDLE_TIMEOUT_MS, limit.unwrap_or_default(), IDLE_WAITING));
            }
            if wire.sent_within(left.min(LOOKED))? {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42, D839: a worker waits on its client
    /// for the next request only while the client is fast, or the worker
    /// is its home, and the wire is read apart from the runtime. MC/DC
    /// pairs: from fast alone allowed, `fast` false forbids it and `homed`
    /// true then allows it again; `direct` false alone forbids it.
    #[test]
    fn a_worker_waits_on_a_fast_client_read_directly() {
        assert!(MayWait { fast: true, homed: false, direct: true }.allowed());
        assert!(!MayWait { fast: false, homed: false, direct: true }.allowed(), "slow");
        assert!(MayWait { fast: false, homed: true, direct: true }.allowed(), "a home waits on a slow client");
        assert!(!MayWait { fast: true, homed: false, direct: false }.allowed(), "through the runtime only");
        assert!(!MayWait { fast: true, homed: true, direct: false }.allowed(), "a home through the runtime only");
    }

    /// Verifies: [DESIGN] RULES.md D231: a request's last replies wait for
    /// the next request's only while that has come whole and fewer than a
    /// batch's bytes wait. MC/DC pairs, from both: `whole` false; `room`
    /// false: each alone sends them.
    #[test]
    fn replies_wait_only_for_a_request_come_whole_within_a_batch() {
        assert!(Pipelined { whole: true, room: true }.held());
        assert!(!Pipelined { whole: false, room: true }.held(), "no request come");
        assert!(!Pipelined { whole: true, room: false }.held(), "a batch waits already");
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
            let db = turborel::db::Database::new();
            let serves = super::super::requests::Serves { db: db.clone(), requests: Arc::default() };
            let lent = Lent::new(Held::new(db.session("X"), serves, turborel_protocol::Capabilities::NONE), wire);
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
