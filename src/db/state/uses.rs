//! \[DESIGN\] What a snapshot is made ready with as it is published, and
//! the counts of its indexes' searches (INDEX_STATISTICS, RULES.md D522),
//! one map from name to counts kept in step with its indexes, so a search
//! finds its index's counts by the one lookup that finds the index, and
//! takes no lock.

use std::sync::Arc;

use super::State;
use crate::index::Uses;

impl State {
    /// Readies it to be published: what spills is encrypted while anything
    /// is (RULES.md D48), and each index it has, and only those, has its
    /// counts, those of an index an earlier snapshot had kept.
    pub(crate) fn published(&mut self) {
        self.work.set_encodings(self.spill_encodings());
        let indexes = &self.indexes;
        self.index_uses.retain(|name, _| indexes.contains_key(name));
        for name in indexes.keys() {
            if !self.index_uses.contains_key(name) {
                self.index_uses.insert(name.clone(), Arc::default());
            }
        }
    }

    /// The counts of the searches of the index named `name`, if it has it.
    pub(crate) fn uses_of(&self, name: &str) -> Option<Arc<Uses>> {
        self.index_uses.get(name).cloned()
    }
}
