//! The mutations of the copies a session keeps of what its statements
//! share (RULES.md R-TEST-9, D780, D781): its snapshot of the definitions,
//! taken again only once one is published, and a prepared statement's copy
//! of what its place keeps, copied again only once the place changes.

use super::super::{Mutation, Package};

/// The snapshot's file.
const PUBLISHED: &str = "src/db/database/published.rs";
/// The places' file.
const SHARED: &str = "src/db/prepared/shared.rs";

/// The own copies' mutations, in the order they run.
pub const OWN_COPIES: [Mutation; 4] = [
    Mutation {
        name: "own-snapshot-never-taken-again",
        package: Package::Core,
        breaks: "a session's statements keep its old snapshot after a change of definitions is published",
        file: PUBLISHED,
        original: "            && *taken == count\n",
        replacement: "            && (*taken == count || *taken != count)\n",
        integration: &[],
        unit: &["db::database::tests::a_session_takes_its_own_snapshot_until_one_is_published"],
    },
    Mutation {
        name: "own-snapshot-shared",
        package: Package::Core,
        breaks: "a session's statements take a count of the published snapshot each, a line every session writes",
        file: PUBLISHED,
        original: "        let copy = Arc::new(State::clone(&**read(&self.0.current)?));",
        replacement: "        let copy = Arc::clone(&*read(&self.0.current)?);",
        integration: &[],
        unit: &["db::database::tests::a_session_takes_its_own_snapshot_until_one_is_published"],
    },
    Mutation {
        name: "own-place-copy-shared",
        package: Package::Core,
        breaks: "an execution takes a count of the plan its place keeps, a line every session's executions write",
        file: SHARED,
        original: "Physical::Fixed(Arc::new(KeptPhysical::clone(plan)), Arc::from(&reads[..]), learnt.clone()),",
        replacement: "Physical::Fixed(Arc::clone(plan), Arc::clone(reads), learnt.clone()),",
        integration: &[],
        unit: &["db::prepared::plans::tests::executions_read_their_own_copy_of_what_the_place_keeps"],
    },
    Mutation {
        name: "own-place-copy-never-again",
        package: Package::Core,
        breaks: "a prepared statement never sees the plan another session kept in its place, and plans it again",
        file: SHARED,
        original: "        if own.as_ref().is_none_or(|(at, _)| *at != changes) {",
        replacement: "        if own.is_none() {",
        integration: &[],
        unit: &["db::prepared::plans::tests::executions_read_their_own_copy_of_what_the_place_keeps"],
    },
];
