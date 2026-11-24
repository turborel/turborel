//! The mutations of the native protocol's declared types and the session
//! status of the catalog (RULES.md R-TEST-9, D396, D397). A column's
//! declared type is told only to a client granted `declared-types`, read
//! back by the client, written as declared, and shown by `\gdesc` without
//! the statement run; and CATALOG.NAME takes no empty name.

use super::super::{Mutation, Package};

/// The server's tests of declared types over a connection.
const TOLD: &str = "declared::declared_types_are_told_only_to_a_client_that_asks";
const SHOWN: &str = "declared::the_client_asks_for_declared_types_and_shows_them";

/// Mechanisms whose removal must fail the declared types' or the
/// catalog's tests.
pub const DECLARED_TYPES: [Mutation; 6] = [
    Mutation {
        name: "declared-told-ungranted",
        package: Package::Server,
        breaks: "a client not granted declared-types is sent them, its Description's bytes no longer its first form",
        file: "server/src/native/requests.rs",
        original: "    let declared = granted.has(DECLARED_TYPES);\n",
        replacement: "    let declared = !granted.has(\"\");\n",
        integration: &[TOLD],
        unit: &[],
    },
    Mutation {
        name: "declared-never-told",
        package: Package::Server,
        breaks: "a client granted declared-types is told none",
        file: "server/src/native/requests.rs",
        original: "    let declared = granted.has(DECLARED_TYPES);\n",
        replacement: "    let declared = granted.has(\"\");\n",
        integration: &[TOLD, SHOWN],
        unit: &[],
    },
    Mutation {
        name: "declared-tail-dropped",
        package: Package::Server,
        breaks: "a Description's declared types are read past, never given to its columns",
        file: "protocol/src/declared.rs",
        original: "        c.declared = Some(d);\n",
        replacement: "        drop((c, d));\n",
        integration: &[TOLD, SHOWN],
        unit: &[],
    },
    Mutation {
        name: "declared-not-written",
        package: Package::Server,
        breaks: "a column's type is written as its tag says though its declared type was told",
        file: "protocol/src/lib.rs",
        original: "        self.declared.as_ref().map_or_else(|| self.data_type.to_string(), |d| d.written.clone())\n",
        replacement: "        self.declared.as_ref().map_or_else(|| self.data_type.to_string(), |_| self.data_type.to_string())\n",
        integration: &[SHOWN],
        unit: &[],
    },
    Mutation {
        name: "gdesc-runs",
        package: Package::Cli,
        breaks: "\\gdesc runs the statement and shows its rows, not its columns",
        file: "cli/src/run/statement.rs",
        original: "            (true, _) => self.connection.describe_into(sql, &mut printer),\n",
        replacement: "            (true, _) => self.connection.query_into(sql, &mut printer),\n",
        integration: &["scripts::gdesc_shows_declared_types_without_running"],
        unit: &[],
    },
    Mutation {
        name: "catalog-name-empty",
        package: Package::Server,
        breaks: "CATALOG.NAME takes a name of spaces alone, so Status CATALOG is empty",
        file: "src/settings.rs",
        original: ".filter(|s| !s.is_empty() && s.chars().count() <= crate::lexer::MAX_IDENTIFIER_LENGTH);",
        replacement: ".filter(|s| s.chars().count() <= crate::lexer::MAX_IDENTIFIER_LENGTH);",
        integration: &[],
        unit: &["native::status::tests::the_catalog_is_never_null"],
    },
];
