//! \[EXTENSION\] The transport between the replicas of a group (RULES.md
//! D57): frames over TCP, each member's link to each other member a
//! connection it makes and keeps, made again after a failure; secured by
//! mutual TLS, each member presenting the server's certificate and key
//! (SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY) and taking only a peer whose
//! certificate the authorities of REPLICATION.CA verify; or in the clear
//! on one machine (REPLICATION.TLS OFF). A connection begins with a hello
//! naming the member that made it, then carries frames, each its length
//! then its bytes; a listener takes the connections of the other members
//! at REPLICATION.LISTEN, and puts each frame arrived in the driver's
//! inbox, asking for its job (`JobAsker`). A frame is sent without
//! waiting, and may be lost: one that cannot be sent, the link down or
//! its queue full, is dropped, as the contract allows, the consensus
//! sending again what it needs. A link down is made again every few
//! ticks (`RETRY_TICKS`), well within a view timeout.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use turborel::db::Database;
use turborel::diagnostics::{Event, ReplicaChange};
use turborel::error::{Code, Result, SqlError};
use turborel::threads::JobAsker;
use turborel_replication::driver::Inbox;
use turborel_replication::{Configuration, Member, ReplicaId, Transport};

use super::shown::Links;
use crate::listening::addresses;

/// \[DESIGN\] What a connection between replicas begins with: this
/// protocol, its version.
const HELLO: &[u8; 8] = b"TBREPL\0\x01";
/// \[DESIGN\] The longest frame taken: a view change may carry a log.
const MAX_FRAME: usize = 256 * 1024 * 1024;
/// \[DESIGN\] Frames waiting for a link at most: past them, frames are
/// dropped, as a lost frame is sent again by the consensus.
const QUEUE: usize = 4096;
/// \[DESIGN\] The ticks a link down waits before it is made again: well
/// within a view timeout, so a link made again as the group starts is up
/// before the backups give the primary up.
const RETRY_TICKS: u32 = 3;
/// \[DESIGN\] How long a connection has to be made.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// \[DESIGN\] How long the link to a member removed by a reconfiguration
/// is kept: leaving, it is told the epoch started, and asks until it is;
/// then it serves nothing, and its link is let go.
const LEAVING_GRACE: Duration = Duration::from_secs(60);

/// The TLS of the links, if they are secured.
pub(super) struct Security {
    pub(super) acceptor: TlsAcceptor,
    pub(super) connector: TlsConnector,
}

/// \[EXTENSION\] The transport of a replica: see the module's documentation.
pub struct TcpTransport {
    me: ReplicaId,
    listen: String,
    security: Option<Security>,
    /// How long a link down waits before it is made again.
    retry: Duration,
    /// The members linked, which a reconfiguration changes.
    linked: Mutex<Linked>,
    inbox: Arc<Inbox>,
    links: Arc<Links>,
    /// What asks for the driver's job, once it serves.
    asker: OnceLock<JobAsker>,
    /// Once serving: the runtime its tasks run on, and the database told.
    serving: OnceLock<(Handle, Database)>,
    /// The task taking connections.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Itself, weakly, to spawn the tasks keeping links made later
    /// (`remember`).
    myself: OnceLock<Weak<TcpTransport>>,
}

/// The members of the epoch, each at its address, the queue its frames
/// wait in, and, once the transport serves, the task keeping its link;
/// before, the queue's other end, waiting for the task.
#[derive(Default)]
struct Linked {
    members: Vec<ReplicaId>,
    addresses: BTreeMap<ReplicaId, String>,
    queues: BTreeMap<ReplicaId, mpsc::Sender<Vec<u8>>>,
    waiting: BTreeMap<ReplicaId, mpsc::Receiver<Vec<u8>>>,
    keepers: BTreeMap<ReplicaId, JoinHandle<()>>,
    /// Members removed, leaving the group, linked still until
    /// `LEAVING_GRACE` passes from when each was removed.
    leaving: BTreeMap<ReplicaId, Instant>,
}

impl Linked {
    /// Member `member` at `address` added: its queue made, its other end
    /// waiting for a task.
    fn add(&mut self, member: ReplicaId, address: String) {
        let (tx, rx) = mpsc::channel(QUEUE);
        self.queues.insert(member, tx);
        self.waiting.insert(member, rx);
        self.addresses.insert(member, address);
        if !self.members.contains(&member) {
            self.members.push(member);
            self.members.sort_unstable();
        }
    }

    /// Member `member` removed: no member from now on, but linked still,
    /// leaving, for a while.
    fn remove(&mut self, member: ReplicaId) {
        self.members.retain(|&m| m != member);
        self.leaving.insert(member, Instant::now());
    }

