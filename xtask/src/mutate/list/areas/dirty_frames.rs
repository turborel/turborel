//! The mutations of the buffer pool's list of frames made dirty (RULES.md
//! R-TEST-9, D30): a frame made dirty never listed, a frame held for
//! writing marked dirty only when it was dirty already, and a flush that
//! leaves the frames it took marked listed, so a page written after it is
//! never listed again.

use super::super::{Mutation, Package};

/// The file of the list.
const DIRTY_FILE: &str = "src/disk/buffer/pool/dirty.rs";
/// The unit test of the frames a flush visits.
const VISITS: &str = "disk::buffer::tests::a_flush_visits_only_the_frames_made_dirty";
/// The unit test of pages written while a flush runs.
const NEVER_LOST: &str = "disk::buffer::tests::pages_written_during_a_flush_are_never_lost";

/// Mechanisms of the dirty list whose removal must fail its tests.
pub const DIRTY_FRAMES: [Mutation; 3] = [
    Mutation {
        name: "dirty-frame-unlisted",
        package: Package::Core,
        breaks: "a frame made dirty is never listed, so no flush writes it back",
        file: DIRTY_FILE,
        original: "            self.dirtied.push(frame.index);\n        }\n    }",
        replacement: "        }\n    }",
        integration: &[],
        unit: &[VISITS, NEVER_LOST],
    },
    Mutation {
        name: "dirty-latched-inverted",
        package: Package::Core,
        breaks: "a page held for writing is marked dirty only if it was dirty already",
        file: DIRTY_FILE,
        original: "if !frame.dirty.load(Ordering::SeqCst) {\n            self.made_dirty(frame);",
        replacement: "if frame.dirty.load(Ordering::SeqCst) {\n            self.made_dirty(frame);",
        integration: &[],
        unit: &[VISITS, NEVER_LOST],
    },
    Mutation {
        name: "dirty-flush-keeps-mark",
        package: Package::Core,
        breaks: "a flush leaves the frames it took marked listed, so pages written after it are never listed again",
        file: DIRTY_FILE,
        original: "            frame.listed.store(false, Ordering::SeqCst);\n",
        replacement: "",
        integration: &[],
        unit: &[VISITS, NEVER_LOST],
    },
];
