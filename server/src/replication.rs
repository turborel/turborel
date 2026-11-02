//! \[EXTENSION\] A node of a replica group (RULES.md D57): the server's
//! part of replication, joining the replication crate's driver to the
//! network. A database replicated is one whose node's settings say so
//! (REPLICATION.REPLICA, the node's number among REPLICATION.MEMBERS):
//! before it is opened, `prepare` gives it the group's log storage
//! (`GroupLogs::in_files`), its journal in the directory's `log/`, over a
//! driver of Viewstamped Replication joined to the transport of
//! `replication/net.rs`; opened to follow (`Database::open_following`),
//! `Prepared::start` joins the driver to it, runs the driver as the
//! database's job `turborel-replica` (`DriverJob`), has the transport
//! listen and link to the other members, and registers
//! INFORMATION_SCHEMA.REPLICAS (`replication/shown.rs`). What the
//! consensus and the transport do is told as `Event::Replica`
//! (`Telling`). The database serves as its replica does: as primary of a
//! view, taking changes; as backup, reads alone (SQLCODE -917).
//!
//! Not yet: state transfer, log retention, recovery of a journal lost,
//! reconfiguration: a database so far behind is opened again from a copy
//! of the primary's files.

mod net;
mod shown;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use turborel::db::{Database, LogStorage, OpenedLog, Standby, StorePlace};
use turborel::diagnostics::{Event, ReplicaChange};
use turborel::error::{Code, Result, SqlError};
use turborel::log::{FileLogOptions, Log, Lsn};
use turborel::threads::JobHandle;
use turborel_replication::driver::{Blocking, Inbox, Ticks, Watch};
use turborel_replication::{Driver, DriverJob, GroupLogs, Joined, Make, ReplicaEvent, ReplicaId, Vr, VrOptions};

use self::net::Security;
pub use self::net::{TcpTransport, members_of};
pub use self::shown::{Links, Replicas};
use crate::settings::{self, LISTEN, MEMBERS, REPLICA, REPLICATION_CA, REPLICATION_TLS, TICK_MS, TLS_CERTIFICATE, TLS_KEY};

/// An error of the node's own (SQLCODE -910).
fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: {why}"))
}

/// \[EXTENSION\] A node prepared: its database given the group's log
/// storage, to be opened to follow, then started.
pub struct Prepared {
    logs: Arc<GroupLogs>,
    transport: Arc<TcpTransport>,
    links: Arc<Links>,
    tick: Duration,
}

/// \[EXTENSION\] A node started: its driver running as its database's
/// job, its transport serving.
pub struct Replication {
    driver: Arc<Driver>,
    transport: Arc<TcpTransport>,
    job: JobHandle,
}

/// The group's log storage, shared with the node that made it.
struct Shared(Arc<GroupLogs>);

impl LogStorage for Shared {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        self.0.open(place)
    }
    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        self.0.exists(place)
    }
    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        self.0.create(place)
    }
}

/// What the driver tells, told as the database's events, and kept for
/// REPLICAS. Called with the driver's lock held: it tells the sink, and
/// calls nothing of the driver's.
struct Telling {
    db: Database,
    links: Arc<Links>,
}

impl Watch for Telling {
    fn committed(&self, _through: Lsn, _log: &[Vec<u8>]) {}

    fn serving(&self, view: u64) {
        self.db.tell(&Event::Replica { change: ReplicaChange::Serving { view } });
    }

    fn stopped(&self) {
        self.db.tell(&Event::Replica { change: ReplicaChange::Stopped });
    }

    fn outcome(&self, _at: Lsn, _record: &[u8], _kept: bool) {}

    fn event(&self, event: &ReplicaEvent) {
        let change = match event {
            ReplicaEvent::ViewChanged { view, primary } => {
                self.links.entered(*view, *primary);
                ReplicaChange::ViewChanged { view: *view, primary: primary.0 }
            }
            ReplicaEvent::FrameRefused { from, why } => ReplicaChange::FrameRefused { from: from.0, why: why.clone() },
            ReplicaEvent::AppendRefused => ReplicaChange::AppendRefused,
        };
        self.db.tell(&Event::Replica { change });
    }
}

/// \[EXTENSION\] Whether `db`'s settings make it a replica:
/// REPLICATION.REPLICA is not 0.
pub fn replicated(db: &Database) -> bool {
    settings::count(db, REPLICA) > 0
}

/// \[EXTENSION\] Whether a replica reads the setting `name` before its
/// database is opened, though it is not read as the database opens: the
/// files it presents to the other members, SERVER.TLS_CERTIFICATE and
/// SERVER.TLS_KEY.
pub fn read_before_open(name: &str) -> bool {
    name.eq_ignore_ascii_case(TLS_CERTIFICATE) || name.eq_ignore_ascii_case(TLS_KEY)
}

