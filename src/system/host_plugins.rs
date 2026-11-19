//! \[EXTENSION\] INFORMATION_SCHEMA.HOST_PLUGINS (RULES.md D229): each
//! object of each host plug-in the node started, its plug-in's version,
//! directory, whether PLUGINS.HOST pins its digest, and its license as
//! its manifest declares it (RULES.md D267, `package_license`); its
//! processes running, the calls made of its plug-in, and why the node
//! stopped, if a host plug-in's failure stopped it. Each setting its
//! manifest declares is a row of type SETTING (RULES.md D281). Host plug-ins are the
//! node's, so the database's owner sees them; any other, none.

use super::{INFORMATION_SCHEMA, Seen, SystemTable, cardinal, identifier, number, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::plugin::package::license::License;
use crate::storage::Row;
use crate::value::Value;

/// The type of a row of a setting the plug-in declares (RULES.md D281).
const SETTING: &str = "SETTING";
/// The longest directory, license or reason shown.
const TEXT_LENGTH: u32 = 4000;

/// HOST_PLUGINS.
pub(super) struct HostPlugins;

/// `t`, or NULL.
fn optional(t: Option<&str>) -> Value {
    t.map_or(Value::Null, text)
}

/// `n`, a count.
fn count(n: u64) -> Value {
    number(usize::try_from(n).unwrap_or(usize::MAX))
}

impl SystemTable for HostPlugins {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "HOST_PLUGINS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut all = vec![identifier("PLUGIN_NAME", true), identifier("VERSION", true), text_column(("DIRECTORY", TEXT_LENGTH), true)];
        all.extend([identifier("IS_PINNED", true), identifier("OBJECT_TYPE", true), identifier("OBJECT_NAME", true)]);
        all.extend([text_column(("LICENSE", TEXT_LENGTH), false), text_column(("LICENSE_FILE", TEXT_LENGTH), false)]);
        all.extend([cardinal("PROCESSES_RUNNING", true), cardinal("CALLS", true), text_column(("STOPPED", TEXT_LENGTH), false)]);
        all
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let host = &seen.state.extension_host;
        let stopped = host.stop.stopped();
        let mut rows = Vec::new();
        for p in &host.hosted() {
            let (info, license) = (p.pool.info(), License::of(&p.manifest));
            let objects = p.manifest.hosted.iter().map(|d| (d.kind.word(), d.name.clone()));
            let settings = p.manifest.settings.iter().map(|d| (SETTING.to_owned(), d.qualified(&p.manifest.name)));
            for (kind, name) in objects.chain(settings) {
                let mut row = vec![text(&p.manifest.name), text(&p.manifest.version), text(&p.directory.display().to_string())];
                row.extend([yes(p.pinned), text(&kind), text(&name)]);
                row.extend([optional(license.expression.as_deref()), optional(license.file.as_deref())]);
                row.extend([count(info.running), count(info.calls), optional(stopped.as_deref())]);
                rows.push(row);
            }
        }
        Ok(rows)
    }
}
