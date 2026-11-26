//! \[DESIGN\] A batch of records' fields by column, as text (RULES.md
//! D1148): what a `Source` that splits its records itself appends each
//! field to, so no value, and no string, is made of a field. Each column
//! keeps its cells' text end to end, where each ends, and a bit for each
//! null, as CopyColumns carries them; its buffers are kept from batch to
//! batch, so a long import allocates no more once the first batch is
//! sent.

use std::io::{Read, Write};

use turborel::storage::lanes::{Lane, Lanes};

use crate::connection::Result;
use crate::copy::Copying;

/// The bytes CopyColumns spends on a cell besides its text: its end.
const END_BYTES: usize = 4;
/// The null bits a byte holds.
const BITS: usize = 8;
/// The bits of a row's number that say its bit within its null byte.
const WITHIN: usize = 7;
/// The shift of a row's number that gives its null byte.
const BYTE_SHIFT: usize = 3;

/// The fields of a batch of records, by column.
#[derive(Debug, Default)]
pub struct Cells {
    lanes: Lanes,
    /// The field of the record under way the next one appended is.
    field: usize,
    /// About the bytes CopyColumns of the cells would be.
    bytes: usize,
}

impl Cells {
    /// Cells of `columns` columns, none yet.
    pub(super) fn new(columns: usize) -> Cells {
        Cells { lanes: Lanes { rows: 0, columns: vec![Lane::default(); columns] }, field: 0, bytes: 0 }
    }

    /// Begins a record: its first field the next appended.
    pub(super) fn begin(&mut self) {
        self.field = 0;
    }

    /// Appends the field `text`.
    pub fn text(&mut self, text: &str) {
        self.piece(text.as_bytes());
        self.end();
    }

    /// Appends a null field.
    pub fn null(&mut self) {
        let (rows, bit) = (self.lanes.rows, 1_u8 << (self.lanes.rows & WITHIN));
        if let Some(lane) = self.lanes.columns.get_mut(self.field) {
            // The row's byte of null bits is made by its first null, or by
            // its end (`row`), so a record never ended adds none.
            if lane.nulls.len() <= rows >> BYTE_SHIFT {
                lane.nulls.push(0);
            }
            if let Some(byte) = lane.nulls.last_mut() {
                *byte |= bit;
            }
        }
        self.end();
    }

    /// Appends `bytes`, UTF-8 as a whole field is, to the field under way.
    pub(crate) fn piece(&mut self, bytes: &[u8]) {
        // A field past the last column is counted, not kept: the record is
        // refused for its count of fields.
        if let Some(lane) = self.lanes.columns.get_mut(self.field) {
            lane.bytes.extend_from_slice(bytes);
            self.bytes = self.bytes.saturating_add(bytes.len());
        }
    }

    /// Ends the field under way.
    pub(crate) fn end(&mut self) {
        if let Some(lane) = self.lanes.columns.get_mut(self.field) {
            // A chunk is sent long before its text is 4 GiB, so the end
            // fits; were it not to, the server refuses the column.
            lane.ends.push(u32::try_from(lane.bytes.len()).unwrap_or(u32::MAX));
            self.bytes = self.bytes.saturating_add(END_BYTES);
        }
        self.field = self.field.saturating_add(1);
    }

    /// How many fields the record under way has.
    pub(super) fn fields(&self) -> usize {
        self.field
    }

    /// Ends the record under way, a row of the batch: it has a field for
    /// each column.
    pub(super) fn row(&mut self) {
        self.lanes.rows = self.lanes.rows.saturating_add(1);
        let bytes = self.lanes.rows.div_ceil(BITS);
        for lane in &mut self.lanes.columns {
            if lane.nulls.len() < bytes {
                lane.nulls.push(0);
            }
        }
    }

    /// How many columns it has.
    pub(super) fn columns(&self) -> usize {
        self.lanes.columns.len()
    }

    /// About the bytes CopyColumns of the rows would be.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Sends the rows over `copying` in a CopyColumns, if there are any,
    /// and empties the cells, their buffers kept.
    pub(super) fn send<S: Read + Write>(&mut self, copying: &mut Copying<'_, S>) -> Result<()> {
        if self.lanes.rows == 0 {
            return Ok(());
        }
        let sent = copying.send_columns(&mut self.lanes);
        self.clear();
        sent
    }

    /// Empties the cells, their buffers kept.
    pub(super) fn clear(&mut self) {
        for lane in &mut self.lanes.columns {
            lane.nulls.clear();
            lane.ends.clear();
            lane.bytes.clear();
        }
        (self.lanes.rows, self.field, self.bytes) = (0, 0, 0);
    }

    /// The rows by column, to read back.
    #[cfg(test)]
    pub(super) fn lanes(&self) -> &Lanes {
        &self.lanes
    }
}

#[cfg(test)]
mod tests {
    use turborel_protocol::Request;

    use super::*;

    /// The kind and body of `request`, encoded.
    fn framed(request: &Request) -> (u8, Vec<u8>) {
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        (bytes[0], bytes[turborel_protocol::HEADER..].to_vec())
    }

    /// Verifies: [DESIGN] RULES.md D1148: the cells of a batch go as a
    /// CopyColumns the server takes, read back as they were, a null among
    /// them past a byte of null bits; one whose column has a byte of null
    /// bits too few, or too many, is refused.
    #[test]
    fn cells_go_as_a_copy_columns_the_server_takes() {
        let mut cells = Cells::new(2);
        for n in 0..10_u32 {
            cells.begin();
            cells.text(&n.to_string());
            if n == 9 {
                cells.null();
            } else {
                cells.text("v");
            }
            cells.row();
        }
        let request = Request::CopyColumns(cells.lanes().clone());
        let (kind, body) = framed(&request);
        let Ok(Request::CopyColumns(back)) = Request::decode(kind, &body) else { panic!("refused") };
        assert_eq!(&back, cells.lanes());
        assert_eq!(back.columns[1].cell(9).unwrap(), None);
        for nulls in [vec![0], vec![0, 0, 0]] {
            let mut wrong = cells.lanes().clone();
            wrong.columns[0].nulls = nulls;
            let (wrong_kind, wrong_body) = framed(&Request::CopyColumns(wrong));
            assert!(Request::decode(wrong_kind, &wrong_body).is_err());
        }
    }
}