    /// The links to members leaving whose grace passed let go: their
    /// queues dropped, their tasks ended.
    fn let_go(&mut self) {
        let gone: Vec<ReplicaId> = self.leaving.iter().filter(|(_, since)| since.elapsed() >= LEAVING_GRACE).map(|(id, _)| *id).collect();
        for member in gone {
            self.leaving.remove(&member);
            self.queues.remove(&member);
            self.waiting.remove(&member);
            self.addresses.remove(&member);
            if let Some(keeper) = self.keepers.remove(&member) {
                keeper.abort();
            }
        }
    }

    /// Whether `member` is linked: a member, or one leaving.
    fn linked(&self, member: ReplicaId) -> bool {
        self.members.contains(&member) || self.leaving.contains_key(&member)
    }
}

/// An error of the transport's.
fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: {why}"))
}

/// The host and port of `address`, as `host:port`.
pub(super) fn host_and_port(address: &str) -> Result<(String, u16)> {
    let (host, port) = address.rsplit_once(':').ok_or_else(|| refused(&format!("{address} is not host:port")))?;
    let port = port.parse::<u16>().map_err(|_| refused(&format!("{address} is not host:port")))?;
    Ok((host.trim().to_owned(), port))
}

/// \[EXTENSION\] The members REPLICATION.MEMBERS names: each its number,
/// `=`, and `host:port`, by commas.
///
/// # Errors
///
/// SQLCODE -910 if an entry is malformed, a number is 0 or named twice.
pub fn members_of(text: &str) -> Result<BTreeMap<ReplicaId, String>> {
    let mut members = BTreeMap::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (number, address) = entry.split_once('=').ok_or_else(|| refused(&format!("{entry} is not NUMBER=host:port")))?;
        let number = number
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| refused(&format!("{entry}: a member's number is 1 or more")))?;
        host_and_port(address)?;
        if members.insert(ReplicaId(number), address.trim().to_owned()).is_some() {
            return Err(refused(&format!("member {number} is named twice")));
        }
    }
    Ok(members)
}

impl TcpTransport {
    /// The transport of member `me` of `members`, listening at `listen`,
    /// secured by `security` if any, its frames arriving in `inbox`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `me` is no member.
    pub(super) fn new(
        (me, members): (ReplicaId, BTreeMap<ReplicaId, String>),
        (listen, tick): (String, Duration),
        security: Option<Security>,
        (inbox, links): (Arc<Inbox>, Arc<Links>),
    ) -> Result<TcpTransport> {
        if !members.contains_key(&me) {
            return Err(refused(&format!("REPLICATION.REPLICA {} is no member of REPLICATION.MEMBERS", me.0)));
        }
        let mut linked = Linked::default();
        for (&member, address) in members.iter().filter(|(m, _)| **m != me) {
            linked.add(member, address.clone());
        }
        linked.members.push(me);
        linked.members.sort_unstable();
        Ok(TcpTransport {
            me,
            listen,
            security,
            retry: tick.saturating_mul(RETRY_TICKS),
            linked: Mutex::new(linked),
            inbox,
            links,
            asker: OnceLock::new(),
            serving: OnceLock::new(),
            tasks: Mutex::new(Vec::new()),
            myself: OnceLock::new(),
        })
    }

    /// \[EXTENSION\] Listens, and makes the links to the other members, on
    /// `runtime`: frames arrived go to the inbox, and `asker` asks for the
    /// driver's job; what happens is told through `db`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot listen.
    pub fn serve(self: &Arc<Self>, runtime: &Handle, db: &Database, asker: JobAsker) -> Result<()> {
        let (host, port) = host_and_port(&self.listen)?;
        let address = addresses(&host, port)?.into_iter().next().ok_or_else(|| refused(&format!("{} names no address", self.listen)))?;
        let listener = runtime
            .block_on(TcpListener::bind(address))
            .map_err(|e| SqlError::new(Code::Io, format!("[EXTENSION] RULES.md D57: cannot listen at {address}: {e}")))?;
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        let accepting = Arc::clone(self);
        let (db_in, asker_in) = (db.clone(), asker.clone());
        drop(self.asker.set(asker));
        drop(self.serving.set((runtime.clone(), db.clone())));
        tasks.push(runtime.spawn(async move { accepting.accept_all(listener, db_in, asker_in).await }));
        drop(tasks);
        self.keep_links();
        Ok(())
    }

    /// Remembers itself, weakly: a reconfiguration spawns the tasks keeping
    /// the links to the members it adds.
    pub(super) fn remember(self: &Arc<Self>) {
        drop(self.myself.set(Arc::downgrade(self)));
    }

