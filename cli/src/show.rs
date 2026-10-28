//! \[DESIGN\] A query's rows shown as a table as they come (RULES.md D46):
//! each column as wide as its type's values are, or its name, so no row
//! need be held to measure them; numbers aligned right, text left. A value
//! wider than its column is shown whole, past it.

use std::io::{self, Write};

use turborel::storage::Row;
use turborel::value::{DataType, Value};
use turborel_client::Column;
use turborel_client::connection::Sink;

/// The widest a column is made for its type: wider values go past it.
pub const MAX_WIDTH: usize = 60;
/// The characters of an approximate number at its longest:
/// `-1.2345678901234567E-308`.
const APPROXIMATE_WIDTH: usize = 24;

/// How a column is shown.
#[derive(Debug, Clone, Copy)]
struct Shape {
    width: usize,
    right: bool,
}

/// What a column of `t`'s values needs, named `name`.
fn shape(name: &str, t: &DataType) -> Shape {
    let (width, right) = match t {
        // A bit string as its 0s and 1s, one to a bit.
        DataType::Character { length } | DataType::Varying { length } | DataType::Bit { length } | DataType::BitVarying { length } => {
            (usize::try_from(*length).unwrap_or(MAX_WIDTH), false)
        }
        // A sign and a point beside the digits.
        DataType::Exact { precision, .. } => (usize::try_from(*precision).unwrap_or(MAX_WIDTH).saturating_add(2), true),
        DataType::Approximate { .. } => (APPROXIMATE_WIDTH, true),
        // A type beyond the standard, shown as its text, as wide as it is.
        DataType::Extension(_) => (MAX_WIDTH, false),
    };
    Shape { width: width.min(MAX_WIDTH).max(name.chars().count()), right }
}

/// A table written to `out` as rows come, nulls shown as `null`.
pub struct Table<'a> {
    out: &'a mut dyn Write,
    null: &'a str,
    shapes: Vec<Shape>,
    /// The first failure to write: nothing more is written after it.
    failed: Option<io::Error>,
}

impl<'a> Table<'a> {
    /// A table to write to `out`, nulls shown as `null`.
    pub fn new(out: &'a mut dyn Write, null: &'a str) -> Table<'a> {
        Table { out, null, shapes: Vec::new(), failed: None }
    }

    /// Ends the table, `count` rows shown.
    ///
    /// # Errors
    ///
    /// The first failure to write the table.
    pub fn finish(mut self, count: u64) -> io::Result<()> {
        let plural = if count == 1 { "" } else { "s" };
        self.write(&format!("({count} row{plural})"));
        self.failed.map_or(Ok(()), Err)
    }

    /// Writes `line`, its trailing spaces cut, unless writing failed.
    fn write(&mut self, line: &str) {
        if self.failed.is_none() {
            self.failed = writeln!(self.out, "{}", line.trim_end()).err();
        }
    }

    /// The line of `cells`, each in its column.
    fn line<'c>(&self, cells: impl Iterator<Item = &'c str>) -> String {
        let padded = cells.zip(&self.shapes).map(|(cell, shape)| {
            if shape.right { format!("{cell:>width$}", width = shape.width) } else { format!("{cell:<width$}", width = shape.width) }
        });
        format!(" {}", padded.collect::<Vec<_>>().join(" | "))
    }
}

impl Sink for Table<'_> {
    fn columns(&mut self, columns: &[Column]) {
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_deref().unwrap_or("")).collect();
        self.shapes = names.iter().zip(columns).map(|(name, c)| shape(name, &c.data_type)).collect();
        let header = self.line(names.into_iter());
        let rule = self.shapes.iter().map(|s| "-".repeat(s.width.saturating_add(2))).collect::<Vec<_>>().join("+");
        self.write(&header);
        self.write(&rule);
    }

    fn rows(&mut self, rows: &[Row]) {
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .map(|v| match v {
                    Value::Null => self.null.to_owned(),
                    Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Extension(_) => v.to_string(),
                })
                .collect();
            let line = self.line(cells.iter().map(String::as_str));
            self.write(&line);
        }
        if self.failed.is_none() {
            self.failed = self.out.flush().err();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turborel::value::Decimal;

    fn column(name: Option<&str>, data_type: DataType) -> Column {
        Column { name: name.map(str::to_owned), data_type, not_null: false }
    }

    /// Verifies: [DESIGN] RULES.md D46: a column is as wide as its type's
    /// values, at most `MAX_WIDTH`, or its name; numbers align right, text
    /// left; a null is shown as asked; the count ends the table.
    #[test]
    fn a_table_is_shaped_by_its_types() {
        let mut out = Vec::new();
        let mut table = Table::new(&mut out, "?");
        let columns = [
            column(Some("NAME"), DataType::Character { length: 3 }),
            column(Some("N"), DataType::Exact { precision: 3, scale: 1 }),
            column(None, DataType::Approximate { precision: 53 }),
            column(Some("LONG"), DataType::Character { length: 32767 }),
        ];
        table.columns(&columns);
        let row = vec![Value::Char("ab".into()), Value::Exact(Decimal::new(-125, 1)), Value::Null, Value::Char("x".into())];
        table.rows(&[row]);
        table.finish(1).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], format!(" NAME |     N | {:24} | LONG", ""));
        assert_eq!(lines[1], format!("------+-------+{}+{}", "-".repeat(26), "-".repeat(MAX_WIDTH + 2)));
        assert_eq!(lines[2], format!(" ab   | -12.5 | {:>24} | x", "?"));
        assert_eq!(lines[3], "(1 row)");
    }

    /// Verifies: [DESIGN] RULES.md D46: a failure to write is kept, and
    /// nothing more is written after it.
    #[test]
    fn a_failure_to_write_is_reported() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut out = Broken;
        let mut table = Table::new(&mut out, "");
        table.columns(&[column(Some("A"), DataType::Character { length: 1 })]);
        table.rows(&[vec![Value::Char("a".into())]]);
        assert_eq!(table.finish(2).unwrap_err().to_string(), "closed");
    }
}
