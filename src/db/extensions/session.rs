//! \[EXTENSION\] What a session does of CREATE and ALTER EXTENSION before
//! the statement is made (RULES.md D168, D171, D173):
//!
//! * Only the database's owner, or anyone of a database with none, loads
//!   an extension, as a program of its runs with the server's rights.
//! * FROM names a package file, installed into the first of
//!   PLUGINS.DIRECTORIES, or a directory under one of them; without FROM
//!   the extension is found by name among the packages installed there,
//!   at the newest version VERSION takes, and so are the extensions it
//!   requires, created first, AS DEPENDENCY.
//! * Its directory is checked (`Host::verify`), its manifest read, and the
//!   text logged is the statement written with what was read, so recovery
//!   and a replica make it of the text alone.
//! * ALTER EXTENSION ... UPDATE runs, after it, the scripts of SQL its
//!   new manifest names for the version it had, in the transaction.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::super::{Completion, Session, State, StatementContext};
use super::refused;
use crate::ast::{ExtensionDefinition, ExtensionStatement, SchemaStatement, SqlStatement};
use crate::error::{At, Code, Result, SqlError};
use crate::lexer::{Tok, delim, lex};
use crate::log::record::DropTarget;
use crate::params::Params;
use crate::parser::Parser;
use crate::plugin::package::semver::{Range, Version};
use crate::plugin::package::{hex, install_file, resolve_installed, upgrade_scripts};
use crate::plugin::{MANIFEST_FILE, Manifest};

/// What a package file's name ends with.
const PACKAGE_SUFFIX: &str = ".tbpkg";

/// One extension to create or update, as found.
struct Found {
    /// Its name.
    name: String,
    /// Its directory, absolute.
    directory: PathBuf,
    /// Created only as another required it.
    dependency: bool,
}

