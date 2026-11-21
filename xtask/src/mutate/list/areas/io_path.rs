//! The mutations of the buffer pool's and pager's I/O path (RULES.md
//! R-TEST-9, D591): the pages in use read with no lock, as the free list
//! holds them through allocation, freeing, the free list's record and
//! open; a frame's buffer reused empty.

use super::super::{Mutation, Package};

/// The pager's test of the pages in use.
const IN_USE: &str = "pages::pages_in_use_are_those_the_free_list_holds";

/// Mechanisms of the I/O path whose removal must fail their tests.
pub const IO_PATH: [Mutation; 4] = [
    Mutation {
        name: "io-freed-still-in-use",
        package: Package::Core,
        breaks: "a page freed stays in use, so a read or write of it is not refused",
        file: "src/disk/pager/in_use.rs",
        original: "        free.free(extents)?;\n        self.in_use.mark(extents, false);",
        replacement: "        free.free(extents)?;",
        integration: &[IN_USE, "pages::allocation_is_checked_and_freed_pages_are_zeroed"],
        unit: &[],
    },
    Mutation {
        name: "io-trunk-not-in-use",
        package: Package::Core,
        breaks: "the pages the free list's record takes are not in use, though the list holds them so",
        file: "src/disk/pager/sync.rs",
        original: "            self.in_use.mark(&trunk, true);\n",
        replacement: "",
        integration: &[IN_USE],
        unit: &[],
    },
    Mutation {
        name: "io-opened-none-in-use",
        package: Package::Core,
        breaks: "a file opened has no page in use, so every read of it is refused",
        file: "src/disk/pager.rs",
        original: "        pager.in_use.mark(&[Extent { start: PageId(0), count: meta.pages }], true);\n",
        replacement: "",
        integration: &[IN_USE],
        unit: &[],
    },
    Mutation {
        name: "io-buffer-not-emptied",
        package: Package::Core,
        breaks: "a frame's buffer is reused with what it held, so a new page holds an old one's content",
        file: "src/disk/buffer/pool.rs",
        original: "        buffer.clear();\n        Ok(buffer)",
        replacement: "        Ok(buffer)",
        integration: &[],
        unit: &["disk::buffer::pool::capture::tests::evicted_pages_are_captured_as_they_go"],
    },
];
