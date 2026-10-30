//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.FILES: each
//! file the database holds (RULES.md D56), an object "associated with
//! INFORMATION_SCHEMA" this implementation adds: where it is, its kind,
//! whether its descriptor is open now, and its reads, writes and syncs.
//! The database's owner sees them; any other, none, as
//! MEMORY_RESERVATIONS. Split from system.rs (RULES.md R-SIZE-2).

use std::path::Path;

use super::{Seen, SystemTable, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::files::plugins::PLUGINS;
use crate::storage::Row;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Value};

/// \[DESIGN\] The longest path shown.
const PATH_LENGTH: u32 = 1024;

/// \[EXTENSION\] INFORMATION_SCHEMA.FILES.
pub(super) struct Files;

/// The kind of the file at `path`, by its name and its directory's: the
/// data file and its shadow files, the catalog, a segment of the log or of
/// older row versions, a spill file, a plug-in's own file (in a directory
/// of `plugins/`, first, whatever its name), or another.
fn kind(path: &Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let dir = path.parent().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or_default();
    if path.ancestors().skip(2).any(|a| a.file_name().is_some_and(|n| n == PLUGINS)) {
        "PLUGIN"
    } else if name == "data" {
        "DATA"
    } else if name.starts_with("data.shadow") {
        "SHADOW"
    } else if name.starts_with("catalog.") {
        "CATALOG"
    } else if dir == "log" {
        "LOG"
    } else if dir == "undo" {
        "UNDO"
    } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("spill")) {
        "SPILL"
    } else {
        "OTHER"
    }
}

impl SystemTable for Files {
    fn name(&self) -> &'static str {
        "FILES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let count = |name: &str| ColumnDesc {
            name: name.to_owned(),
            data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
            not_null: true,
            default: None,
            domain: None,
        };
        vec![
            text_column(("PATH", PATH_LENGTH), true),
            text_column(("KIND", 7), true),
            text_column(("IS_OPEN", 3), true),
            count("READS"),
            count("WRITES"),
            count("SYNCS"),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let count = |n: u64| Value::Exact(Decimal::new(i128::from(n), 0));
        let shown =
            |p: &Path| text(&p.display().to_string().chars().take(usize::try_from(PATH_LENGTH).unwrap_or(usize::MAX)).collect::<String>());
        Ok(seen
            .state
            .files
            .infos()
            .into_iter()
            .map(|f| {
                vec![
                    shown(&f.path),
                    text(if f.noted { "SERVER" } else { kind(&f.path) }),
                    yes(f.open),
                    count(f.reads),
                    count(f.writes),
                    count(f.syncs),
                ]
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D56: each file's kind, by its name and
    /// its directory's.
    #[test]
    fn files_are_of_their_kinds() {
        for (path, expected) in [
            ("db/data", "DATA"),
            ("db/data.shadow1", "SHADOW"),
            ("db/catalog.0", "CATALOG"),
            ("db/log/segment-0000000001.log", "LOG"),
            ("db/undo/segment-0000000001.log", "UNDO"),
            ("db/tmp/turborel-1-2-3.spill", "SPILL"),
            ("db/lock", "OTHER"),
            ("db/plugins/mine/data", "PLUGIN"),
            ("db/plugins/mine/parts/0", "PLUGIN"),
        ] {
            assert_eq!(kind(Path::new(path)), expected, "{path}");
        }
    }
}