/// `text` as a character string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl Session {
    /// CREATE or ALTER EXTENSION `e`, in `cx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the session may not load extensions; -103 if the
    /// statement writes MANIFEST; SQLSTATE 38T05 to 38T09 of finding and
    /// checking it; as the definition's.
    pub(in crate::db) fn extension_statement(&self, cx: StatementContext<'_>, e: &ExtensionStatement) -> Result<Completion> {
        self.loading_allowed()?;
        let written = e.definition();
        if written.read.is_some() {
            return Err(refused("MANIFEST is written by the database in the text it logs, not by a statement").or_at(written.at));
        }
        match e {
            ExtensionStatement::Create(d) => self.create_found(cx, d),
            ExtensionStatement::Update(d) => self.update_found(cx, d),
        }
    }

    /// That the session may load an extension: its user owns the database,
    /// or none does.
    pub(in crate::db) fn loading_allowed(&self) -> Result<()> {
        let Some(owner) = self.db.owner()?.filter(|o| *o != *self.auth()) else { return Ok(()) };
        let msg = format!("[EXTENSION] only the database's owner {owner} loads an extension, and {} is not it", self.auth());
        Err(SqlError::new(Code::Privilege, msg))
    }

    /// The target of DROP EXTENSION `name`, written at `at`.
    ///
    /// # Errors
    ///
    /// As `loading_allowed`, placed at `at`.
    pub(in crate::db) fn extension_drop_target(&self, name: &str, at: usize) -> Result<DropTarget> {
        self.loading_allowed().or_at(at).map(|()| DropTarget::Extension(super::extension_lock(name)))
    }

    /// CREATE EXTENSION `d`: the extensions it requires that are not
    /// created, then it.
    fn create_found(&self, cx: StatementContext<'_>, d: &ExtensionDefinition) -> Result<Completion> {
        if cx.st.extensions.contains_key(&d.name) {
            return Err(refused(format!("there is an extension {} already", d.name)).or_at(d.at));
        }
        let found = Self::found(cx.st, d)?;
        let mut completion = Completion::Success;
        for f in found.iter().filter(|f| !cx.st.extensions.contains_key(&f.name)) {
            completion = self.made(cx, "CREATE", f)?;
        }
        Ok(completion)
    }

    /// ALTER EXTENSION `d` UPDATE, then its scripts.
    fn update_found(&self, cx: StatementContext<'_>, d: &ExtensionDefinition) -> Result<Completion> {
        let old = cx.st.extensions.get(&d.name).ok_or_else(|| refused(format!("there is no extension {}", d.name)).or_at(d.at))?;
        let from = Version::parse(&old.manifest.version)?;
        let directory = match &d.from {
            Some(path) => Self::placed(cx.st, path)?,
            None => Self::newest(cx.st, d, &from)?,
        };
        let f = Found { name: d.name.clone(), directory, dependency: false };
        self.made(cx, "ALTER", &f)?;
        let manifest = Manifest::parse(&read_manifest(&f.directory)?)?;
        for script in upgrade_scripts(&manifest, &from)? {
            self.run_script(cx, &f.directory.join(&script))?;
        }
        Ok(Completion::Success)
    }

    /// The newest version installed of `d`'s extension past `from`, of
    /// the range TO names if it does.
    fn newest(st: &State, d: &ExtensionDefinition, from: &Version) -> Result<PathBuf> {
        let range = d.version.as_deref().map(Range::parse).transpose()?;
        let pinned = pinned(st, Some(&d.name))?;
        let chosen = resolve_installed(&st.extension_host.limits.directories(), &d.name, range.as_ref(), &pinned)?;
        let (_, version, dir) =
            chosen.into_iter().find(|(n, _, _)| *n == d.name).ok_or_else(|| refused(format!("no version of {} is installed", d.name)))?;
        if version <= *from && d.version.is_none() {
            return Err(refused(format!("extension {} is at {from}, the newest version installed", d.name)));
        }
        Ok(dir)
    }

    /// The extensions CREATE EXTENSION `d` creates, those it requires
    /// first.
    fn found(st: &State, d: &ExtensionDefinition) -> Result<Vec<Found>> {
        let dirs = st.extension_host.limits.directories();
        let pinned = pinned(st, None)?;
        let mut found = Vec::new();
        let own = match &d.from {
            Some(path) => Some(Self::placed(st, path)?),
            None => None,
        };
        let requires = match &own {
            Some(dir) => Manifest::parse(&read_manifest(dir)?)?.requires,
            None => vec![(d.name.clone(), d.version.clone().unwrap_or_else(|| "*".to_owned()))],
        };
        for (name, range) in requires {
            let range = Range::parse(&range)?;
            for (n, _, dir) in resolve_installed(&dirs, &name, Some(&range), &pinned)? {
                if !found.iter().any(|f: &Found| f.name == n) {
                    found.push(Found { dependency: n != d.name, name: n, directory: dir });
                }
            }
        }
        if let Some(dir) = own {
            found.retain(|f| f.name != d.name);
            found.push(Found { name: d.name.clone(), directory: dir, dependency: false });
        }
        Ok(found)
    }

    /// The directory of FROM's `path`: a package file installed, or a
    /// directory allowed.
    fn placed(st: &State, path: &str) -> Result<PathBuf> {
        let host = &st.extension_host;
        let dirs = host.limits.directories();
        if path.ends_with(PACKAGE_SUFFIX) && Path::new(path).is_file() {
            return install_file(Path::new(path), &dirs, &*host.crypto()?, &host.trust());
        }
        dirs.allowed(Path::new(path))
    }

    /// `f` made by `verb` EXTENSION, as read, logged as its text.
    fn made(&self, cx: StatementContext<'_>, verb: &str, f: &Found) -> Result<Completion> {
        let host = &cx.st.extension_host;
        let directory = host.limits.directories().allowed(&f.directory)?;
        let signer = host.verify(&directory)?;
        let manifest = read_manifest(&directory)?;
        let digest = host.crypto().ok().filter(|_| signer.is_some() || directory.join(crate::plugin::package::DIGESTS_FILE).exists());
        let digest = digest.map(|c| hex(&c.sha256(manifest.as_bytes())));
        let update = if verb == "ALTER" { " UPDATE" } else { "" };
        let mut text = format!("{verb} EXTENSION \"{}\"{update}", f.name.replace('"', "\"\""));
        let _ = write!(text, " MANIFEST X'{}' DIRECTORY {}", hex(manifest.as_bytes()), quoted(&directory.display().to_string()));
        for (word, value) in [("SIGNER", &signer), ("DIGEST", &digest)] {
            if let Some(v) = value {
                let _ = write!(text, " {word} {}", quoted(v));
            }
        }
        if f.dependency {
            text.push_str(" AS DEPENDENCY");
        }
        let SqlStatement::Schema { statement, text } = Parser::new(&text)?.sql_statement()? else {
            return Err(refused("an extension's statement did not read back"));
        };
        let SchemaStatement::Extension(_) = &statement else { return Err(refused("an extension's statement did not read back")) };
        let st = self.definitions(self.db.snapshot()?)?;
        self.schema_statement(&st, cx.tx, &statement, &text)
    }

    /// Runs each statement of the script `path`, in the transaction.
    fn run_script(&self, cx: StatementContext<'_>, path: &Path) -> Result<()> {
        let script =
            std::fs::read_to_string(path).map_err(|e| refused(format!("the upgrade script {} cannot be read: {e}", path.display())))?;
        for sql in statements_of(&script)? {
            let statement = Parser::for_prepare(&sql, true)?.sql_statement()?;
            let st = self.definitions(self.db.snapshot()?)?;
            let mut params = Params::executing(Vec::new());
            self.run_statement(StatementContext { st: &st, tx: cx.tx }, &statement, &mut params, None)?;
        }
        Ok(())
    }
}

/// The manifest file of `directory`, read.
fn read_manifest(directory: &Path) -> Result<String> {
    let file = directory.join(MANIFEST_FILE);
    std::fs::read_to_string(&file).map_err(|e| crate::plugin::errors::unavailable(&format!("{} cannot be read: {e}", file.display())))
}

/// The extensions created, but `except`, at their versions, which a
/// resolution keeps.
fn pinned(st: &State, except: Option<&str>) -> Result<Vec<(String, Version)>> {
    st.extensions
        .values()
        .filter(|e| except != Some(e.manifest.name.as_str()))
        .map(|e| Ok((e.manifest.name.clone(), Version::parse(&e.manifest.version)?)))
        .collect()
}

/// The statements of `script`, split at its semicolons outside literals
/// and comments.
fn statements_of(script: &str) -> Result<Vec<String>> {
    let chars: Vec<char> = script.chars().collect();
    let mut all = Vec::new();
    let mut from = 0;
    for t in lex(script)? {
        let end = matches!(t.tok, Tok::End);
        if end || t.tok == Tok::Delimiter(delim::SEMICOLON) {
            let sql: String = chars.get(from..t.start.min(chars.len())).unwrap_or_default().iter().collect();
            if !sql.trim().is_empty() {
                all.push(sql.trim().to_owned());
            }
            from = t.start.saturating_add(1);
        }
        if end {
            break;
        }
    }
    Ok(all)
}
