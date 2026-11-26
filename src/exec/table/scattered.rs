//! \[DESIGN\] (RULES.md D1119) A row made a group of its own, no group
//! looked for. A morsel whose keys are nearly all apart, as orders grouped
//! by customer, found nearly every row's group missing and made it, so its
//! table compared each key and placed each group to reduce next to
//! nothing, and the partitions found every group again. Such a morsel's
//! rows past its first batch are each made a group, its key, hash and
//! first row pushed, no slot placed, and the table marked as one that may
//! hold a key more than once. It is then only split among the partitions
//! (`made.rs`), each of which finds or makes its groups one at a time.

use super::{Columns, GroupTable, Key, narrow};

impl GroupTable {
    /// A group of one row made of `key`, of hash `hash`, its first row the
    /// `first`th, though one of that key may be held: its number. Its
    /// slots no longer hold every group, so no group is looked for in it
    /// after, and it is only made into a Made.
    pub(in crate::exec) fn scattered(&mut self, hash: u64, key: Key<'_>, first: u64) -> u32 {
        let g = narrow(self.hashes.len());
        self.keys.push(key);
        self.hashes.push(hash);
        self.firsts.push(first);
        self.states.grow();
        self.repeats = true;
        g
    }
}
