//! \[DESIGN\] What an update that holds the values of the columns it sets
//! alone tells its table (RULES.md D916), and the row after as its log
//! record holds it (D475), which only the table can give, as it alone
//! holds the columns the update did not set.
//!
//! The image is the values of the row after, in column order, each as
//! `value::codec` writes a value, and where the values of the columns set
//! lie among them. An update's log record is made of it and of the values
//! those columns had before (`log::record::put_update_record_image`), the
//! bytes the record of the two rows whole would be. A table that reads the
//! row whole to write it gives the image of that row (`Patch::whole`).

use std::ops::Range;

use super::{Row, RowId, TableStorage};
use crate::error::Result;
use crate::txn::TxId;
use crate::value::codec::put_value;

/// A column, and where its value lies among an image's bytes.
pub(crate) type Placed = (usize, Range<usize>);

/// \[DESIGN\] The row after an update as its log record holds it.
#[derive(Debug, Default)]
pub struct AfterImage {
    /// Its values, in column order, each as `value::codec` writes it.
    values: Vec<u8>,
    /// How many they are.
    columns: usize,
    /// Each column set, in ascending order, and where its value lies in
    /// `values`.
    set: Vec<Placed>,
}

impl AfterImage {
    /// An image of no row yet, holding no room: one a thread keeps is
    /// written again by each update.
    pub const fn new() -> AfterImage {
        AfterImage { values: Vec::new(), columns: 0, set: Vec::new() }
    }

    /// Empties it, its room kept.
    pub fn clear(&mut self) {
        self.values.clear();
        self.set.clear();
        self.columns = 0;
    }

    /// Appends its next column's value, written by `write`, and notes
    /// where it lies if the column is `set`.
    ///
    /// # Errors
    ///
    /// `write`'s; the image is then to be cleared.
    pub fn put(&mut self, set: bool, write: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> Result<()> {
        let start = self.values.len();
        write(&mut self.values)?;
        if set {
            self.set.push((self.columns, start..self.values.len()));
        }
        self.columns = self.columns.saturating_add(1);
        Ok(())
    }

    /// Makes it the image of `row`, an update of which set the columns
    /// `set`.
    pub fn put_row(&mut self, row: &Row, set: &[usize]) {
        self.clear();
        for (column, value) in row.iter().enumerate() {
            let start = self.values.len();
            put_value(&mut self.values, value);
            if set.contains(&column) {
                self.set.push((column, start..self.values.len()));
            }
        }
        self.columns = row.len();
    }

    /// Its values' bytes, how many values they are, and where those of the
    /// columns set lie among the bytes.
    pub(crate) fn parts(&self) -> (&[u8], usize, &[Placed]) {
        (&self.values, self.columns, &self.set)
    }
}

/// \[DESIGN\] What an update whose row holds the values of the columns it
/// sets alone tells `TableStorage::replace_set` (RULES.md D916).
#[derive(Debug)]
pub struct Patch<'a> {
    /// The columns set, in ascending order, each once: the row written
    /// keeps every other as the table holds it.
    pub set: &'a [usize],
    /// The row after, for the table to give as it writes it.
    pub image: &'a mut AfterImage,
}

impl Patch<'_> {
    /// Row `id` of `storage` as `tx` sees it, read whole, with each column
    /// set given its value in `row`, and its image taken: the row a table
    /// that patches nothing writes whole.
    ///
    /// # Errors
    ///
    /// As `TableStorage::get`; SQLCODE -910 if there is no such row.
    pub fn whole<S: TableStorage + ?Sized>(&mut self, storage: &S, tx: TxId, id: RowId, row: &Row) -> Result<Row> {
        let whole = super::rows::whole_set(storage, tx, id, (row, self.set))?;
        self.image.put_row(&whole, self.set);
        Ok(whole)
    }
}

// Unit tests: an image's values and where those set lie (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::AfterImage;
    use crate::value::codec::put_value;
    use crate::value::{Decimal, Value};

    /// Verifies: [DESIGN] RULES.md D916: the image of a row is its values
    /// as the codec writes them, one after another, with the place of each
    /// column set; written a value at a time it is the same; and cleared,
    /// it is of no row.
    #[test]
    fn an_image_holds_its_rows_values_and_where_those_set_lie() {
        let row = vec![Value::Exact(Decimal::new(7, 0)), Value::Null, Value::Char("text".into())];
        let mut whole = Vec::new();
        for v in &row {
            put_value(&mut whole, v);
        }
        let mut image = AfterImage::new();
        image.put_row(&row, &[0, 2]);
        let (values, columns, set) = image.parts();
        assert_eq!((values, columns), (whole.as_slice(), 3));
        let lie: Vec<(usize, Vec<u8>)> = set.iter().map(|(c, span)| (*c, values[span.clone()].to_vec())).collect();
        let alone = |v: &Value| {
            let mut bytes = Vec::new();
            put_value(&mut bytes, v);
            bytes
        };
        assert_eq!(lie, [(0, alone(&row[0])), (2, alone(&row[2]))]);
        let mut stepped = AfterImage::new();
        for (column, value) in row.iter().enumerate() {
            stepped
                .put(column != 1, |out| {
                    put_value(out, value);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(format!("{:?}", stepped.parts()), format!("{:?}", image.parts()));
        image.clear();
        assert_eq!(image.parts(), (&[][..], 0, &[][..]));
    }
}
