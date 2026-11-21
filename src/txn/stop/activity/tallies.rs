//! \[DESIGN\] The tally of a session's statements (RULES.md D521): for each
//! normalized text and user, the calls, their time, the most one took, the
//! rows they gave or changed, read and spilled, and the last run. A
//! statement's run is counted as it ends (`Tallied::done`), into its own
//! session's tallies, so no two sessions ever take one lock as they run.
//! At most the STATEMENTS.KEPT the run began under are kept: a new one
//! past them takes the place of the one called least.

use std::sync::{Arc, Mutex};

use super::Activity;
use crate::hash::murmur::{MurmurMap, murmur3_x64_128};
use crate::sync::lock_builtin;
use crate::system::Text;

/// The tally of one statement text, run by one user.
#[derive(Debug, Clone)]
pub struct Tally {
    /// Its text.
    pub text: Arc<Text>,
    /// The user who ran it.
    pub user: Arc<str>,
    /// Its calls.
    pub calls: u64,
    /// Their time, in microseconds.
    pub micros: u64,
    /// The most one took, in microseconds.
    pub most: u64,
    /// The rows they gave, a query's, or changed.
    pub rows: u64,
    /// The rows their scans read.
    pub read: u64,
    /// The bytes their work areas spilled.
    pub spilled: u64,
    /// When the last began, as a timestamp's instant.
    pub last: i64,
}

/// One run of a statement: its time, its rows given or changed, read and
/// spilled, and when it began.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    /// Its time, in microseconds.
    pub micros: u64,
    /// The rows it gave or changed.
    pub rows: u64,
    /// The rows its scans read.
    pub read: u64,
    /// The bytes its work areas spilled.
    pub spilled: u64,
    /// When it began, as a timestamp's instant.
    pub last: i64,
}

impl Tally {
    /// `run` counted as one more call.
    fn ran(&mut self, run: &Run) {
        self.calls = self.calls.saturating_add(1);
        self.micros = self.micros.saturating_add(run.micros);
        self.most = self.most.max(run.micros);
        self.rows = self.rows.saturating_add(run.rows);
        self.read = self.read.saturating_add(run.read);
        self.spilled = self.spilled.saturating_add(run.spilled);
        self.last = self.last.max(run.last);
    }

    /// `other`'s counts added to its own.
    fn merge(&mut self, other: &Tally) {
        self.calls = self.calls.saturating_add(other.calls);
        self.micros = self.micros.saturating_add(other.micros);
        self.most = self.most.max(other.most);
        self.rows = self.rows.saturating_add(other.rows);
        self.read = self.read.saturating_add(other.read);
        self.spilled = self.spilled.saturating_add(other.spilled);
        self.last = self.last.max(other.last);
    }
}

/// The tallies of statements, by the hashes of their normalized text and
/// of their user.
#[derive(Debug, Default)]
pub struct Tallies {
    by: MurmurMap<(u64, u64), Tally>,
}

impl Tallies {
    /// Adds `tally` under `key`, as one more of the same, or as a new one
    /// if there is room among `kept`, or in place of the one called least.
    pub fn add(&mut self, key: (u64, u64), tally: &Tally, kept: usize) {
        if let Some(had) = self.by.get_mut(&key) {
            had.merge(tally);
            return;
        }
        if self.room(kept) {
            self.by.insert(key, tally.clone());
        }
    }

    /// Adds `run` under `key`, as `add` does, its text and user, given by
    /// `named`, taken only for a new one: a run of a statement tallied
    /// before copies nothing shared.
    pub fn ran(&mut self, key: (u64, u64), run: &Run, kept: usize, named: impl FnOnce() -> Option<(Arc<Text>, Arc<str>)>) {
        if let Some(had) = self.by.get_mut(&key) {
            had.ran(run);
            return;
        }
        let Some((text, user)) = named() else { return };
        if self.room(kept) {
            let mut tally = Tally { text, user, calls: 0, micros: 0, most: 0, rows: 0, read: 0, spilled: 0, last: run.last };
            tally.ran(run);
            self.by.insert(key, tally);
        }
    }

    /// Whether a new tally may be added within `kept`, the one called
    /// least taken out to make room if there is none.
    fn room(&mut self, kept: usize) -> bool {
        if kept == 0 {
            return false;
        }
        if self.by.len() >= kept {
            let least = self.by.iter().min_by_key(|(_, t)| (t.calls, t.last)).map(|(k, _)| *k);
            if let Some(least) = least {
                self.by.remove(&least);
            }
        }
        true
    }

    /// Adds each of `other`'s, as `add` does.
    pub fn merge(&mut self, other: &Tallies, kept: usize) {
        for (key, tally) in &other.by {
            self.add(*key, tally, kept);
        }
    }

    /// Forgets them all.
    pub fn clear(&mut self) {
        self.by.clear();
    }

    /// Each, in no order.
    pub fn iter(&self) -> impl Iterator<Item = &Tally> {
        self.by.values()
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.by.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }
}

/// The user a session's statements last ran as, and the hash of their
/// name, kept so a run copies neither.
#[derive(Debug, Default)]
pub(super) struct User(Mutex<Option<(Arc<str>, u64)>>);

impl User {
    /// The hash of the user `name`, kept as the user statements run as.
    fn of(&self, name: &str) -> u64 {
        let mut user = lock_builtin(&self.0);
        if let Some((_, hash)) = user.as_ref().filter(|(had, _)| &**had == name) {
            return *hash;
        }
        let hash = murmur3_x64_128(name.as_bytes(), 0).0;
        *user = Some((Arc::from(name), hash));
        hash
    }

    /// The user kept, if its hash is `hash`.
    fn named(&self, hash: u64) -> Option<Arc<str>> {
        lock_builtin(&self.0).as_ref().filter(|(_, h)| *h == hash).map(|(name, _)| Arc::clone(name))
    }
}

/// A statement's run, to be tallied as it ends: the keys of its text and
/// user, the session's counts as it began, and the tallies kept then. It
/// holds nothing shared, so a run of a statement tallied before takes no
/// count of a text other sessions share.
#[derive(Debug)]
pub struct Tallied {
    key: (u64, u64),
    counts: (u64, u64),
    kept: usize,
    began: i64,
}

impl Tallied {
    /// The run of `text` by `user`, of `activity`'s session, begun at
    /// `began` while `kept` tallies are kept.
    pub fn of(activity: &Activity, text: &Text, (user, kept): (&str, usize), began: i64) -> Tallied {
        Tallied { key: (text.key(), activity.user.of(user)), counts: activity.counts(), kept, began }
    }

    /// The run ended, after `micros` of work, having given or changed
    /// `rows` rows: tallied in `activity`'s session's tallies, a new
    /// tally's text the statement the session notes running.
    pub fn done(self, activity: &Activity, micros: u64, rows: u64) {
        let (read, spilled) = activity.counts();
        let (read, spilled) = (read.saturating_sub(self.counts.0), spilled.saturating_sub(self.counts.1));
        let run = Run { micros, rows, read, spilled, last: self.began };
        let named = || {
            let text = lock_builtin(&activity.statement).as_ref().map(|(t, _)| Arc::clone(t)).filter(|t| t.key() == self.key.0)?;
            Some((text, activity.user.named(self.key.1)?))
        };
        lock_builtin(&activity.tallies).ran(self.key, &run, self.kept, named);
    }
}
