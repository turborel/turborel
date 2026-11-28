//! \[DESIGN\] A frozen row group written (RULES.md D31, D1148): of rows,
//! or of a load's columns given as records, each column's chunks made of
//! its values written, and its zones, Bloom filters, imprints and sketch of
//! them read where they are (`column.rs`), so either makes the same group.

use std::borrow::Cow;

use super::column::{Column, Decoded, Rowed};
use super::ids::{write_by_id, write_ids};
use super::zones::write_column;
use super::{Group, Pages, Seal, count32, header, pax, put_chain};
use crate::disk::pager::PageId;
use crate::engine::columns::{Fields, imprints, sketch};
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::storage::lanes::Records;
use crate::value::{DataType, Value};

impl Group {
    /// Writes `rows`, whose ids are `ids`, a column of each of `types`, as
    /// a group, in their order.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the rows are not of the types; -900 if there are too
    /// many; an error of the pages; nothing written is kept. Its rows are
    /// in the order of the columns at `order`, those at `bloom` have
    /// Bloom filters, and those at `named` sketches, if they take them.
    pub fn write<S: Seal, R: AsRef<[Value]>>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, rows): (&[RowId], &[R]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
    ) -> Result<Group> {
        let fields = (0..types.len())
            .zip(types)
            .map(|(at, t)| Fields::of(t, rows.iter().map(|r| r.as_ref().get(at).unwrap_or(&Value::Null))))
            .collect::<Result<Vec<_>>>()?;
        let columns: Vec<Rowed<'_, R>> = (0..types.len()).map(|at| Rowed { rows, at }).collect();
        Self::write_from(pages, types, (ids, &fields, &columns), (order, bloom, named))
    }

    /// \[DESIGN\] (RULES.md D1148) As `write`, of rows given by column:
    /// of each of `types`, in order, its values as their type writes them
    /// in a record, each column's chunks made of its records as they are.
    /// The group is the one `write` makes of the same rows, byte for byte.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if there are too many; -910 if there is not a column
    /// of each type, each a value of each id; an error of the pages;
    /// nothing written is kept.
    pub fn write_records<S: Seal>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, records): (&[RowId], &[Records]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
    ) -> Result<Group> {
        if records.len() != types.len() || records.iter().any(|r| r.len() != ids.len()) {
            return Err(SqlError::new(Code::Api, "[DESIGN] a group's columns are not one of each type, a value of each row"));
        }
        let fields: Vec<Fields<'_>> = types.iter().zip(records).map(|(t, r)| Fields::from_records(t, Cow::Borrowed(r))).collect();
        let columns: Vec<Decoded<'_>> = types.iter().zip(records).map(|(t, r)| Decoded::new(t, r)).collect();
        Self::write_from(pages, types, (ids, &fields, &columns), (order, bloom, named))
    }

    /// As `write`, of the values of each column written, `fields`, and
    /// read, `columns`.
    fn write_from<S: Seal, C: Column>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, fields, columns): (&[RowId], &[Fields<'_>], &[C]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
    ) -> Result<Group> {
        let mut written = Vec::new();
        let made = Self::write_all(pages, types, (ids, fields, columns), (order, bloom, named), &mut written);
        if made.is_err() {
            pages.free(&written);
        }
        made
    }

    fn write_all<S: Seal, C: Column>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, fields, values): (&[RowId], &[Fields<'_>], &[C]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
        written: &mut Vec<PageId>,
    ) -> Result<Group> {
        let (Some(&first), Some(&last)) = (ids.iter().min(), ids.iter().max()) else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a frozen row group of no rows"));
        };
        let id_runs = write_ids(pages, ids, written)?;
        let by_id = write_by_id(pages, ids, written)?;
        let mut columns = if pages.pax {
            pax::write_pax(pages, types, (fields, values, bloom), written)?
        } else {
            let each = types.iter().zip(fields).zip(values).enumerate();
            each.map(|(c, ((t, f), v))| write_column(pages, t, (f, v, bloom.contains(&c)), written)).collect::<Result<Vec<_>>>()?
        };
        let edges = imprints::put_all(pages, (types, values), &mut columns, written)?;
        let sketches = sketch::put_all(pages, (types, values, named), &mut columns, written)?;
        // A lengths lane of each VARCHAR column, but in the hybrid layout (RULES.md D1319).
        let lanes = if pages.pax { vec![None; types.len()] } else { super::lanes::write(pages, types, fields, written)? };
        let mut group = Group {
            first,
            last,
            rows: count32(values.first().map_or(ids.len(), Column::len))?,
            ids: id_runs,
            by_id,
            columns: columns.into_iter().map(Some).collect(),
            order: order.to_vec(),
            pax: pages.pax,
            edges,
            sketched: named.to_vec(),
            sketches,
            lanes,
            header: Vec::new(),
            place: (pages.file, pages.base),
        };
        group.header = put_chain(pages, &header::encode(&group, types)?, written)?;
        Ok(group)
    }
}
