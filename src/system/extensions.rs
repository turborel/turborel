//! \[EXTENSION\] The views of extensions (RULES.md D172), in
//! INFORMATION_SCHEMA, each as SELECT on it is PUBLIC's, as any user may
//! call an extension's objects:
//!
//! * EXTENSIONS: each extension created, its version, directory, signer,
//!   and its processes running and started, calls made and processes
//!   stopped after a failure since the database opened.
//! * EXTENSION_OBJECTS: each object an extension declares, its kind,
//!   name, determinism and batch, and its declaration as written; and
//!   each setting it declares, of type SETTING, and each built-in one it
//!   reads, of type READS SETTING (RULES.md D281).
//! * EXTENSION_PACKAGES: the database's lock of packages, each extension
//!   with its digest, whether CREATE EXTENSION named it, the ranges it
//!   requires of others, and its license (RULES.md D267).

use std::sync::Arc;

use super::{INFORMATION_SCHEMA, Seen, SystemTable, cardinal, identifier, number, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;
use crate::value::Value;

/// The longest directory, digest, declaration or list of requirements
/// shown.
const TEXT_LENGTH: u32 = 4000;

/// The type of a row of EXTENSION_OBJECTS of a setting the extension
/// declares (RULES.md D281).
const SETTING: &str = "SETTING";
/// The type of a row of EXTENSION_OBJECTS of a built-in setting the
/// extension reads.
const READS_SETTING: &str = "READS SETTING";

/// EXTENSIONS.
struct Extensions;
/// EXTENSION_OBJECTS.
struct Objects;
/// EXTENSION_PACKAGES.
struct Packages;

/// The views.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 3] {
    [Arc::new(Extensions), Arc::new(Objects), Arc::new(Packages)]
}

/// `n`, a count.
fn count(n: u64) -> Value {
    number(usize::try_from(n).unwrap_or(usize::MAX))
}

/// `t`, or NULL.
fn optional(t: Option<&str>) -> Value {
    t.map_or(Value::Null, text)
}

impl SystemTable for Extensions {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EXTENSIONS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("EXTENSION_NAME", true), identifier("VERSION", true), text_column(("DIRECTORY", TEXT_LENGTH), true)];
        all.extend([identifier("SIGNER", false), cardinal("PROCESSES_RUNNING", true), cardinal("PROCESSES_STARTED", true)]);
        all.extend([cardinal("PROCESSES_FAILED", true), cardinal("CALLS", true), cardinal("CALLS_FAILED", true)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for e in seen.state.extensions.values() {
            let info = e.pool.info();
            let mut row = vec![text(&e.manifest.name), text(&e.manifest.version), text(&e.directory.display().to_string())];
            row.extend([optional(e.signer.as_deref()), count(info.running), count(info.started), count(info.stopped)]);
            row.extend([count(info.calls), count(info.failed)]);
            rows.push(row);
        }
        Ok(rows)
    }
}

impl SystemTable for Objects {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EXTENSION_OBJECTS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("EXTENSION_NAME", true), identifier("OBJECT_TYPE", true), identifier("OBJECT_NAME", true)];
        all.extend([identifier("IS_DETERMINISTIC", true), cardinal("BATCH_SIZE", false)]);
        all.push(text_column(("DECLARATION", TEXT_LENGTH), true));
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for e in seen.state.extensions.values() {
            for (d, _) in &e.objects {
                let batch = d.batch.map_or(Value::Null, |b| count(u64::from(b)));
                let row = vec![text(&e.manifest.name), text(d.kind.word()), text(&d.name), yes(d.deterministic), batch, text(&d.text)];
                rows.push(row);
            }
            let m = &e.manifest;
            let declared = m.settings.iter().map(|d| (SETTING, d.qualified(&m.name), d.text.clone()));
            let read = m.reads_settings.iter().map(|n| (READS_SETTING, n.clone(), format!("READS SETTINGS ('{n}')")));
            for (kind, name, declaration) in declared.chain(read) {
                rows.push(vec![text(&m.name), text(kind), text(&name), yes(false), Value::Null, text(&declaration)]);
            }
        }
        Ok(rows)
    }
}

impl SystemTable for Packages {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "EXTENSION_PACKAGES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("PACKAGE_NAME", true), identifier("PACKAGE_VERSION", true), identifier("DIGEST", false)];
        all.extend([identifier("IS_EXPLICIT", true), text_column(("REQUIRES", TEXT_LENGTH), true)]);
        all.extend([text_column(("TURBOREL", TEXT_LENGTH), false), text_column(("LICENSE", TEXT_LENGTH), false)]);
        all.push(text_column(("LICENSE_FILE", TEXT_LENGTH), false));
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for desc in seen.state.catalog.extensions() {
            let manifest = seen.state.extensions.get(&desc.name).map(|e| &e.manifest);
            let requires = manifest.map(|m| m.requires.iter().map(|(n, r)| format!("{n} {r}")).collect::<Vec<_>>().join(", "));
            let mut row = vec![text(&desc.name), text(&desc.version), optional(desc.digest.as_deref())];
            row.extend([yes(desc.explicit), text(&requires.unwrap_or_default())]);
            row.extend([optional(manifest.and_then(|m| m.turborel.as_deref())), optional(manifest.and_then(|m| m.license.as_deref()))]);
            row.push(optional(manifest.and_then(|m| m.license_file.as_deref())));
            rows.push(row);
        }
        Ok(rows)
    }
}
