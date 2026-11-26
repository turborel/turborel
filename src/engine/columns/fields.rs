//! \[DESIGN\] The values of a column as a group writes them (RULES.md D31,
//! D1144, D1148): each as its type writes it in a record, end to end in
//! one buffer, where each ends, and which are null, as a load gives a
//! column (`storage/lanes.rs`), so a column is one allocation, not one a
//! value, and a load's column is taken as it is. Of them, the chunks a
//! group's pages hold, each sized once.

use std::borrow::Cow;

use super::encoded::{self, Plan, Sizer};
use super::{bit_of, corrupt, fsst, nulls_len, padding, too_large, word};
use crate::error::Result;
use crate::storage::lanes::Records;
use crate::value::{DataType, Value};

/// Of a column's records, those from `from` to `to`, read in place.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cells<'a> {
    records: &'a Records,
    from: usize,
    to: usize,
}

/// Where record `n` of `records` ends, `at` if it has no end.
fn end_of(records: &Records, n: usize, at: usize) -> usize {
    records.ends.get(n).map_or(at, |&e| usize::try_from(e).unwrap_or(usize::MAX))
}

impl<'a> Cells<'a> {
    /// All of `records`.
    pub(super) fn whole(records: &'a Records) -> Cells<'a> {
        Cells { records, from: 0, to: records.len() }
    }

    /// How many there are.
    pub(super) fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }

    /// Where the first's record begins in the buffer.
    fn start(&self) -> usize {
        self.from.checked_sub(1).map_or(0, |p| end_of(self.records, p, 0))
    }

    /// Each one's record, none for a null: of a record past the buffer,
    /// none too.
    pub(super) fn iter(self) -> impl Iterator<Item = Option<&'a [u8]>> + 'a {
        let records = self.records;
        let mut start = self.start();
        (self.from..self.to).map(move |n| {
            let end = end_of(records, n, start);
            let cell = if records.is_null(n) { None } else { records.bytes.get(start..end) };
            start = end;
            cell
        })
    }

    /// Their records end to end, a null's none, as the buffer holds them.
    pub(super) fn span(&self) -> &'a [u8] {
        let end = self.to.checked_sub(1).map_or(0, |last| end_of(self.records, last, 0));
        self.records.bytes.get(self.start()..end).unwrap_or_default()
    }

    /// Whether none is null.
    pub(super) fn no_nulls(&self) -> bool {
        (self.from..self.to).all(|n| !self.records.is_null(n))
    }
}

/// The width every value of `t` in `cells` is written in, if they all are.
fn width_of(cells: Cells<'_>, t: &DataType) -> Option<usize> {
    let width = t.kind().record_width(t)?;
    cells.iter().flatten().all(|f| f.len() == width).then_some(width)
}

/// The values of a column, each as its type writes it in a record, none
/// for a null, end to end (`Records`).
pub struct Fields<'r> {
    records: Cow<'r, Records>,
    table: std::sync::OnceLock<Option<fsst::Table>>,
    width: Option<usize>,
}

impl Fields<'static> {
    /// `values`, of type `t`, written into one buffer.
    ///
    /// # Errors
    ///
    /// As `SqlType::encode_record` says; SQLCODE -900 if they pass the
    /// bytes a buffer's ends count.
    pub fn of<V: std::borrow::Borrow<Value>>(t: &DataType, values: impl Iterator<Item = V>) -> Result<Fields<'static>> {
        let mut records = Records::default();
        let kind = t.kind();
        for (n, v) in values.enumerate() {
            let (byte, bit) = bit_of(n);
            if byte == records.nulls.len() {
                records.nulls.push(0);
            }
            let value = v.borrow();
            if !value.is_null() {
                kind.encode_record_into(value, t, &mut records.bytes)?;
            } else if let Some(b) = records.nulls.get_mut(byte) {
                *b |= 1 << bit;
            }
            records.ends.push(u32::try_from(records.bytes.len()).map_err(|_| too_large())?);
        }
        Ok(Fields::from_records(t, Cow::Owned(records)))
    }
}

