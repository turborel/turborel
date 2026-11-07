//! \[DESIGN\] A connection's session and the writer of its replies, lent
//! to the worker that runs each request and given back once its replies
//! are written (RULES.md D42, D45). The connection's task does not wait
//! for the worker: it reads the next request meanwhile, and waits for the
//! loan only once it has one to run, or the connection ends. So a request
//! answered wakes its worker and no other thread of the server's, and the
//! replies are written by the thread that made them, from its own cache.
//! A worker that cannot give the loan back as it should, having panicked
//! or never run, gives it back failed, so the connection ends rather than
//! waits for it.
//!
//! The session's transaction, active or not, says how long the task
//! waits for the next request (`native::awaited`): the worker gives the
//! loan back saying so, and wakes the task when that changed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::sync::futures::Notified;
use turborel::error::{Result, SqlError};
use turborel_protocol::{Reply, broken};

use super::requests::{Held, Outgoing, Work};
use super::wire::Writer;
use crate::server::Services;
use crate::shown::Present;

/// What a worker is lent: the session and what it holds between requests,
/// the writer of the connection's replies, and why the connection failed,
/// if a write did.
pub(super) struct Lent {
    pub(super) held: Held,
    pub(super) writer: Writer,
    pub(super) failed: Option<SqlError>,
}

/// Where the loan is while no worker has it.
pub(super) struct Slot {
    lent: Mutex<Option<Lent>>,
    /// Told once each time the loan is given back.
    back: Notify,
    /// Whether the session's transaction was active as the loan was last
    /// given back.
    active: AtomicBool,
    /// Told when `active` changed as the loan was given back.
    changed: Notify,
}

impl Slot {
    /// The loan of `held` and `writer`, in its slot.
    pub(super) fn new(held: Held, writer: Writer) -> Arc<Slot> {
        let active = AtomicBool::new(held.session.transaction().is_some());
        let lent = Mutex::new(Some(Lent { held, writer, failed: None }));
        Arc::new(Slot { lent, back: Notify::new(), active, changed: Notify::new() })
    }

    /// Whether the session's transaction was active as the loan was last
    /// given back.
    pub(super) fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Done once `active` changes after it is enabled.
    pub(super) fn changed(&self) -> Notified<'_> {
        self.changed.notified()
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

    /// Gives `lent` back, and tells who waits for it, and who waits for
    /// its transaction to begin or end.
    fn give_back(&self, lent: Lent) {
        let active = lent.held.session.transaction().is_some();
        let before = self.active.swap(active, Ordering::AcqRel);
        *self.locked() = Some(lent);
        self.back.notify_one();
        if before != active {
            self.changed.notify_waiters();
        }
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

    /// Does `work`, then `then` as `Held::handle_then` says, the replies
    /// written through `sending` in batches of about `batch` bytes, within
    /// `max_statements` prepared statements; shown running meanwhile. A
    /// write that fails ends the request, the loan given back failed.
    pub(super) fn run(
        mut self,
        (work, then): (Work, Option<(Work, impl FnOnce() -> bool)>),
        sending: &Sending,
        (batch, max_statements): (usize, usize),
    ) {
        self.ran = true;
        let Some(Lent { held, writer, failed }) = self.lent.as_mut() else { return };
        sending.present.running(true);
        let mut sink = |replies: Vec<Reply>| sending.send(writer, &replies);
        let mut out = Outgoing::new(&mut sink, batch);
        let written = held.handle_then(work, then, &mut out, max_statements).and_then(|()| out.finish());
        *failed = written.err();
        sending.present.running(false);
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

/// How a worker sends replies: on the connection's runtime, each batch
/// within SERVER.IDLE_TIMEOUT_MS (`Services::idle`), until the operator
/// ends the connection.
#[derive(Clone)]
pub(super) struct Sending {
    services: Services,
    handle: Handle,
    present: Present,
}

impl Sending {
    /// Sending on the runtime of the task calling this, for the connection
    /// `present` shows.
    pub(super) fn new(services: &Services, present: &Present) -> Sending {
        Sending { services: services.clone(), handle: Handle::current(), present: present.clone() }
    }

    /// Sends `replies` through `writer`.
    ///
    /// # Errors
    ///
    /// The write failed, or ran out of time; the operator ended the
    /// connection.
    fn send(&self, writer: &mut Writer, replies: &[Reply]) -> Result<()> {
        self.handle.block_on(async {
            tokio::select! {
                sent = self.services.idle(writer.send_all(replies)) => sent,
                () = self.present.ended() => Err(super::ended()),
            }
        })
    }
}
