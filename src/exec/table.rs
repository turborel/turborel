//! \[DESIGN\] (RULES.md D356, D357) The executor's table of groups, which
//! a grouping by batch (`batch/groups.rs`), DISTINCT as such a grouping,
//! both levels of a DISTINCT set function's two groupings, and each morsel
//! of a grouping run in parallel and each partition of its merge
//! (`parallel/grouped.rs`) hold their groups in. Each group is numbered in
//! the order it was made, so a grouping gives its groups in the order of
//! their first rows whatever the hashes. A group's key is held in lanes of
//! its columns' types (`keys.rs`), its hash beside it (`hash.rs`), its
//! first row's number, and each set function's state in a column of its
//! own (`states.rs`); its slot holds its number (`slots.rs`), so a table
//! that grows places its groups again by the hashes it keeps.
//!
//! A batch's keys are hashed a vector at a time, then each row's group is
//! found or made, then each set function's column is fed the batch's rows
//! in one loop. A table whose probe grows past `slots::MAX_PROBE` hashes
//! its keys by SipHash-1-3 keyed by the database's seed from then on, as
//! keys made to collide under the hash of words would otherwise cost a
//! probe of every group each (RULES.md D357); its groups, their numbers and
//! so the order it gives them in are unchanged.

mod hash;
mod keys;
mod made;
mod slots;
mod states;

pub(crate) use hash::Seed;
pub(in crate::exec) use hash::{hash_rows, hash_values};
pub(in crate::exec) use keys::{Key, Keys};
pub(in crate::exec) use made::Made;

use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken};
use crate::logical::AggCall;
use crate::storage::{Batch, Row};
use slots::{MAX_PROBE, Slots};
pub(in crate::exec) use states::States;

/// The rows of a batch taken by a table's groups, to be fed to their
/// states: each one's place among those given, its place in the batch,
/// and its group.
#[derive(Debug, Default)]
pub(in crate::exec) struct Pending {
    given: Vec<u32>,
    read: Vec<u32>,
    groups: Vec<u32>,
}

