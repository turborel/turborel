//! \[EXTENSION\] A plug-in's table function (RULES.md D169), a
//! `TableFunction`. If its manifest says READS SQL DATA, the program is
//! asked, as the invocation is bound, which queries it reads (op 2); the
//! statement runs them in its own transaction with its user's privileges,
//! as it does a built-in table function's, and the rows are sent with the
//! arguments (op 1).

use crate::bytes::Put;
use crate::error::Result;
use crate::functions::builtin::TableFunction;
use crate::plugin::manifest::Declared;
use crate::plugin::payload::{TABLE_READS, TABLE_ROWS, strings};
use crate::plugin::values::{put_values, stored};
use crate::plugin::wire::{put_rows, rows};
use crate::value::{DataType, Value};

use super::{Link, Resolve, answer, counted, resolved};

/// A plug-in's table function.
#[derive(Debug)]
pub struct PluginTable {
    link: Link,
    parameters: Vec<DataType>,
    columns: Vec<(String, DataType)>,
    reads: bool,
}

impl PluginTable {
    /// The table function `d` declares.
    ///
    /// # Errors
    ///
    /// A type `resolve` refuses.
    pub fn new(d: &Declared, link: Link, resolve: Resolve<'_>) -> Result<PluginTable> {
        let parameters = resolved(&d.parameters, resolve)?;
        let types: Vec<_> = d.columns.iter().map(|(_, t)| t.clone()).collect();
        let columns = d.columns.iter().map(|(n, _)| n.clone()).zip(resolved(&types, resolve)?).collect();
        Ok(PluginTable { link, parameters, columns, reads: d.reads })
    }
}

impl TableFunction for PluginTable {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == self.parameters.len()).then(|| self.parameters.clone())
    }
    fn columns(&self) -> Vec<(String, DataType)> {
        self.columns.clone()
    }
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        if !self.reads {
            return Ok(Vec::new());
        }
        let given: Vec<Value> = known.iter().map(|v| v.cloned().unwrap_or(Value::Null)).collect();
        let mut payload = Vec::new();
        put_values(&mut payload, &given)?;
        let got = self.link.plain(TABLE_READS, payload)?;
        answer(&got, strings)
    }
    fn rows(&self, args: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let mut payload = Vec::new();
        put_values(&mut payload, args)?;
        payload.put_u32(u32::try_from(read.len()).unwrap_or(u32::MAX));
        for query in read {
            put_rows(&mut payload, query)?;
        }
        let got = self.link.plain(TABLE_ROWS, payload)?;
        let made = answer(&got, rows)?;
        made.into_iter()
            .map(|row| {
                let row = counted(row, self.columns.len())?;
                row.into_iter().zip(&self.columns).map(|(v, (_, t))| stored(v, t)).collect()
            })
            .collect()
    }
}
