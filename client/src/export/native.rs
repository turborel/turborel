//! \[DESIGN\] Native rows (RULES.md D195, D975): the protocol's own frames,
//! a Description, then Rows replies, then a Complete, every value as it
//! was, of whatever type. A Rows reply holds at most a reply's longest
//! body (`MAX_BODY`). A large object sent by its locator is written in it
//! as a locator numbered within the reply, from 1, depth first in the
//! order of the rows, their fields and their elements; and its octets follow the Rows
//! reply, in that order, in Lob Data replies of at most a chunk each, as
//! many as its length takes. So a large object of any length is written
//! as it is read, and `read_native` gives each its value.
//!
//! ```text
//! Description
//! { Rows  { Lob Data }... }...
//! Complete
//! ```

use std::io::{self, Read, Write};

use turborel::storage::Row;
use turborel::value::lob::Lob;
use turborel_protocol::{Column, Completed, Failure, HEADER, LobReply, MAX_BODY, Reply, row_size};

use super::{Layout, Lobs, held, map_located};
use crate::connection::{Result, failed};

/// The octets of a Rows reply's body before its rows: their count.
const ROWS_COUNT: usize = 4;

/// \[DESIGN\] RULES.md D975: rows as the protocol's own frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct Native;

/// Writes `reply` to `out` as a frame.
fn frame(out: &mut dyn Write, reply: &Reply) -> io::Result<()> {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    out.write_all(&bytes)
}

impl Native {
    /// `rows`, each large object in them sent by its locator numbered in
    /// its place, from 1, and those large objects, in order.
    fn numbered(rows: &[Row]) -> io::Result<(Vec<Row>, Vec<Lob>)> {
        let mut found = Vec::new();
        let mut renumber = |l: &Lob| {
            found.push(l.clone());
            let number = u32::try_from(found.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "more large objects than a reply numbers"))?;
            Ok(Lob::of_locator(l.kind(), number, (l.octets(), l.chars())))
        };
        let rows = rows.iter().map(|row| row.iter().map(|v| map_located(v, &mut renumber)).collect()).collect::<io::Result<_>>()?;
        Ok((rows, found))
    }

    /// Writes `rows` as one Rows reply, then the octets of each large
    /// object in them sent by its locator.
    fn group(out: &mut dyn Write, rows: &[Row], lobs: &mut dyn Lobs) -> io::Result<()> {
        let (rows, found) = Native::numbered(rows)?;
        frame(out, &Reply::Rows(rows))?;
        for lob in &found {
            lobs.chunks(lob, &mut |chunk| frame(out, &Reply::Lob(LobReply::Data(chunk.to_vec()))))?;
        }
        Ok(())
    }
}

impl Layout for Native {
    fn columns(&mut self, out: &mut dyn Write, columns: &[Column]) -> io::Result<()> {
        frame(out, &Reply::Description(columns.to_vec()))
    }

    /// The rows, in Rows replies of at most a reply's longest body each.
    fn rows(&mut self, out: &mut dyn Write, rows: &[Row], lobs: &mut dyn Lobs) -> io::Result<()> {
        let most = usize::try_from(MAX_BODY).unwrap_or(usize::MAX);
        let (mut from, mut body) = (0, ROWS_COUNT);
        for (at, row) in rows.iter().enumerate() {
            let size = row_size(row);
            if at > from && body.saturating_add(size) > most {
                Native::group(out, rows.get(from..at).unwrap_or_default(), lobs)?;
                (from, body) = (at, ROWS_COUNT);
            }
            body = body.saturating_add(size);
        }
        if from < rows.len() { Native::group(out, rows.get(from..).unwrap_or_default(), lobs) } else { Ok(()) }
    }

    fn end(&mut self, out: &mut dyn Write) -> io::Result<()> {
        frame(out, &Reply::Complete(Completed::of(0, "00000")))
    }
}

/// The next frame of native rows from `input`.
fn next(input: &mut impl Read) -> Result<Reply> {
    let mut head = [0_u8; HEADER];
    input.read_exact(&mut head).map_err(|e| not_native(&e.to_string()))?;
    let (kind, length) = turborel_protocol::header(head, MAX_BODY).map_err(|e| Failure::from(&e))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body).map_err(|e| not_native(&e.to_string()))?;
    Reply::decode(kind, &body).map_err(|e| Failure::from(&e))
}

/// The failure of input that is not native rows: `why`.
fn not_native(why: &str) -> Failure {
    failed(&format!("[DESIGN] not native rows: {why}"))
}

/// The octets of `lob`, numbered in the file, read from the Lob Data
/// replies that follow its Rows reply: held whole.
fn filled(input: &mut impl Read, lob: &Lob) -> Result<Lob> {
    // Its length is the file's word, so no more is reserved than a reply holds.
    let most = usize::try_from(MAX_BODY).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(usize::try_from(lob.octets()).unwrap_or(0).min(most));
    while u64::try_from(bytes.len()).unwrap_or(u64::MAX) < lob.octets() {
        let Reply::Lob(LobReply::Data(chunk)) = next(input)? else { return Err(not_native("a large object's octets cut short")) };
        bytes.extend_from_slice(&chunk);
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > lob.octets() {
        return Err(not_native("a large object's octets past its length"));
    }
    held(lob.kind(), &bytes).map_err(|e| not_native(&e.to_string()))
}

/// `batch`, a Rows reply's rows, each large object numbered in the file
/// given its octets from `input`, which follow it in order.
fn located_in(input: &mut impl Read, batch: Vec<Row>) -> Result<Vec<Row>> {
    let mut found: Vec<Lob> = Vec::new();
    let mut collect = |l: &Lob| {
        // Numbered in the file in the order written, from 1.
        let expected = u32::try_from(found.len()).ok().and_then(|n| n.checked_add(1));
        if l.locator() != expected {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a large object numbered out of order"));
        }
        found.push(l.clone());
        Ok(l.clone())
    };
    for row in &batch {
        for v in row {
            map_located(v, &mut collect).map_err(|e| not_native(&e.to_string()))?;
        }
    }
    if found.is_empty() {
        return Ok(batch);
    }
    let mut values = found.iter().map(|l| filled(input, l)).collect::<Result<Vec<_>>>()?.into_iter();
    let mut fill = |_: &Lob| values.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a large object unread"));
    let filled = batch.iter().map(|row| row.iter().map(|v| map_located(v, &mut fill)).collect()).collect::<io::Result<_>>();
    filled.map_err(|e| not_native(&e.to_string()))
}

/// Reads native rows (`ExportFormat::Native`) from `input`: the columns,
/// and the rows, each large object held whole.
///
/// # Errors
///
/// SQLSTATE 08006 for input that is not native rows, or cut short.
pub fn read_native(input: &mut impl Read) -> Result<(Vec<Column>, Vec<Row>)> {
    let Reply::Description(columns) = next(input)? else { return Err(not_native("no Description first")) };
    let mut rows = Vec::new();
    loop {
        match next(input)? {
            Reply::Rows(batch) => rows.extend(located_in(input, batch)?),
            Reply::Complete(_) => return Ok((columns, rows)),
            Reply::Error(_)
            | Reply::Lob(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Suspended
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong
            | Reply::Debug(_)
            | Reply::Event(_) => return Err(not_native("a frame other than Rows before the Complete")),
        }
    }
}