/// The TLS of the links, as the settings say: none if REPLICATION.TLS is
/// OFF.
fn security(db: &Database) -> Result<Option<Security>> {
    if settings::text(db, REPLICATION_TLS) == turborel::settings::OFF {
        return Ok(None);
    }
    let (chain, key, ca) = (settings::text(db, TLS_CERTIFICATE), settings::text(db, TLS_KEY), settings::text(db, REPLICATION_CA));
    if chain.is_empty() || key.is_empty() || ca.is_empty() {
        return Err(refused("REPLICATION.TLS is ON, so SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY and REPLICATION.CA name its files"));
    }
    let failed = |e: std::io::Error| SqlError::new(Code::Io, format!("[EXTENSION] RULES.md D57: cannot take TLS: {e}"));
    let (served_chain, served_key) = crate::tls::identity(&chain, &key).map_err(failed)?;
    let (own_chain, own_key) = crate::tls::identity(&chain, &key).map_err(failed)?;
    let wrong = |e: rustls::Error| SqlError::new(Code::Io, format!("[EXTENSION] RULES.md D57: cannot take TLS: {e}"));
    let acceptor =
        TlsAcceptor::from(crate::tls::mutual_config(served_chain, served_key, crate::tls::roots(&ca).map_err(failed)?).map_err(wrong)?);
    let connector =
        TlsConnector::from(crate::tls::presenting_config(crate::tls::roots(&ca).map_err(failed)?, own_chain, own_key).map_err(wrong)?);
    Ok(Some(Security { acceptor, connector }))
}

/// \[EXTENSION\] Prepares `db` as the replica its settings name
/// (`replicated`), before it is opened from its files: its log storage
/// the group's. None if it is not replicated.
///
/// # Errors
///
/// SQLCODE -910 if the settings are wrong: the node no member, a member
/// malformed, TLS's files not named; -915 if TLS's files cannot be
/// taken; `Database::set_log_storage`'s.
pub fn prepare(db: &Database) -> Result<Option<Prepared>> {
    if !replicated(db) {
        return Ok(None);
    }
    let me = ReplicaId(u32::try_from(settings::count(db, REPLICA)).unwrap_or(u32::MAX));
    let members = members_of(&settings::text(db, MEMBERS))?;
    let own =
        members.get(&me).cloned().ok_or_else(|| refused(&format!("REPLICATION.REPLICA {} is no member of REPLICATION.MEMBERS", me.0)))?;
    let listen = Some(settings::text(db, LISTEN)).filter(|l| !l.is_empty()).unwrap_or(own);
    let tick = Duration::from_millis(u64::try_from(settings::count(db, TICK_MS)).unwrap_or(u64::MAX));
    let inbox = Arc::new(Inbox::default());
    let links = Arc::new(Links::new(me, &members));
    let transport =
        Arc::new(TcpTransport::new((me, members.clone()), (listen, tick), security(db)?, (Arc::clone(&inbox), Arc::clone(&links)))?);
    let make: Make = Arc::new(|of, replica, recovered| {
        Ok(Box::new(match recovered {
            None => Vr::new(of, replica, VrOptions::default())?,
            Some(recovered) => Vr::restart(of, replica, VrOptions::default(), recovered)?,
        }))
    });
    let sending: Arc<dyn turborel_replication::Transport> = Arc::<TcpTransport>::clone(&transport);
    let joined = Joined {
        transport: sending,
        inbox,
        ticker: Arc::new(Ticks::every(tick)),
        wait: Arc::new(Blocking { most: tick }),
        watch: Arc::new(Telling { db: db.clone(), links: Arc::clone(&links) }),
    };
    let ids: Vec<ReplicaId> = members.keys().copied().collect();
    let logs = Arc::new(GroupLogs::in_files(FileLogOptions::default(), (&ids, me), make, joined));
    db.set_log_storage(Box::new(Shared(Arc::clone(&logs))))?;
    Ok(Some(Prepared { logs, transport, links, tick }))
}

impl Prepared {
    /// \[EXTENSION\] Starts the node, its database opened to follow and
    /// holding the log through `holds` (`Database::open_following`): the
    /// driver joined to it and run as its job, the transport serving on
    /// `runtime`, REPLICAS registered.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database was not opened; -915 if the transport
    /// cannot listen; the driver's, the job's, the table's.
    pub fn start(self, db: &Database, runtime: &Handle, holds: Lsn) -> Result<Replication> {
        let driver = self.logs.driver().ok_or_else(|| refused("the database was not opened from its files"))?;
        driver.join(holds)?;
        let job = db.add_job(Arc::new(DriverJob::new(Arc::clone(&driver), self.tick)))?;
        self.transport.serve(runtime, db, db.job_asker(job))?;
        db.register_system_table(Box::new(Replicas(Arc::clone(&self.links))))?;
        db.request_job(job, Duration::ZERO);
        Ok(Replication { driver, transport: self.transport, job })
    }
}

impl Replication {
    /// \[EXTENSION\] Its driver.
    pub fn driver(&self) -> &Arc<Driver> {
        &self.driver
    }

    /// \[EXTENSION\] Its job.
    pub fn job(&self) -> JobHandle {
        self.job
    }

    /// \[EXTENSION\] Where its transport listens.
    pub fn listen(&self) -> &str {
        self.transport.listen()
    }

    /// \[EXTENSION\] Its transport.
    pub fn transport(&self) -> &Arc<TcpTransport> {
        &self.transport
    }

    /// \[EXTENSION\] Stops the node, before `db`, its database, closes:
    /// its transport ended, its driver halted, and the database deposed,
    /// so it serves no more and closes as a replica's does, logging
    /// nothing (`Database::close`).
    ///
    /// # Errors
    ///
    /// `Standby::depose`'s.
    pub fn stop(&self, db: &Database) -> Result<()> {
        self.transport.stop();
        self.driver.halt(refused("the node stopped"));
        db.depose()
    }
}
