//! The mutations of the routine debugger (RULES.md R-TEST-9, D1000 to
//! D1029): the DEBUG privilege held by all and lost from the catalog's
//! bytes.

use super::super::{Mutation, Package};

/// The file of EXECUTE and DEBUG on a routine, granted and revoked.
const PRIVILEGES: &str = "src/db/routines/privileges.rs";

/// Mechanisms whose removal must fail the debugger's requirements.
pub const DEBUGGER: [Mutation; 2] = [
    Mutation {
        name: "debugger-debug-held-by-all",
        package: Package::Core,
        breaks: "one granted no DEBUG grants it on",
        file: PRIVILEGES,
        original: "Privilege::Debug => desc.debug_of(who, owner_of(catalog, &desc.name.auth)),",
        replacement: "Privilege::Debug => { let _ = (who, catalog); (true, true) }",
        integration: &["debug_privileges::debug_is_granted_on_by_its_grant_option"],
        unit: &[],
    },
    Mutation {
        name: "debugger-debug-not-written",
        package: Package::Core,
        breaks: "the DEBUG grants of a routine are lost from the catalog's bytes, so from a checkpoint's snapshot",
        file: "src/catalog/codec/routines.rs",
        original: "    put_usage(out, &routine.debug);",
        replacement: "    put_usage(out, &[]);",
        integration: &["debug_privileges::debug_is_kept_across_reopening"],
        unit: &["catalog::codec::routines::tests::each_kind_of_routine_reads_back_as_written"],
    },
];
