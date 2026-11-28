//! The mutations of a table's text key lanes held in chunks (RULES.md
//! R-TEST-9, D1255): a chunk never grown, the room a chunk is made with
//! and counted at, a text longer than a chunk alone in one, groups put in
//! another order with their texts where they are, texts given as one
//! buffer in their groups' order, and texts read back from a spill each
//! from its own place in the run written, and a null taken from another
//! lane told.

use super::super::{Mutation, Package};

/// A text stays where it was written.
const NEVER_MOVES: &str = "exec::table::keys::texts::tests::a_text_written_never_moves";

/// The room of each chunk, and a text longer than a chunk.
const ROOM: &str = "exec::table::keys::texts::tests::a_chunk_has_room_for_what_the_lane_holds_within_its_bounds";

/// Text keys through every way their tables go.
const PATHS: &str = "exec::table::keys::texts::tests::text_keys_hold_their_groups_made_merged_spilled_and_read_back";

/// Mechanisms whose removal must fail the text lanes' requirements.
pub const TEXT_LANES: [Mutation; 8] = [
    Mutation {
        name: "text-chunk-grown-where-it-is",
        package: Package::Core,
        breaks: "a text is appended to the open chunk past its room, so the chunk grows and the texts it holds move (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "chunk.capacity().saturating_sub(chunk.len()) >= text.len());",
        replacement: "chunk.capacity() >= chunk.len());",
        integration: &[],
        unit: &[NEVER_MOVES],
    },
    Mutation {
        name: "text-chunk-room-never-more",
        package: Package::Core,
        breaks: "every chunk is made with the first chunk's room, so a lane of many texts makes a chunk a page (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "self.held.clamp(FIRST, MOST).max(len) };",
        replacement: "FIRST.max(len) };",
        integration: &[],
        unit: &[ROOM],
    },
    Mutation {
        name: "text-chunk-long-text-opens",
        package: Package::Core,
        breaks: "a text longer than the most a chunk holds opens the chunk it is given, so the chunk open before it is left part filled (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "        if len <= MOST {",
        replacement: "        if len <= usize::MAX {",
        integration: &[],
        unit: &[ROOM],
    },
    Mutation {
        name: "text-chunk-room-not-counted",
        package: Package::Core,
        breaks: "a chunk's room is not counted when the chunk is made, so a lane's texts are held outside what its grant is charged (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "        self.room = self.room.saturating_add(room);",
        replacement: "",
        integration: &[],
        unit: &["exec::table::keys::texts::tests::the_room_of_a_chunk_is_counted_when_it_is_made"],
    },
    Mutation {
        name: "text-lane-gathered-in-its-own-order",
        package: Package::Core,
        breaks: "a lane of texts put in a partition's order keeps its groups in their own, so each group is given another's text (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "places.iter().map(|&i| self.spans.get(index(i)).copied().unwrap_or_default()).collect();",
        replacement: "places.iter().enumerate().map(|(i, _)| self.spans.get(i).copied().unwrap_or_default()).collect();",
        integration: &[],
        unit: &["exec::table::keys::texts::tests::groups_gathered_keep_their_texts_where_they_are", PATHS],
    },
    Mutation {
        name: "text-lane-joined-as-written",
        package: Package::Core,
        breaks: "texts in one chunk are given as the chunk whatever order their groups are in, so a column gives a group another's text (RULES.md D1255)",
        file: "src/exec/table/keys/texts.rs",
        original: "if self.buffers.len() == 1 && self.spans.iter().try_fold(0_u32, follow).is_some() {",
        replacement: "if self.buffers.len() == 1 {",
        integration: &[],
        unit: &["exec::table::keys::texts::tests::texts_are_joined_in_one_buffer_in_their_groups_order"],
    },
    Mutation {
        name: "text-lane-read-from-the-runs-start",
        package: Package::Core,
        breaks: "each text of a lane read from a spill is read from the start of the run written, so every group holds the first text's octets (RULES.md D1255)",
        file: "src/exec/table/keys/written.rs",
        original: "        from = to;",
        replacement: "        from = 0;",
        integration: &[],
        unit: &[PATHS, "exec::table::keys::written::tests::every_lane_is_read_back_as_written"],
    },
    Mutation {
        name: "text-lane-null-not-told",
        package: Package::Core,
        breaks: "a lane of texts that takes a null from another lane does not tell that a group's may be null, so the null group reads as the empty text in its place (RULES.md D1255, D1274)",
        file: "src/exec/table/keys.rs",
        original: "            self.nulled |= other.nulled;",
        replacement: "",
        integration: &[],
        unit: &["exec::table::keys::texts::tests::lanes_of_texts_are_appended_and_gathered_as_their_groups_were", PATHS],
    },
];