/// A place in a batch, or a group's number, as a `u32`: every one fits.
fn narrow(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// \[DESIGN\] What a table keeps of each group beside its key: each set
/// function's states, a column each, of a grouping; nothing, of a table of
/// keys alone, as EXCEPT and INTERSECT count in (`exec/set/tabled.rs`),
/// which goes to another thread as no accumulator can.
pub(in crate::exec) trait Columns {
    /// Makes room for one more group.
    fn grow(&mut self);
    /// Keeps the first `len` groups' room.
    fn truncate(&mut self, len: usize);
    /// The bytes it holds.
    fn bytes(&self) -> usize;
}

impl Columns for Vec<States> {
    fn grow(&mut self) {
        self.iter_mut().for_each(States::grow);
    }

    fn truncate(&mut self, len: usize) {
        for states in self.iter_mut() {
            states.truncate(len);
        }
    }

    fn bytes(&self) -> usize {
        self.iter().map(States::bytes).sum()
    }
}

impl Columns for () {
    fn grow(&mut self) {}

    fn truncate(&mut self, _len: usize) {}

    fn bytes(&self) -> usize {
        0
    }
}

/// A table's groups, found by their keys, each with what `S` keeps of it.
pub(in crate::exec) struct GroupTable<S: Columns = Vec<States>> {
    /// The set functions each group computes.
    aggs: Vec<AggCall>,
    keys: Keys,
    /// Each group's hash (`hash.rs`).
    hashes: Vec<u64>,
    /// Each group's first row's number.
    firsts: Vec<u64>,
    /// Each set function's states, a column each.
    states: S,
    slots: Slots,
    /// Of a table whose probe grew long, each group's hash by SipHash keyed
    /// by the seed, which its slots are placed by from then on.
    keyed: Option<Vec<u64>>,
    seed: Seed,
    /// The rows of the batch being grouped, to be fed to their states.
    pending: Pending,
    /// The bytes of a key, made where a lane cannot compare by its values.
    scratch: Vec<u8>,
    /// The bytes its groups may hold before it is full.
    limit: usize,
    /// The rows and pieces its groups took since it was last empty.
    pub(in crate::exec) taken: usize,
}

impl GroupTable {
    /// No groups yet, of keys of `width` columns, each computing `aggs`,
    /// hashed by `seed`, full past `limit` bytes.
    pub(in crate::exec) fn new(width: usize, aggs: &[AggCall], seed: &Seed, limit: usize) -> GroupTable {
        GroupTable {
            aggs: aggs.to_vec(),
            keys: Keys::new(width),
            hashes: Vec::new(),
            firsts: Vec::new(),
            states: aggs.iter().map(States::of).collect(),
            slots: Slots::of(&[]),
            keyed: None,
            seed: seed.clone(),
            pending: Pending::default(),
            scratch: Vec::new(),
            limit,
            taken: 0,
        }
    }

    /// The set functions each group computes.
    pub(in crate::exec) fn aggs(&self) -> &[AggCall] {
        &self.aggs
    }
}

impl GroupTable<()> {
    /// No groups yet, of keys of `width` columns alone, hashed by `seed`.
    pub(in crate::exec) fn of_keys(width: usize, seed: &Seed) -> GroupTable<()> {
        GroupTable {
            aggs: Vec::new(),
            keys: Keys::new(width),
            hashes: Vec::new(),
            firsts: Vec::new(),
            states: (),
            slots: Slots::of(&[]),
            keyed: None,
            seed: seed.clone(),
            pending: Pending::default(),
            scratch: Vec::new(),
            limit: usize::MAX,
            taken: 0,
        }
    }
}

impl<S: Columns> GroupTable<S> {
    /// How many groups.
    pub(in crate::exec) fn len(&self) -> usize {
        self.hashes.len()
    }

    /// The seed it hashes by.
    pub(in crate::exec) fn seed(&self) -> &Seed {
        &self.seed
    }

    /// How many columns its keys have.
    pub(in crate::exec) fn width(&self) -> usize {
        self.keys.width()
    }

    /// The hash its slots place `key`, of hash `hash`, by: that, or its
    /// keyed hash once its probe grew long.
    fn placed_by(&mut self, hash: u64, key: Key<'_>) -> u64 {
        if self.keyed.is_none() {
            return hash;
        }
        self.scratch.clear();
        key.put(self.keys.width(), &mut self.scratch);
        self.seed.keyed(&self.scratch)
    }

    /// The group of `key`, of hash `hash`, if it has one.
    pub(in crate::exec) fn find(&mut self, hash: u64, key: Key<'_>) -> Option<u32> {
        let placed = self.placed_by(hash, key);
        let Self { slots, keys, scratch, .. } = self;
        let probe = slots.find(placed, |g| keys.equal(crate::storage::mask::index(g), key, scratch));
        if probe.length > MAX_PROBE {
            self.rekeyed();
        }
        probe.found
    }

    /// The group of `key`, of hash `hash`, if it has one, the table left
    /// as it is, so threads find groups in it at once (RULES.md D490); a
    /// key's bytes made in `scratch`.
    pub(in crate::exec) fn found(&self, hash: u64, key: Key<'_>, scratch: &mut Vec<u8>) -> Option<u32> {
        let placed = if self.keyed.is_some() {
            scratch.clear();
            key.put(self.keys.width(), scratch);
            self.seed.keyed(scratch)
        } else {
            hash
        };
        self.slots.find(placed, |g| self.keys.equal(crate::storage::mask::index(g), key, scratch)).found
    }

    /// Makes the group of `key`, of hash `hash`, its first row the
    /// `first`th, fed nothing: its number.
    pub(in crate::exec) fn make(&mut self, hash: u64, key: Key<'_>, first: u64) -> u32 {
        let g = narrow(self.hashes.len());
        let placed = self.placed_by(hash, key);
        self.keys.push(key);
        self.hashes.push(hash);
        self.firsts.push(first);
        self.states.grow();
        let length = match &mut self.keyed {
            Some(keyed) => {
                keyed.push(placed);
                self.slots.place(placed, g, keyed)
            }
            None => self.slots.place(hash, g, &self.hashes),
        };
        if length > MAX_PROBE {
            self.rekeyed();
        }
        g
    }

    /// The group of `key`, of hash `hash`, made if new, its first row the
    /// `first`th.
    pub(in crate::exec) fn found_or_made(&mut self, hash: u64, key: Key<'_>, first: u64) -> u32 {
        match self.find(hash, key) {
            Some(g) => g,
            None => self.make(hash, key, first),
        }
    }

    /// \[DESIGN\] (RULES.md D357) Places its groups by their keys' SipHash,
    /// keyed by the seed, from now on, if they are not yet: keys made to
    /// collide under the hash of words do not under one keyed by a seed
    /// their maker does not know.
    fn rekeyed(&mut self) {
        if self.keyed.is_some() {
            return;
        }
        let mut keyed = Vec::with_capacity(self.hashes.len());
        for g in 0..self.hashes.len() {
            self.scratch.clear();
            Key::Group(&self.keys, g).put(self.keys.width(), &mut self.scratch);
            keyed.push(self.seed.keyed(&self.scratch));
        }
        self.slots.rebuilt(&keyed);
        self.keyed = Some(keyed);
    }

    /// Whether it places its groups by keyed hashes (RULES.md D357).
    #[cfg(test)]
    pub(in crate::exec) fn is_keyed(&self) -> bool {
        self.keyed.is_some()
    }

    /// Group `g`'s first row's number.
    pub(in crate::exec) fn first(&self, g: usize) -> u64 {
        self.firsts.get(g).copied().unwrap_or(u64::MAX)
    }

    /// Group `g`'s grouping values.
    pub(in crate::exec) fn values(&self, g: usize) -> Row {
        self.keys.values(g)
    }

    /// Appends group `g`'s key's bytes to `out`, as `put_key` makes them.
    pub(in crate::exec) fn key_bytes(&self, g: usize, out: &mut Vec<u8>) {
        Key::Group(&self.keys, g).put(self.keys.width(), out);
    }

    /// Whether its groups hold more than its limit's bytes: no group is
    /// made then, so one is always made of an empty table.
    pub(in crate::exec) fn full(&self) -> bool {
        !self.hashes.is_empty() && self.bytes() > self.limit
    }

    /// The bytes it holds: its vectors' room, and what its keys' values and
    /// its accumulators hold.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let words =
            self.hashes.capacity().saturating_add(self.firsts.capacity()).saturating_add(self.keyed.as_ref().map_or(0, Vec::capacity));
        let states = self.states.bytes();
        words.saturating_mul(size_of::<u64>()).saturating_add(self.keys.bytes()).saturating_add(self.slots.bytes()).saturating_add(states)
    }

    /// Keeps its first `len` groups, in their order, placed again, the
    /// room of the others given back.
    pub(in crate::exec) fn truncate(&mut self, len: usize) {
        self.keys.truncate(len);
        self.hashes.truncate(len);
        self.hashes.shrink_to_fit();
        self.firsts.truncate(len);
        self.firsts.shrink_to_fit();
        self.states.truncate(len);
        match &mut self.keyed {
            Some(keyed) => {
                keyed.truncate(len);
                keyed.shrink_to_fit();
                self.slots.rebuilt(keyed);
            }
            None => self.slots.rebuilt(&self.hashes),
        }
        if len == 0 {
            self.taken = 0;
        }
    }
}

