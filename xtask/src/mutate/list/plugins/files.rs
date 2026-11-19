//! The mutations of packages as files (RULES.md R-TEST-9, D260 to D268):
//! a license's grammar, its file packed, the lock's license lines, the
//! files given to install resolved among, a lock's digests checked, and
//! a starter that writes over nothing.

use super::super::{Mutation, Package};

/// The mutations of package files, the starter and licenses.
pub const FILES: [Mutation; 6] = [
    Mutation {
        name: "spdx-unclosed-paren-accepted",
        package: Package::Core,
        breaks: "a license expression with a parenthesis left open is read",
        file: "src/plugin/package/license.rs",
        original: "            if self.peek() != Some(&Token::Close) {\n",
        replacement: "            if false {\n",
        integration: &[],
        unit: &["plugin::package::license::tests::license_expressions_follow_the_spdx_grammar"],
    },
    Mutation {
        name: "pack-license-file-left-out",
        package: Package::Core,
        breaks: "the LICENSE FILE is not packed, so neither its digest nor the signature covers its text",
        file: "src/plugin/package/project.rs",
        original: "        files.push((named(license), EntryKind::Doc, present(project.dir, &named(license), \"LICENSE FILE\")?));\n",
        replacement: "        present(project.dir, &named(license), \"LICENSE FILE\")?;\n",
        integration: &[],
        unit: &["plugin::package::tests::licenses_are_read_packed_and_shown"],
    },
    Mutation {
        name: "lock-license-lost",
        package: Package::Core,
        breaks: "a lock forgets the license expression of each package",
        file: "src/plugin/package/lock.rs",
        original: "                let _ = writeln!(text, \"{LICENSE}{expression}\");\n",
        replacement: "                let _ = expression;\n",
        integration: &[],
        unit: &["plugin::package::tests::a_lock_round_trips"],
    },
    Mutation {
        name: "install-given-files-ignored",
        package: Package::Cli,
        breaks: "install chooses only among the packages installed, never the files given",
        file: "cli/src/extension/commands.rs",
        original: "        let mut all = self.given.versions(name)?;\n",
        replacement: "        let mut all = self.given.versions(\"\")?;\n",
        integration: &["extensions::install_resolves_among_the_files_given_and_installed"],
        unit: &[],
    },
    Mutation {
        name: "install-lock-digest-unchecked",
        package: Package::Cli,
        breaks: "install --lock takes a package file whose digest is not the lock's",
        file: "cli/src/extension/commands.rs",
        original: "if let Some(g) = file.filter(|g| g.digest != p.digest) {",
        replacement: "if let Some(g) = file.filter(|g| g.digest != g.digest) {",
        integration: &["extensions::the_lock_reproduces_a_set_from_files"],
        unit: &[],
    },
    Mutation {
        name: "starter-writes-over-a-directory",
        package: Package::Cli,
        breaks: "new writes its project into a directory that is there",
        file: "cli/src/extension/starter.rs",
        original: "    if at.exists() {\n",
        replacement: "    if at.is_file() {\n",
        integration: &["extensions::starter::new_refuses_what_it_cannot_make"],
        unit: &[],
    },
];
