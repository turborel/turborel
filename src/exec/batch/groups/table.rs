//! \[DESIGN\] A table of groups being made (RULES.md D31, D38): each group's
//! first row's number, grouping values and set functions, in the order
//! they were made, found by grouping key, counted by the bytes they hold
//! against a limit. A grouping by batch holds its groups in one, and, if
//! its set functions can write their states, the groups partly made of
//! rows past it in another (`groups.rs`). Split from groups.rs (RULES.md
//! R-SIZE-2).

use std::collections::HashMap;

use crate::exec::group::STATE_BYTES;
use crate::functions::Accumulator;
use crate::spill::row_bytes;
use crate::storage::Row;

/// A group: its first row's number, its grouping values and its set
/// functions.
pub(super) type Held = (u64, Row, Vec<Box<dyn Accumulator>>);

/// The bytes `group` of key `key` holds, near enough: its key, its
/// grouping values and its set functions' states.
fn bytes_of(key: &[u8], group: &Held) -> usize {
    let states = group.2.len().saturating_mul(STATE_BYTES);
    key.len().saturating_add(row_bytes(&group.1)).saturating_add(states)
}

/// Groups, found by key, while they hold no more than a limit's bytes.
pub(super) struct Table {
    /// Each group's place in `groups`, by its grouping key.
    found: HashMap<Vec<u8>, usize>,
    /// The groups, in the order they were made.
    pub(super) groups: Vec<Held>,
    bytes: usize,
    limit: usize,
    /// Of each group, one more than its place among the groups a batch
    /// has rows of, or 0 if the batch has none of them yet.
    pub(super) slots: Vec<usize>,
    /// The rows and pieces its groups have taken since it was last empty.
    pub(super) taken: usize,
}

impl Table {
    /// No groups yet, of `limit` bytes.
    pub(super) fn new(limit: usize) -> Table {
        Table { found: HashMap::new(), groups: Vec::new(), bytes: 0, limit, slots: Vec::new(), taken: 0 }
    }

    /// The place of the group of key `key`, if it has one.
    #[inline]
    pub(super) fn get(&self, key: &[u8]) -> Option<usize> {
        self.found.get(key).copied()
    }

    /// Whether its groups hold more than its limit: no group is made then
    /// (so one is always made of an empty table).
    #[inline]
    pub(super) fn full(&self) -> bool {
        self.bytes > self.limit
    }

    /// Makes `group` that of key `key`; its place.
    pub(super) fn add(&mut self, key: &[u8], group: Held) -> usize {
        self.bytes = self.bytes.saturating_add(bytes_of(key, &group));
        self.groups.push(group);
        let at = self.groups.len().saturating_sub(1);
        self.found.insert(key.to_vec(), at);
        at
    }

    /// Its groups, each with its key, taken out, so it is empty again.
    pub(super) fn drained(&mut self) -> Vec<(Vec<u8>, Held)> {
        let mut groups: Vec<Option<Held>> = std::mem::take(&mut self.groups).into_iter().map(Some).collect();
        let keyed = self.found.drain().filter_map(|(key, at)| groups.get_mut(at).and_then(Option::take).map(|g| (key, g))).collect();
        (self.bytes, self.slots, self.taken) = (0, Vec::new(), 0);
        keyed
    }

    /// The bytes its groups hold.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Its groups made at `at` and after, each with its key, taken out;
    /// those before keep their places.
    pub(super) fn split_off(&mut self, at: usize) -> Vec<(Vec<u8>, Held)> {
        let mut later: Vec<Option<Held>> = self.groups.split_off(at).into_iter().map(Some).collect();
        let mut keys: Vec<(usize, Vec<u8>)> = Vec::new();
        self.found.retain(|key, &mut place| {
            let kept = place < at;
            if !kept {
                keys.push((place, key.clone()));
            }
            kept
        });
        keys.sort_unstable_by_key(|k| k.0);
        self.slots.truncate(at);
        let taken: Vec<(Vec<u8>, Held)> = keys
            .into_iter()
            .filter_map(|(place, key)| later.get_mut(place.saturating_sub(at)).and_then(Option::take).map(|g| (key, g)))
            .collect();
        for (key, group) in &taken {
            self.bytes = self.bytes.saturating_sub(bytes_of(key, group));
        }
        taken
    }

    /// Its groups, taken out, in the order they were made.
    pub(super) fn into_groups(self) -> Vec<Held> {
        self.groups
    }
}
