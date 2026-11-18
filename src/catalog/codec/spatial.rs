//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3] The spatial
//! reference systems defined and the geometry columns registered, in
//! bytes, and a change to them, as the log records it.
//!
//! ```text
//! option:  0 none | 1 and the value
//! system:  name (str), id (i32 as u32), organization (option of str and
//!          i32 as u32), definition (str), description (option of str)
//! column:  table name, column (str), system's name (option of str)
//! catalog: systems (count u32, each a system), columns (count u32, each a
//!          column)
//! change:  0 define, a system | 1 drop, name (str) | 2 register, a
//!          column | 3 unregister, table name, column (str)
//! ```

use super::{count, flag, put_flag, put_name, qualified};
use crate::bytes::{Put, Reader};
use crate::catalog::{GeometryColumnDesc, SpatialCatalog, SpatialChange, SpatialRefSysDesc};
use crate::error::{Code, Result, SqlError};

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.put_u32(u32::from_le_bytes(v.to_le_bytes()));
}

fn get_i32(r: &mut Reader<'_>) -> Result<i32> {
    Ok(i32::from_le_bytes(r.u32()?.to_le_bytes()))
}

fn put_text(out: &mut Vec<u8>, v: Option<&str>) {
    put_flag(out, v.is_some());
    if let Some(v) = v {
        out.put_str(v);
    }
}

fn text(r: &mut Reader<'_>) -> Result<Option<String>> {
    if flag(r)? { r.str().map(Some) } else { Ok(None) }
}

fn put_system(out: &mut Vec<u8>, s: &SpatialRefSysDesc) {
    out.put_str(&s.name);
    put_i32(out, s.id);
    put_flag(out, s.organization.is_some());
    if let Some((org, id)) = &s.organization {
        out.put_str(org);
        put_i32(out, *id);
    }
    out.put_str(&s.definition);
    put_text(out, s.description.as_deref());
}

fn system(r: &mut Reader<'_>) -> Result<SpatialRefSysDesc> {
    let (name, id) = (r.str()?, get_i32(r)?);
    let organization = if flag(r)? { Some((r.str()?, get_i32(r)?)) } else { None };
    Ok(SpatialRefSysDesc { name, id, organization, definition: r.str()?, description: text(r)? })
}

fn put_column(out: &mut Vec<u8>, c: &GeometryColumnDesc) {
    put_name(out, &c.table);
    out.put_str(&c.column);
    put_text(out, c.srs.as_deref());
}

fn column(r: &mut Reader<'_>) -> Result<GeometryColumnDesc> {
    Ok(GeometryColumnDesc { table: qualified(r)?, column: r.str()?, srs: text(r)? })
}

/// `spatial` in bytes.
pub(crate) fn put_spatial(out: &mut Vec<u8>, spatial: &SpatialCatalog) {
    out.put_u32(count(spatial.systems().len()));
    spatial.systems().iter().for_each(|s| put_system(out, s));
    out.put_u32(count(spatial.columns().len()));
    spatial.columns().iter().for_each(|c| put_column(out, c));
}

/// The systems and columns `put_spatial` wrote.
pub(crate) fn spatial(r: &mut Reader<'_>) -> Result<SpatialCatalog> {
    let systems = (0..r.u32()?).map(|_| system(r)).collect::<Result<Vec<_>>>()?;
    let columns = (0..r.u32()?).map(|_| column(r)).collect::<Result<Vec<_>>>()?;
    Ok(SpatialCatalog::of(systems, columns))
}

/// `change` in bytes.
pub(crate) fn put_spatial_change(out: &mut Vec<u8>, change: &SpatialChange) {
    match change {
        SpatialChange::Define(s) => {
            out.put_u8(0);
            put_system(out, s);
        }
        SpatialChange::Drop(name) => {
            out.put_u8(1);
            out.put_str(name);
        }
        SpatialChange::Register(c) => {
            out.put_u8(2);
            put_column(out, c);
        }
        SpatialChange::Unregister(table, column) => {
            out.put_u8(3);
            put_name(out, table);
            out.put_str(column);
        }
    }
}

/// The change `put_spatial_change` wrote.
pub(crate) fn spatial_change(r: &mut Reader<'_>) -> Result<SpatialChange> {
    match r.u8()? {
        0 => system(r).map(SpatialChange::Define),
        1 => r.str().map(SpatialChange::Drop),
        2 => column(r).map(SpatialChange::Register),
        3 => Ok(SpatialChange::Unregister(qualified(r)?, r.str()?)),
        tag => Err(SqlError::new(Code::Corrupt, format!("a spatial change of tag {tag}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3]
    /// systems, columns and each change read back as written.
    #[test]
    fn spatial_catalog_reads_back_as_written() {
        let system = SpatialRefSysDesc {
            name: "LOCAL".into(),
            id: -7,
            organization: Some(("ACME".into(), 12_i32)),
            definition: "LOCAL_CS[\"x\"]".into(),
            description: None,
        };
        let column = GeometryColumnDesc { table: QualifiedName { auth: "A".into(), ident: "T".into() }, column: "G".into(), srs: None };
        let catalog = SpatialCatalog::of(vec![system.clone()], vec![column.clone()]);
        let mut out = Vec::new();
        put_spatial(&mut out, &catalog);
        let mut r = Reader::new(&out, "spatial");
        assert_eq!(spatial(&mut r).unwrap(), catalog);
        r.finish().unwrap();
        let changes = [
            SpatialChange::Define(system),
            SpatialChange::Drop("LOCAL".into()),
            SpatialChange::Register(column.clone()),
            SpatialChange::Unregister(column.table, column.column),
        ];
        for change in changes {
            let mut written = Vec::new();
            put_spatial_change(&mut written, &change);
            let mut read = Reader::new(&written, "change");
            assert_eq!(spatial_change(&mut read).unwrap(), change);
            read.finish().unwrap();
        }
        assert!(spatial_change(&mut Reader::new(&[9], "change")).is_err());
    }
}
