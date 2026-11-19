//! The example's host objects of replication (RULES.md D271, D272): the
//! transport LOOP, which gives back each frame sent as arrived from the
//! replica it was sent to, and the journal KEPT, whose records are written
//! to `journal.log` in the working directory and made durable at SYNC.
//! A frame HALF makes LOOP write part of a message and end, as a process
//! killed mid-message; a record CRASH makes KEPT write part of an entry
//! and end, as a process killed mid-append, its record never named.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use turborel_plugin::{Answer, Held, Journal, Member, Plugin, PluginError, ReplicaTransport};

/// The plug-in with them.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.replica_transport("LOOP", Loop::default()).journal("KEPT", Kept::default())
}

/// The program ended as a process killed mid-write: `partial` written to
/// `out`, and no more.
pub fn killed(out: &mut dyn Write, partial: &[u8]) -> ! {
    let _written = out.write_all(partial).and_then(|()| out.flush());
    std::process::abort()
}

/// LOOP: the members linked, and the frames to give back.
#[derive(Default)]
struct Loop {
    members: Vec<u32>,
    arrived: Vec<(u32, Vec<u8>)>,
}

impl ReplicaTransport for Loop {
    fn open(&mut self, _me: u32, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.members = configuration.iter().map(|m| m.0).collect();
        Ok(self.members.clone())
    }
    fn send(&mut self, to: u32, frame: &[u8]) -> Answer<bool> {
        if frame == b"HALF" {
            // A frame's header claiming a whole message, and part of one.
            killed(&mut std::io::stdout(), &[0, 0, 1, 0, 2, 9])
        }
        let linked = self.members.contains(&to);
        if linked {
            self.arrived.push((to, frame.to_vec()));
        }
        Ok(linked)
    }
    fn receive(&mut self) -> Answer<Vec<(u32, Vec<u8>)>> {
        Ok(std::mem::take(&mut self.arrived))
    }
    fn reconfigure(&mut self, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.open(0, configuration)
    }
}

/// KEPT: the records persisted and not yet synced.
#[derive(Default)]
struct Kept {
    pending: Vec<(u64, Vec<u8>)>,
}

/// Where KEPT keeps its records.
const LOG: &str = "journal.log";

/// An I/O failure, as an ERROR.
fn failed(e: &std::io::Error) -> PluginError {
    PluginError::external(&format!("journal.log: {e}"))
}

/// An entry of `journal.log`: its position, length and record.
fn entry(at: u64, record: &[u8]) -> Vec<u8> {
    let mut out = at.to_be_bytes().to_vec();
    out.extend_from_slice(&u32::try_from(record.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(record);
    out
}

/// The whole entries of `bytes`, a torn last one left out.
fn entries(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut rest = bytes;
    while let Some((head, tail)) = rest.split_at_checked(12) {
        let length = head.get(8..).and_then(|l| <[u8; 4]>::try_from(l).ok()).map_or(0, u32::from_be_bytes);
        let Some((record, after)) = tail.split_at_checked(usize::try_from(length).unwrap_or(usize::MAX)) else { break };
        records.push(record.to_vec());
        rest = after;
    }
    records
}

impl Journal for Kept {
    fn open(&mut self) -> Answer<Held> {
        let mut bytes = Vec::new();
        if let Ok(mut file) = File::open(LOG) {
            file.read_to_end(&mut bytes).map_err(|e| failed(&e))?;
        }
        Ok(Held { durable: true, records: entries(&bytes), ..Held::default() })
    }
    fn persist(&mut self, at: u64, record: &[u8]) -> Answer<()> {
        if record == b"CRASH" {
            let mut file = OpenOptions::new().create(true).append(true).open(LOG).map_err(|e| failed(&e))?;
            let torn = entry(at, record);
            killed(&mut file, torn.get(..torn.len().div_euclid(2)).unwrap_or_default())
        }
        self.pending.push((at, record.to_vec()));
        Ok(())
    }
    fn truncate(&mut self, after: u64) -> Answer<()> {
        self.pending.retain(|(at, _)| *at <= after);
        Ok(())
    }
    fn retain(&mut self, _through: u64) -> Answer<()> {
        Ok(())
    }
    fn mark(&mut self, _at: u64, _epoch: u64, _members: &[Member]) -> Answer<()> {
        Ok(())
    }
    fn persist_state(&mut self, _state: &[u8]) -> Answer<()> {
        Ok(())
    }
    fn sync(&mut self) -> Answer<Vec<u64>> {
        let mut file = OpenOptions::new().create(true).append(true).open(LOG).map_err(|e| failed(&e))?;
        let pending = std::mem::take(&mut self.pending);
        for (at, record) in &pending {
            file.write_all(&entry(*at, record)).map_err(|e| failed(&e))?;
        }
        file.sync_all().map_err(|e| failed(&e))?;
        Ok(pending.into_iter().map(|(at, _)| at).collect())
    }
    fn flush(&mut self) -> Answer<Vec<u64>> {
        self.sync()
    }
}
