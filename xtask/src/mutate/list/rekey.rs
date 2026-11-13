//! The mutations of rekeying (RULES.md R-TEST-9): the formats set at
//! once told, the log, catalog and pages written in them, and the sweep
//! that writes again each page behind them.

use super::{Mutation, Package};

/// The mutations of rekeying, in the order they run.
pub const REKEY: [Mutation; 8] = [
    Mutation {
        name: "rekey-untold",
        package: Package::Core,
        breaks: "a change of STORAGE.ENCODINGS or STORAGE.CHECKSUM_KEY reaches no file",
        file: "src/db/settings.rs",
        original: "        self.tell_formats()?;",
        replacement: "        self.tell_durability()?;",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &[],
    },
    Mutation {
        name: "rekey-log-segment",
        package: Package::Core,
        breaks: "the log goes on in its segment, in the old formats, after they change",
        file: "src/log/file.rs",
        original: "        if self.rotate_asked.load(Ordering::Acquire) {\n            self.rotate_for_formats();",
        replacement: "        if self.rotate_asked.load(Ordering::Acquire) {\n            drop(());",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &[],
    },
    Mutation {
        name: "rekey-catalog-switch",
        package: Package::Core,
        breaks: "the catalog appends to its file, in the old formats, after they change",
        file: "src/db/catalog_file.rs",
        original: "        if active.records < compact_after && !active.broken && !self.switch.swap(false, Ordering::AcqRel) {",
        replacement: "        if active.records < compact_after && !active.broken {",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &[],
    },
    Mutation {
        name: "rekey-pages-key",
        package: Package::Core,
        breaks: "the data file's pages keep the old key once STORAGE.CHECKSUM_KEY changes",
        file: "src/disk/buffer.rs",
        original: "        if hash.keyed() {\n            pager.set_hash(hash)?;",
        replacement: "        if hash.keyed() {\n            drop(hash);",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &[],
    },
    Mutation {
        name: "rekey-behind-blind",
        package: Package::Core,
        breaks: "the sweep finds no page in other formats, so none is written again",
        file: "src/disk/pager/formats.rs",
        original: "        key.is_some_and(|key| ids != current.encodings || key != current.key)",
        replacement: "        key.is_some_and(|key| ids != current.encodings && key != current.key && false)",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &["disk::pager::formats::tests::pages_behind_the_formats_are_found_and_the_file_settled"],
    },
    Mutation {
        name: "rekey-meta-stale",
        package: Package::Core,
        breaks: "the meta slot stays keyed by the old key once the header is settled, so the file cannot open without it",
        file: "src/disk/pager/sync.rs",
        original: "            if !free.changed() && !self.resettled.swap(false, Ordering::AcqRel) {",
        replacement: "            if !free.changed() {",
        integration: &[],
        unit: &["disk::pager::formats::tests::pages_behind_the_formats_are_found_and_the_file_settled"],
    },
    Mutation {
        name: "rekey-walk-stale",
        package: Package::Core,
        breaks: "the sweep goes on from where it was once the formats change under it, leaving the pages before in the old ones",
        file: "src/db/rekey.rs",
        original: "        if walk.target.as_ref() != Some(current) {",
        replacement: "        if walk.target.is_none() {",
        integration: &["rekeying::the_sweep_runs_in_its_own_time"],
        unit: &[],
    },
    Mutation {
        name: "rekey-settled-untold",
        package: Package::Core,
        breaks: "the files settled are never told",
        file: "src/db/checkpoint.rs",
        original: "            self.tell_if_settled();",
        replacement: "            drop(());",
        integration: &["rekeying::files_follow_the_formats_set_at_once"],
        unit: &[],
    },
];