impl<'r> Fields<'r> {
    /// The values `records` holds, of type `t`, taken as they are, none
    /// copied: the width they share found once (RULES.md D1144, D1148).
    pub fn from_records(t: &DataType, records: Cow<'r, Records>) -> Fields<'r> {
        let width = width_of(Cells::whole(&records), t);
        Fields { records, table: std::sync::OnceLock::new(), width }
    }

    /// Their records.
    pub fn records(&self) -> &Records {
        &self.records
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Those from `from`, `count` of them; none past the last.
    fn cells(&self, from: usize, count: usize) -> Cells<'_> {
        let to = from.saturating_add(count).min(self.len());
        Cells { records: &self.records, from: from.min(to), to }
    }

    /// The record of value `n`, none if it is null.
    fn field(&self, n: usize) -> Option<&[u8]> {
        self.records.record(n)
    }

    /// How many of those from `from` on fit a chunk of `capacity` bytes, of
    /// type `t`, compressed if `compress`; at least one, so a value longer
    /// than a chunk has one of its own.
    pub fn fitting(&self, t: &DataType, from: usize, (capacity, compress): (usize, bool)) -> usize {
        self.fitting_planned(t, from, (capacity, compress)).0
    }

    /// As `fitting`, with the encoding the chunk of those that fit takes,
    /// as `chunk` would choose it of them (RULES.md D1144): the plan found
    /// as the last that fits was added, of the same values in the same
    /// order. A value alone longer than a chunk is given no plan.
    fn fitting_planned(&self, t: &DataType, from: usize, (capacity, compress): (usize, bool)) -> (usize, Option<Plan>) {
        let rest = self.cells(from, self.len());
        let mut sizer = self.sizer(t, compress);
        let mut fits = None;
        for (n, field) in rest.iter().enumerate() {
            sizer.add(field);
            let (plan, bytes) = sizer.best();
            if bytes > capacity {
                return if n == 0 { (1, None) } else { (n, fits) };
            }
            fits = Some(plan);
        }
        (rest.len(), fits)
    }

    /// A sizer of its values, of type `t`, compressed if `compress`.
    fn sizer(&self, t: &DataType, compress: bool) -> Sizer<'_> {
        Sizer::new(self.width, (compress, matches!(t, DataType::Approximate { .. })), self.table(t, compress))
    }

    /// \[DESIGN\] (RULES.md D31) How many rows from `from` on fit one page
    /// of `capacity` bytes together, a chunk of each of `columns`, each
    /// fields and their type, compressed if `compress`: none if even the
    /// first row's chunks do not (the hybrid layout, `group/pax.rs`).
    pub fn fitting_together(columns: &[(&Fields<'_>, &DataType)], from: usize, (capacity, compress): (usize, bool)) -> usize {
        let mut sizers: Vec<Sizer<'_>> = columns.iter().map(|&(f, t)| f.sizer(t, compress)).collect();
        let end = columns.iter().map(|(f, _)| f.len()).min().unwrap_or(0);
        for n in from..end {
            let mut size = 0_usize;
            for (&(f, _), sizer) in columns.iter().zip(&mut sizers) {
                sizer.add(f.field(n));
                size = size.saturating_add(sizer.best().1);
            }
            if size > capacity {
                return n.saturating_sub(from);
            }
        }
        end.saturating_sub(from)
    }

    /// The chunk of those from `from`, `count` of them, of type `t`, in the
    /// encoding that takes fewest bytes, compressed if `compress`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is longer than a count describes; -916 if those
    /// asked for are not all there.
    pub fn chunk(&self, t: &DataType, from: usize, (count, compress): (usize, bool)) -> Result<Vec<u8>> {
        self.chunk_planned(t, (from, count), (compress, None))
    }

    /// The chunks of those from `from` on, each as many as fit one of
    /// `capacity` bytes, of type `t`, compressed if `compress`, with where
    /// each begins and how many it holds: each sized once (RULES.md D1144).
    ///
    /// # Errors
    ///
    /// As `chunk`.
    pub fn chunks(&self, t: &DataType, (capacity, compress): (usize, bool)) -> Result<Vec<(usize, usize, Vec<u8>)>> {
        let mut made = Vec::new();
        let mut from = 0_usize;
        while from < self.len() {
            let (count, plan) = self.fitting_planned(t, from, (capacity, compress));
            made.push((from, count, self.chunk_planned(t, (from, count), (compress, plan))?));
            from = from.saturating_add(count);
        }
        Ok(made)
    }

    /// As `chunk`, in `plan` if given, as a sizer of the same values chose it.
    fn chunk_planned(&self, t: &DataType, (from, count): (usize, usize), (compress, plan): (bool, Option<Plan>)) -> Result<Vec<u8>> {
        let end = from.checked_add(count).ok_or_else(too_large)?;
        if end > self.len() {
            return Err(corrupt("its values are out of range"));
        }
        let cells = self.cells(from, count);
        let mut sizer = self.sizer(t, compress);
        let plan = plan.unwrap_or_else(|| {
            for f in cells.iter() {
                sizer.add(f);
            }
            sizer.best().0
        });
        let mut out = vec![encoded::kind_of(plan)];
        out.extend_from_slice(&word(count)?);
        let mut nulls = vec![0_u8; nulls_len(count)];
        for (n, field) in cells.iter().enumerate() {
            let (byte, bit) = bit_of(n);
            if let Some(b) = nulls.get_mut(byte).filter(|_| field.is_none()) {
                *b |= 1 << bit;
            }
        }
        out.extend_from_slice(&nulls);
        // A dictionary of texts in order, so a range is a range of codes.
        encoded::encode(plan, cells, (sizer.table(), padding(t).is_some()), &mut out)?;
        Ok(out)
    }

    /// Of texts of a VARCHAR type `t`, compressed if `compress`, the table
    /// of symbols chosen of them all once (`fsst.rs`), each chunk then
    /// sized and written by it.
    fn table(&self, t: &DataType, compress: bool) -> Option<&fsst::Table> {
        let (DataType::Varying { .. }, true) = (t, compress) else { return None };
        self.table.get_or_init(|| Some(fsst::Table::chosen(&Cells::whole(&self.records).iter().flatten().collect::<Vec<_>>()))).as_ref()
    }
}
