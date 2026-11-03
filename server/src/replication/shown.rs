//! \[EXTENSION\] What a replica shows of its group (RULES.md D57):
//! INFORMATION_SCHEMA.REPLICAS, a row a member, to the database's owner:
//! its number and address, whether it is this node, its role in the view
//! this node knows, the view's number, whether this node's link to it is up, and
//! how long ago a frame from it arrived. The transport and the driver's
//! watch keep what it shows (`Links`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use turborel::catalog::ColumnDesc;
use turborel::error::Result;
use turborel::storage::Row;
use turborel::system::{Seen, SystemTable};
use turborel::value::Value;
use turborel_replication::{Member, ReplicaId};

use crate::shown::{count, counted, counted_or_null, text};

/// \[DESIGN\] The longest address shown.
const ADDRESS_LENGTH: u32 = 1024;

/// A member's link, as this node sees it.
#[derive(Default, Clone, Copy)]
struct Link {
    /// Whether this node's connection to it is up.
    up: bool,
    /// When a frame from it last arrived.
    heard: Option<Instant>,
}

/// What a replica knows of its group: its members and links, the epoch
/// and the view.
pub struct Links {
    me: ReplicaId,
    /// The members of the epoch, each at its address.
    members: Mutex<Vec<(ReplicaId, String)>>,
    known: Mutex<BTreeMap<ReplicaId, Link>>,
    /// The epoch this node is in.
    epoch: Mutex<u64>,
    /// The view this node knows, and its primary.
    view: Mutex<Option<(u64, ReplicaId)>>,
}

impl Links {
    /// The links of member `me` to `members`, each at its address.
    pub(super) fn new(me: ReplicaId, members: &BTreeMap<ReplicaId, String>) -> Links {
        let members = members.iter().map(|(&id, address)| (id, address.clone())).collect();
        Links { me, members: Mutex::new(members), known: Mutex::new(BTreeMap::new()), epoch: Mutex::new(0), view: Mutex::new(None) }
    }

    /// This node is in `epoch`, whose members are `configuration`.
    pub(super) fn reconfigured(&self, epoch: u64, configuration: &[Member]) {
        *self.members.lock().unwrap_or_else(PoisonError::into_inner) = configuration.iter().map(|m| (m.id, m.address.clone())).collect();
        *self.epoch.lock().unwrap_or_else(PoisonError::into_inner) = epoch;
    }

    /// The members of the epoch, each at its address.
    pub fn members(&self) -> Vec<(ReplicaId, String)> {
        self.members.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The link to `member` is up, or down.
    pub(super) fn up(&self, member: ReplicaId, up: bool) {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).entry(member).or_default().up = up;
    }

    /// A frame from `member` arrived.
    pub(super) fn heard(&self, member: ReplicaId) {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).entry(member).or_default().heard = Some(Instant::now());
    }

    /// This node entered `view`, whose primary is `primary`.
    pub(super) fn entered(&self, view: u64, primary: ReplicaId) {
        *self.view.lock().unwrap_or_else(PoisonError::into_inner) = Some((view, primary));
    }

    /// The view this node knows, if any, and its primary.
    pub fn view(&self) -> Option<(u64, ReplicaId)> {
        *self.view.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.REPLICAS.
pub struct Replicas(pub(super) Arc<Links>);

impl SystemTable for Replicas {
    fn name(&self) -> &'static str {
        "REPLICAS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![
            counted("MEMBER"),
            text("ADDRESS", ADDRESS_LENGTH, true),
            text("IS_SELF", 3, true),
            text("ROLE", 7, true),
            counted("EPOCH"),
            counted_or_null("VIEW_NUMBER"),
            text("IS_CONNECTED", 3, true),
            counted_or_null("LAST_HEARD_MS"),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let yes = |is: bool| Value::Char(if is { "YES" } else { "NO" }.to_owned());
        let shown = |s: &str| Value::Char(s.to_owned());
        let links = self.0.known.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let view = self.0.view();
        let epoch = *self.0.epoch.lock().unwrap_or_else(PoisonError::into_inner);
        let members = self.0.members();
        let rows = members.iter().map(|(member, address)| {
            let link = links.get(member).copied().unwrap_or_default();
            let me = *member == self.0.me;
            let role = match view {
                Some((_, primary)) if primary == *member => "PRIMARY",
                Some(_) => "BACKUP",
                None => "UNKNOWN",
            };
            vec![
                count(u64::from(member.0)),
                shown(address),
                yes(me),
                shown(role),
                count(epoch),
                view.map_or(Value::Null, |(v, _)| count(v)),
                yes(me || link.up),
                link.heard.map_or(Value::Null, |at| count(u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX))),
            ]
        });
        Ok(rows.collect())
    }
}
