//! The mutations of a checkpoint's capture, copy on write (RULES.md
//! R-TEST-9, D590): a page owed copied by its writer, by a free and by an
//! eviction; only pages that need no other page owed; the file covering
//! the pages owed; pages allocated known to have no overflow.

use super::super::{Mutation, Package};

/// The capture's unit tests.
const HOLDS: &str = "disk::buffer::pool::capture::tests::a_capture_holds_pages_as_they_were";
const EVICTED: &str = "disk::buffer::pool::capture::tests::evicted_pages_are_captured_as_they_go";

/// Mechanisms of a checkpoint's capture whose removal must fail their tests.
pub const CHECKPOINT: [Mutation; 6] = [
    Mutation {
        name: "ckpt-writer-copies-nothing",
        package: Package::Core,
        breaks: "a writer changes a page owed without copying it, so the walk captures the page as changed since",
        file: "src/disk/buffer/pool.rs",
        original: "        if frame.owed.load(Ordering::Acquire) {\n            self.owed_copy(frame, &content);",
        replacement: "        if frame.owed.load(Ordering::Acquire) && false {\n            self.owed_copy(frame, &content);",
        integration: &[],
        unit: &[HOLDS],
    },
    Mutation {
        name: "ckpt-free-copies-nothing",
        package: Package::Core,
        breaks: "a page owed is freed without a copy, so the batch lacks it",
        file: "src/disk/buffer/pool.rs",
        original: "            if frame.owed.load(Ordering::Acquire) {\n                self.owed_copy(frame, &content);",
        replacement: "            if frame.owed.load(Ordering::Acquire) && false {\n                self.owed_copy(frame, &content);",
        integration: &[],
        unit: &[HOLDS],
    },
    Mutation {
        name: "ckpt-eviction-captures-nothing",
        package: Package::Core,
        breaks: "an eviction drops a page owed uncaptured, so the batch lacks it",
        file: "src/disk/buffer/pool/capture.rs",
        original: "            Some((_, capturing)) => self.pager(key.file).and_then(|pager| pager.captured(PageId(key.id), content, &capturing)),",
        replacement: "            Some(_) => Ok(()),",
        integration: &[],
        unit: &[EVICTED],
    },
    Mutation {
        name: "ckpt-overflow-owed",
        package: Package::Core,
        breaks: "a page past its page is owed, so its capture, which allocates nothing, fails",
        file: "src/disk/pager/captured.rs",
        original: "        len <= capturing.capacity && self.known",
        replacement: "        len <= usize::MAX && self.known",
        integration: &[],
        unit: &[HOLDS],
    },
    Mutation {
        name: "ckpt-file-not-covered",
        package: Package::Core,
        breaks: "the file is grown to the first page owed, not the last, so a new page past its end cannot be captured",
        file: "src/disk/buffer/pool/capture.rs",
        original: "        let last = capture.pages.iter().map(|(_, key)| key.id).max();",
        replacement: "        let last = capture.pages.iter().map(|(_, key)| key.id).min();",
        integration: &[],
        unit: &[HOLDS],
    },
    Mutation {
        name: "ckpt-allocated-unknown",
        package: Package::Core,
        breaks: "a page allocated is not known to have no overflow, so a new page is written with writers held",
        file: "src/disk/pager.rs",
        original: ".for_each(|id| self.known.set(id, &[]));",
        replacement: ".for_each(|id| self.known.forget(id));",
        integration: &[],
        unit: &[HOLDS],
    },
];
