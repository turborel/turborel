//! \[EXTENSION\] A plug-in's system table (RULES.md D222), a
//! `SystemTable` of INFORMATION_SCHEMA whose rows are asked as a query
//! reads it, a page of BATCH rows a call, each row's values stored in its
//! column's type as a routine's result is. Its name lives as long as the
//! program, as `SystemTable` names are fixed (`leaked`).

use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::plugin::payload::SYSTEM_ROWS;
use crate::plugin::payload::system::{Page, paged, put_page};
use crate::plugin::values::stored;
use crate::storage::Row;
use crate::system::{INFORMATION_SCHEMA, Seen, SystemTable};
use crate::value::DataType;

use super::{Link, answer, leaked};

/// The rows a page holds when the manifest says no BATCH.
pub const DEFAULT_BATCH: u32 = 1024;
/// \[DESIGN\] The most rows a system table gives one query: its rows are
/// held together (`SystemTable::rows`), so a program that never says it
/// is done is stopped.
pub const MOST_ROWS: usize = 1_000_000;

/// A plug-in's system table.
#[derive(Debug)]
pub struct PluginSystemTable {
    link: Link,
    name: &'static str,
    columns: Vec<ColumnDesc>,
    batch: u32,
}

impl PluginSystemTable {
    /// The table `link` names, of `columns`, `batch` rows a page.
    pub fn new(link: Link, columns: Vec<(String, DataType)>, batch: Option<u32>) -> PluginSystemTable {
        let columns = columns
            .into_iter()
            .map(|(name, data_type)| ColumnDesc {
                name,
                data_type,
                not_null: false,
                default: None,
                domain: None,
                collation: None,
                generated: None,
            })
            .collect();
        let name = leaked(&link.object);
        PluginSystemTable { link, name, columns, batch: batch.unwrap_or(DEFAULT_BATCH).max(1) }
    }
}

impl SystemTable for PluginSystemTable {
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn columns(&self) -> Vec<ColumnDesc> {
        self.columns.clone()
    }
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut all: Vec<Row> = Vec::new();
        loop {
            let from = u64::try_from(all.len()).unwrap_or(u64::MAX);
            let mut payload = Vec::new();
            put_page(&mut payload, &Page { user: seen.auth.to_owned(), owner: seen.is_owner(), from, max: self.batch });
            let (rows, more) = answer(&self.link.plain(SYSTEM_ROWS, payload)?, paged)?;
            for row in rows {
                if row.len() != self.columns.len() {
                    return Err(crate::plugin::errors::violation(&format!(
                        "a row of {} values, of {} columns",
                        row.len(),
                        self.columns.len()
                    )));
                }
                all.push(row.into_iter().zip(&self.columns).map(|(v, c)| stored(v, &c.data_type)).collect::<Result<_>>()?);
            }
            if !more {
                return Ok(all);
            }
            if all.len() >= MOST_ROWS {
                return Err(crate::plugin::errors::violation(&format!("its system table gave more than {MOST_ROWS} rows")));
            }
        }
    }
}