    /// Itself, strongly, if remembered and alive.
    fn weak_self(&self) -> Option<Arc<Self>> {
        self.myself.get().and_then(Weak::upgrade)
    }

    /// A task keeping the link to each member whose queue waits for one,
    /// once the transport serves.
    fn keep_links(self: &Arc<Self>) {
        let Some((runtime, db)) = self.serving.get() else { return };
        let mut linked = self.linked.lock().unwrap_or_else(PoisonError::into_inner);
        let waiting = std::mem::take(&mut linked.waiting);
        for (member, queue) in waiting {
            let linking = Arc::clone(self);
            let telling = db.clone();
            linked.keepers.insert(member, runtime.spawn(async move { linking.link(member, queue, telling).await }));
        }
    }

    /// \[EXTENSION\] Stops listening, reading and sending: every task
    /// ended, its connections closed.
    pub fn stop(&self) {
        for task in self.tasks.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
            task.abort();
        }
        for (_, keeper) in std::mem::take(&mut self.linked.lock().unwrap_or_else(PoisonError::into_inner).keepers) {
            keeper.abort();
        }
    }

    /// The address this transport listens at.
    pub fn listen(&self) -> &str {
        &self.listen
    }

    /// The member this transport is.
    pub fn me(&self) -> ReplicaId {
        self.me
    }

    /// Takes the other members' connections, each read on a task of its
    /// own, kept to be ended with the transport, so a member connected to
    /// a node stopped connects again to the node run again; one not taken
    /// is told.
    async fn accept_all(self: Arc<Self>, listener: TcpListener, db: Database, asker: JobAsker) {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                tokio::time::sleep(self.retry).await;
                continue;
            };
            let reading = Arc::clone(&self);
            let (telling, asker) = (db.clone(), asker.clone());
            let task = tokio::spawn(async move {
                if let Err(why) = reading.read_all(stream, &asker).await {
                    telling.tell(&Event::Replica { change: ReplicaChange::ConnectionRefused { peer: peer.to_string(), why } });
                }
            });
            let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            tasks.retain(|t| !t.is_finished());
            tasks.push(task);
        }
    }

    /// Reads a connection: its hello, then its frames into the inbox.
    async fn read_all(&self, stream: TcpStream, asker: &JobAsker) -> std::result::Result<(), String> {
        match &self.security {
            Some(security) => {
                let secured = security.acceptor.accept(stream).await.map_err(|e| format!("TLS refused: {e}"))?;
                self.read_frames(secured, asker).await
            }
            None => self.read_frames(stream, asker).await,
        }
    }

    /// Reads the hello and frames of `stream`.
    async fn read_frames<S: AsyncRead + Unpin>(&self, mut stream: S, asker: &JobAsker) -> std::result::Result<(), String> {
        let mut hello = [0_u8; 12];
        stream.read_exact(&mut hello).await.map_err(|e| format!("no hello: {e}"))?;
        if hello.get(..8) != Some(HELLO.as_slice()) {
            return Err("not a replica's hello".to_owned());
        }
        let from = ReplicaId(u32::from_le_bytes(hello.get(8..12).and_then(|b| b.try_into().ok()).unwrap_or_default()));
        if !self.linked.lock().unwrap_or_else(PoisonError::into_inner).linked(from) || from == self.me {
            return Err(format!("member {} is no other member", from.0));
        }
        loop {
            let frame = read_frame(&mut stream).await.map_err(|e| format!("from member {}: {e}", from.0))?;
            self.links.heard(from);
            self.inbox.push(from, frame);
            asker.ask();
        }
    }

    /// Keeps the link to `member`: a connection made, its frames from
    /// `queue` written, made again after a failure, told as it goes up
    /// and down; a failure to make it told once for each reason, not at
    /// each try.
    async fn link(self: Arc<Self>, member: ReplicaId, mut queue: mpsc::Receiver<Vec<u8>>, db: Database) {
        let address = self.linked.lock().unwrap_or_else(PoisonError::into_inner).addresses.get(&member).cloned().unwrap_or_default();
        let mut told: Option<String> = None;
        loop {
            match self.connect(&address).await {
                Ok(mut stream) => {
                    told = None;
                    self.links.up(member, true);
                    db.tell(&Event::Replica { change: ReplicaChange::Connected { member: member.0 } });
                    let why = write_all(&mut stream, &mut queue).await;
                    self.links.up(member, false);
                    db.tell(&Event::Replica { change: ReplicaChange::Disconnected { member: member.0, why } });
                }
                Err(why) => {
                    if told.as_ref() != Some(&why) {
                        db.tell(&Event::Replica { change: ReplicaChange::Disconnected { member: member.0, why: why.clone() } });
                        told = Some(why);
                    }
                }
            }
            tokio::time::sleep(self.retry).await;
        }
    }

    /// A connection to `address`, secured if the links are, its hello
    /// written.
    async fn connect(&self, address: &str) -> std::result::Result<Box<dyn Stream>, String> {
        let (host, port) = host_and_port(address).map_err(|e| e.to_string())?;
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port))).await;
        let stream =
            connected.map_err(|_| format!("{address}: no answer within {CONNECT_TIMEOUT:?}"))?.map_err(|e| format!("{address}: {e}"))?;
        let mut stream: Box<dyn Stream> = match &self.security {
            Some(security) => {
                let name = ServerName::try_from(host.clone()).map_err(|e| format!("{address}: {e}"))?;
                Box::new(security.connector.connect(name, stream).await.map_err(|e| format!("{address}: TLS failed: {e}"))?)
            }
            None => Box::new(stream),
        };
        let mut hello = HELLO.to_vec();
        hello.extend_from_slice(&self.me.0.to_le_bytes());
        stream.write_all(&hello).await.map_err(|e| format!("{address}: {e}"))?;
        Ok(stream)
    }
}

