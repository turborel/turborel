//! Mutations of XML within other types and other types within XML: large
//! objects and XML values kept with their rows and recovered, and the
//! mapping of collections and rows to XML (RULES.md R-TEST-9, D79, D86).

use super::{Mutation, Package};

/// Each mechanism broken alone, and the tests that fail.
pub const XML_COMPOSITE: [Mutation; 1] = [Mutation {
    name: "lob-after-definitions",
    package: Package::Core,
    breaks: "recovery makes a transaction's large objects before the table it made",
    file: "src/db/recovery/stale.rs",
    original: "if matches!(kept, Change::LobPut { .. }) && replaying.pending.contains_key(&tx) {",
    replacement: "if matches!(kept, Change::LobPut { .. }) && replaying.pending.is_empty() {",
    integration: &["lob_storage::objects_of_a_table_made_in_their_transaction_are_recovered"],
    unit: &[],
}];