impl GroupTable {
    /// Takes the `n`th row given, at `r` of its batch, into group `g`, to
    /// be fed to its states.
    pub(in crate::exec) fn take(&mut self, g: u32, (n, r): (usize, usize)) {
        self.taken = self.taken.saturating_add(1);
        if !self.aggs.is_empty() {
            self.pending.given.push(narrow(n));
            self.pending.read.push(narrow(r));
            self.pending.groups.push(g);
        }
    }

    /// Counts a row or piece taken by a group, fed nothing.
    pub(in crate::exec) fn took(&mut self) {
        self.taken = self.taken.saturating_add(1);
    }

    /// Feeds the rows taken of `batch` to their groups' states, as `taken`
    /// says, of `lanes` where evaluated by vectors, an expression's row
    /// made in `scratch`, warnings heard by `runner`; and forgets them.
    ///
    /// # Errors
    ///
    /// As a set function, or an expression it takes, fails.
    pub(in crate::exec) fn feed(
        &mut self,
        batch: &Batch,
        (taken, lanes): (&[Taken], &[Option<Fed<'_>>]),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        if self.pending.groups.is_empty() {
            return Ok(());
        }
        for (((states, agg), take), lane) in self.states.iter_mut().zip(&self.aggs).zip(taken).zip(lanes) {
            states.feed((&self.pending, agg), (batch, take, lane), (scratch, runner))?;
        }
        self.pending.given.clear();
        self.pending.read.clear();
        self.pending.groups.clear();
        Ok(())
    }

    /// Merges into group `g` the states `written`, each as its accumulator
    /// writes it, of a piece whose first row is the `first`th.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(in crate::exec) fn merge_written(&mut self, g: u32, (first, written): (u64, &[Vec<u8>])) -> Result<()> {
        let g = crate::storage::mask::index(g);
        if let Some(held) = self.firsts.get_mut(g) {
            *held = (*held).min(first);
        }
        for ((states, agg), state) in self.states.iter_mut().zip(&self.aggs).zip(written) {
            states.merge_written((g, agg), state)?;
        }
        Ok(())
    }

    /// Appends to `out` group `g`'s state of the `a`th set function, as
    /// its accumulator writes it: whether it has one.
    pub(in crate::exec) fn write_state(&self, (g, a): (usize, usize), out: &mut Vec<u8>) -> bool {
        self.states.get(a).is_some_and(|states| states.write(g, out))
    }

    /// How many set functions.
    pub(in crate::exec) fn functions(&self) -> usize {
        self.states.len()
    }

    /// Its groups, each its first row's number and its row, its grouping
    /// values then each set function's result, in the order made.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`.
    pub(in crate::exec) fn finished(&self) -> Result<Vec<(u64, Row)>> {
        (0..self.len())
            .map(|g| {
                let mut row = self.keys.values(g);
                row.reserve(self.states.len());
                for states in &self.states {
                    row.push(states.finish(g)?);
                }
                Ok((self.first(g), row))
            })
            .collect()
    }
}

// Unit tests: groups found as their keys' bytes are equal, in the order
// made, past growth and the keyed hash (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