/// A connection's stream, secured or not.
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Stream for S {}

/// Writes the frames of `queue` to `stream` until one fails: why.
async fn write_all(stream: &mut Box<dyn Stream>, queue: &mut mpsc::Receiver<Vec<u8>>) -> String {
    while let Some(frame) = queue.recv().await {
        if let Err(e) = write_frame(stream, &frame).await {
            return e.to_string();
        }
    }
    "the transport stopped".to_owned()
}

/// Writes `frame`: its length, then its bytes.
async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, frame: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(frame.len()).map_err(|_| std::io::Error::other("a frame too long"))?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(frame).await?;
    stream.flush().await
}

/// Reads a frame: its length, then its bytes.
async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).await?;
    let len = usize::try_from(u32::from_le_bytes(len)).unwrap_or(usize::MAX);
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("a frame of {len} bytes, past {MAX_FRAME}")));
    }
    let mut frame = vec![0; len];
    stream.read_exact(&mut frame).await?;
    Ok(frame)
}

impl Transport for TcpTransport {
    /// Queued for the link to `to`; dropped if the queue is full. To this
    /// replica itself, put in its inbox.
    fn send(&self, to: ReplicaId, frame: &[u8]) -> Result<()> {
        if to == self.me {
            self.inbox.push(to, frame.to_vec());
            if let Some(asker) = self.asker.get() {
                asker.ask();
            }
            return Ok(());
        }
        let mut linked = self.linked.lock().unwrap_or_else(PoisonError::into_inner);
        linked.let_go();
        let queue = linked.queues.get(&to).ok_or_else(|| refused(&format!("replica {} is no member of the group", to.0)))?;
        drop(queue.try_send(frame.to_vec()));
        Ok(())
    }

    fn members(&self) -> Vec<ReplicaId> {
        self.linked.lock().unwrap_or_else(PoisonError::into_inner).members.clone()
    }

    /// The members of `configuration` linked, each new one's link kept
    /// from now on; those no longer named leaving, linked for a while yet
    /// (`LEAVING_GRACE`), as they are told the epoch started and ask
    /// until they are; one named again is a member again.
    fn reconfigure(&self, configuration: &Configuration) -> Result<()> {
        {
            let mut linked = self.linked.lock().unwrap_or_else(PoisonError::into_inner);
            linked.let_go();
            let named: Vec<ReplicaId> = configuration.iter().map(|m| m.id).collect();
            for gone in linked.members.clone().into_iter().filter(|m| !named.contains(m) && *m != self.me) {
                linked.remove(gone);
            }
            let back: Vec<ReplicaId> = named.iter().copied().filter(|m| linked.leaving.contains_key(m)).collect();
            for member in back {
                linked.leaving.remove(&member);
                if !linked.members.contains(&member) {
                    linked.members.push(member);
                }
            }
            linked.members.sort_unstable();
            let added: Vec<&Member> = configuration.iter().filter(|m| m.id != self.me && !linked.addresses.contains_key(&m.id)).collect();
            for member in added {
                host_and_port(&member.address)?;
                linked.add(member.id, member.address.clone());
            }
            if !linked.members.contains(&self.me) {
                linked.members.push(self.me);
                linked.members.sort_unstable();
            }
        }
        if let Some(me) = self.weak_self() {
            me.keep_links();
        }
        Ok(())
    }
}
