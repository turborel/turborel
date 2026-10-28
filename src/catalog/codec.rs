//! \[DESIGN\] The catalog's descriptions in bytes (RULES.md D25): what a
//! checkpoint's image records of the definitions as they are, so a
//! database opens from them, not from every definition ever made
//! (`db/catalog_file.rs`). Fields are written through `bytes.rs`, values
//! and rows as in `value/codec.rs`.
//!
//! ```text
//! name:       auth (str), ident (str)
//! positions:  count u32, each u32
//! type:       0 character, length u32 | 1 exact, precision u32, scale u32
//!             | 2 approximate, precision u32
//! column:     name (str), type, not null u8, default: 0 none | 1 value
//!             | 2 the user
//! table:      name, columns (count u32, each a column), constraints (count
//!             u32, each: name, kind, deferrable u8, initially deferred u8),
//!             engine (str), options (count u32, each str and value),
//!             generation u32
//! kind:       0 not null, position u32 | 1 unique, positions, primary u8
//!             | 2 foreign key, positions, table name, positions, match u8,
//!             on update u8, on delete u8 | 3 check, text (str), reads
//!             (count u32, names), horizon
//! horizon:    count u32, each a name and its degree u32
//! view:       name, columns, query (str), check option u8 (0 none, 1
//!             cascaded, 2 local), grouped u8, updates (0, or 1, a name and
//!             positions), reads (count u32, names), horizon
//! index:      name (str), table name, positions, method (str), unique u8,
//!             options
//! privilege:  grantor (str), grantee (0 PUBLIC, or 1 and str), table name,
//!             select u8, insert u8, delete u8, update positions,
//!             references positions, grantable u8
//! ```

use std::collections::BTreeSet;

use super::{
    Actions, CheckLevels, ColumnDefault, ColumnDesc, ColumnDomain, ConstraintAttributes, ConstraintDesc, ConstraintKind, DomainDesc,
    ForeignKey, Horizon, IndexDesc, MatchType, PrivilegeDesc, QualifiedName, ReferentialAction, TableDesc, Temporary, TemporaryKind,
    Underlying, UsageDesc, ViewDesc,
};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::codec::{put_value, value};
use crate::value::types::{APPROXIMATE, CHARACTER, EXACT};
use crate::value::{DataType, SqlType};

/// A count, or a position, as a u32.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A u32 read as a position.
fn position(r: &mut Reader<'_>) -> Result<usize> {
    Ok(usize::try_from(r.u32()?).unwrap_or(usize::MAX))
}

/// Items written by `put`, after how many.
fn put_all<T>(out: &mut Vec<u8>, items: impl ExactSizeIterator<Item = T>, mut put: impl FnMut(&mut Vec<u8>, T)) {
    out.put_u32(count(items.len()));
    for item in items {
        put(out, item);
    }
}

/// Items read by `get`, after how many.
fn get_all<T>(r: &mut Reader<'_>, mut get: impl FnMut(&mut Reader<'_>) -> Result<T>) -> Result<Vec<T>> {
    (0..r.u32()?).map(|_| get(r)).collect()
}

fn put_name(out: &mut Vec<u8>, name: &QualifiedName) {
    out.put_str(&name.auth);
    out.put_str(&name.ident);
}

fn qualified(r: &mut Reader<'_>) -> Result<QualifiedName> {
    Ok(QualifiedName { auth: r.str()?, ident: r.str()? })
}

fn put_positions<'a>(out: &mut Vec<u8>, positions: impl ExactSizeIterator<Item = &'a usize>) {
    put_all(out, positions, |out, &p| out.put_u32(count(p)));
}

fn positions(r: &mut Reader<'_>) -> Result<Vec<usize>> {
    get_all(r, position)
}

fn put_flag(out: &mut Vec<u8>, flag: bool) {
    out.put_u8(u8::from(flag));
}

fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(r.corrupt("a flag is neither 0 nor 1")),
    }
}

/// The tag of each built-in kind of type, before its parameters.
const CHARACTER_TYPE: u8 = 0;
const EXACT_TYPE: u8 = 1;
const APPROXIMATE_TYPE: u8 = 2;
/// A type beyond the standard: its kind's name, then its parameters.
const EXTENSION_TYPE: u8 = 3;

fn put_type(out: &mut Vec<u8>, data_type: DataType) {
    match data_type {
        DataType::Character { .. } => out.put_u8(CHARACTER_TYPE),
        DataType::Exact { .. } => out.put_u8(EXACT_TYPE),
        DataType::Approximate { .. } => out.put_u8(APPROXIMATE_TYPE),
        DataType::Extension(e) => {
            out.put_u8(EXTENSION_TYPE);
            out.put_str(e.kind.name());
        }
    }
    data_type.kind().put_params(&data_type, out);
}

fn data_type(r: &mut Reader<'_>) -> Result<DataType> {
    let kind: &dyn SqlType = match r.u8()? {
        CHARACTER_TYPE => &CHARACTER,
        EXACT_TYPE => &EXACT,
        APPROXIMATE_TYPE => &APPROXIMATE,
        EXTENSION_TYPE => {
            let name = r.str()?;
            r.kind(&name)?
        }
        _ => return Err(r.corrupt("not a data type")),
    };
    kind.params(r)
}

fn put_column(out: &mut Vec<u8>, column: &ColumnDesc) {
    out.put_str(&column.name);
    put_type(out, column.data_type);
    put_flag(out, column.not_null);
    put_default(out, column.default.as_ref());
    // Its domain, 1 then its name and default, or 0 for none.
    match &column.domain {
        None => out.put_u8(0),
        Some(ColumnDomain { name, default }) => {
            out.put_u8(1);
            put_name(out, name);
            put_default(out, default.as_ref());
        }
    }
}

/// A default: 0 for none, 1 and a value, or 2 for a user's.
pub(crate) fn put_default(out: &mut Vec<u8>, default: Option<&ColumnDefault>) {
    match default {
        None => out.put_u8(0),
        Some(ColumnDefault::Value(v)) => {
            out.put_u8(1);
            put_value(out, v);
        }
        Some(ColumnDefault::User) => out.put_u8(2),
    }
}

/// The default `put_default` wrote.
pub(crate) fn default(r: &mut Reader<'_>) -> Result<Option<ColumnDefault>> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(ColumnDefault::Value(value(r)?)),
        2 => Some(ColumnDefault::User),
        _ => return Err(r.corrupt("not a column default")),
    })
}

fn column(r: &mut Reader<'_>) -> Result<ColumnDesc> {
    let (name, data_type, not_null) = (r.str()?, data_type(r)?, flag(r)?);
    let own = default(r)?;
    let domain = if flag(r)? { Some(ColumnDomain { name: qualified(r)?, default: default(r)? }) } else { None };
    Ok(ColumnDesc { name, data_type, not_null, default: own, domain })
}

fn put_horizon(out: &mut Vec<u8>, horizon: &Horizon) {
    put_all(out, horizon.0.iter(), |out, (table, degree)| {
        put_name(out, table);
        out.put_u32(count(*degree));
    });
}

fn horizon(r: &mut Reader<'_>) -> Result<Horizon> {
    Ok(Horizon(get_all(r, |r| Ok((qualified(r)?, position(r)?)))?))
}

/// The tag of `action`, and the action of a tag.
const ACTIONS: [ReferentialAction; 4] =
    [ReferentialAction::Cascade, ReferentialAction::SetNull, ReferentialAction::SetDefault, ReferentialAction::NoAction];
const MATCHES: [MatchType; 3] = [MatchType::Simple, MatchType::Partial, MatchType::Full];

fn tag_of<T: PartialEq>(all: &[T], item: &T) -> u8 {
    u8::try_from(all.iter().position(|a| a == item).unwrap_or_default()).unwrap_or_default()
}

fn of_tag<T: Copy>(r: &Reader<'_>, all: &[T], tag: u8) -> Result<T> {
    all.get(usize::from(tag)).copied().ok_or_else(|| r.corrupt("not a known tag"))
}

fn put_kind(out: &mut Vec<u8>, kind: &ConstraintKind) {
    match kind {
        ConstraintKind::NotNull(at) => {
            out.put_u8(0);
            out.put_u32(count(*at));
        }
        ConstraintKind::Unique { columns, primary } => {
            out.put_u8(1);
            put_positions(out, columns.iter());
            put_flag(out, *primary);
        }
        ConstraintKind::ForeignKey(fk) => {
            out.put_u8(2);
            put_positions(out, fk.columns.iter());
            put_name(out, &fk.table);
            put_positions(out, fk.referenced.iter());
            out.put_u8(tag_of(&MATCHES, &fk.match_type));
            out.put_u8(tag_of(&ACTIONS, &fk.on_update));
            out.put_u8(tag_of(&ACTIONS, &fk.on_delete));
        }
        ConstraintKind::Check { text, reads, horizon } => {
            out.put_u8(3);
            out.put_str(text);
            put_all(out, reads.iter(), put_name);
            put_horizon(out, horizon);
        }
    }
}

fn kind(r: &mut Reader<'_>) -> Result<ConstraintKind> {
    Ok(match r.u8()? {
        0 => ConstraintKind::NotNull(position(r)?),
        1 => ConstraintKind::Unique { columns: positions(r)?, primary: flag(r)? },
        2 => {
            let (columns, table, referenced) = (positions(r)?, qualified(r)?, positions(r)?);
            let (m, u, d) = (r.u8()?, r.u8()?, r.u8()?);
            let (match_type, on_update, on_delete) = (of_tag(r, &MATCHES, m)?, of_tag(r, &ACTIONS, u)?, of_tag(r, &ACTIONS, d)?);
            ConstraintKind::ForeignKey(ForeignKey { columns, table, referenced, match_type, on_update, on_delete })
        }
        3 => ConstraintKind::Check { text: r.str()?, reads: get_all(r, qualified)?, horizon: horizon(r)? },
        _ => return Err(r.corrupt("not a kind of constraint")),
    })
}

fn put_options(out: &mut Vec<u8>, options: &[(String, crate::value::Value)]) {
    put_all(out, options.iter(), |out, (name, v)| {
        out.put_str(name);
        put_value(out, v);
    });
}

fn options(r: &mut Reader<'_>) -> Result<Vec<(String, crate::value::Value)>> {
    get_all(r, |r| Ok((r.str()?, value(r)?)))
}

/// `table` in bytes.
pub(crate) fn put_table(out: &mut Vec<u8>, table: &TableDesc) {
    put_name(out, &table.name);
    put_all(out, table.columns.iter(), put_column);
    put_all(out, table.constraints.iter(), put_constraint);
    out.put_str(&table.engine);
    put_options(out, &table.options);
    out.put_u32(table.generation);
    // [ISO/IEC 9075:1992 §4.9] 0 persistent; else its kind, 1 global, 2
    // created local, 3 declared, and 1 for ON COMMIT DELETE ROWS.
    match table.temporary {
        None => out.put_u8(0),
        Some(Temporary { kind, delete_rows }) => {
            out.put_u8(match kind {
                TemporaryKind::Global => 1,
                TemporaryKind::CreatedLocal => 2,
                TemporaryKind::Declared => 3,
            });
            put_flag(out, delete_rows);
        }
    }
}

/// The table `put_table` wrote.
pub(crate) fn table(r: &mut Reader<'_>) -> Result<TableDesc> {
    let (name, columns) = (qualified(r)?, get_all(r, column)?);
    let constraints = get_all(r, constraint)?;
    let (engine, options, generation) = (r.str()?, options(r)?, r.u32()?);
    let kind = match r.u8()? {
        0 => None,
        1 => Some(TemporaryKind::Global),
        2 => Some(TemporaryKind::CreatedLocal),
        3 => Some(TemporaryKind::Declared),
        _ => return Err(r.corrupt("not a kind of table")),
    };
    let temporary = kind.map(|kind| flag(r).map(|delete_rows| Temporary { kind, delete_rows })).transpose()?;
    Ok(TableDesc { name, columns, constraints, engine, options, generation, temporary })
}

/// A constraint: its name, kind and attributes, and the domain constraint
/// it is a copy of, if one.
pub(crate) fn put_constraint(out: &mut Vec<u8>, c: &ConstraintDesc) {
    put_name(out, &c.name);
    put_kind(out, &c.kind);
    put_flag(out, c.attributes.deferrable);
    put_flag(out, c.attributes.initially_deferred);
    put_flag(out, c.domain.is_some());
    if let Some(domain) = &c.domain {
        put_name(out, domain);
    }
}

/// The constraint `put_constraint` wrote.
pub(crate) fn constraint(r: &mut Reader<'_>) -> Result<ConstraintDesc> {
    let (name, kind) = (qualified(r)?, kind(r)?);
    let attributes = ConstraintAttributes { deferrable: flag(r)?, initially_deferred: flag(r)? };
    let domain = if flag(r)? { Some(qualified(r)?) } else { None };
    Ok(ConstraintDesc { name, kind, attributes, domain })
}

/// [ISO/IEC 9075:1992 §11.21] `domain` in bytes: its name, type and
/// default, its constraints, and its USAGE grants, each grantor, grantee
/// (0, or 1 then the name; none is PUBLIC) and grant option.
pub(crate) fn put_domain(out: &mut Vec<u8>, domain: &DomainDesc) {
    put_name(out, &domain.name);
    put_type(out, domain.data_type);
    put_default(out, domain.default.as_ref());
    put_all(out, domain.constraints.iter(), put_constraint);
    put_all(out, domain.usage.iter(), |out, u| {
        out.put_str(&u.grantor);
        put_flag(out, u.grantee.is_some());
        if let Some(grantee) = &u.grantee {
            out.put_str(grantee);
        }
        put_flag(out, u.grantable);
    });
}

/// The domain `put_domain` wrote.
pub(crate) fn domain(r: &mut Reader<'_>) -> Result<DomainDesc> {
    let (name, data_type, default) = (qualified(r)?, data_type(r)?, default(r)?);
    let constraints = get_all(r, constraint)?;
    let usage = get_all(r, |r| {
        let grantor = r.str()?;
        let grantee = if flag(r)? { Some(r.str()?) } else { None };
        Ok(UsageDesc { grantor, grantee, grantable: flag(r)? })
    })?;
    Ok(DomainDesc { name, data_type, default, constraints, usage })
}

/// `view` in bytes.
pub(crate) fn put_view(out: &mut Vec<u8>, view: &ViewDesc) {
    put_name(out, &view.name);
    put_all(out, view.columns.iter(), put_column);
    out.put_str(&view.query);
    out.put_u8(match view.check_option {
        None => 0,
        Some(CheckLevels::Cascaded) => 1,
        Some(CheckLevels::Local) => 2,
    });
    put_flag(out, view.grouped);
    match &view.updates {
        None => out.put_u8(0),
        Some((table, columns)) => {
            out.put_u8(1);
            put_name(out, table);
            put_positions(out, columns.iter());
        }
    }
    put_all(out, view.reads.iter(), put_name);
    put_horizon(out, &view.horizon);
    match &view.underlying {
        None => out.put_u8(0),
        Some(u) => {
            out.put_u8(1);
            put_all(out, u.leaves.iter(), put_name);
            put_all(out, u.columns.iter(), |out, drawn| {
                put_all(out, drawn.iter(), |out, (table, column)| {
                    put_name(out, table);
                    out.put_str(column);
                });
            });
        }
    }
}

/// The view `put_view` wrote.
pub(crate) fn view(r: &mut Reader<'_>) -> Result<ViewDesc> {
    let (name, columns, query) = (qualified(r)?, get_all(r, column)?, r.str()?);
    let check_option = match r.u8()? {
        0 => None,
        1 => Some(CheckLevels::Cascaded),
        2 => Some(CheckLevels::Local),
        _ => return Err(r.corrupt("not a check option")),
    };
    let grouped = flag(r)?;
    let updates = if flag(r)? { Some((qualified(r)?, positions(r)?)) } else { None };
    let (reads, horizon) = (get_all(r, qualified)?, horizon(r)?);
    let underlying = if flag(r)? {
        let leaves = get_all(r, qualified)?;
        let drawn = get_all(r, |r| get_all(r, |r| Ok((qualified(r)?, r.str()?))))?;
        Some(Underlying { leaves, columns: drawn })
    } else {
        None
    };
    Ok(ViewDesc { name, columns, query, check_option, grouped, updates, reads, horizon, underlying })
}

/// `index` in bytes.
pub(crate) fn put_index(out: &mut Vec<u8>, index: &IndexDesc) {
    out.put_str(&index.name);
    put_name(out, &index.table);
    put_positions(out, index.columns.iter());
    out.put_str(&index.method);
    put_flag(out, index.unique);
    put_options(out, &index.options);
}

/// The index `put_index` wrote.
pub(crate) fn index(r: &mut Reader<'_>) -> Result<IndexDesc> {
    Ok(IndexDesc { name: r.str()?, table: qualified(r)?, columns: positions(r)?, method: r.str()?, unique: flag(r)?, options: options(r)? })
}

/// `privilege` in bytes.
pub(crate) fn put_privilege(out: &mut Vec<u8>, privilege: &PrivilegeDesc) {
    out.put_str(&privilege.grantor);
    match &privilege.grantee {
        None => out.put_u8(0),
        Some(grantee) => {
            out.put_u8(1);
            out.put_str(grantee);
        }
    }
    put_name(out, &privilege.table);
    let a = &privilege.actions;
    put_flag(out, a.select);
    put_flag(out, a.delete);
    put_positions(out, a.insert.iter());
    put_positions(out, a.update.iter());
    put_positions(out, a.references.iter());
    put_flag(out, privilege.grantable);
}

/// The privilege `put_privilege` wrote.
pub(crate) fn privilege(r: &mut Reader<'_>) -> Result<PrivilegeDesc> {
    let grantor = r.str()?;
    let grantee = if flag(r)? { Some(r.str()?) } else { None };
    let table = qualified(r)?;
    let (select, delete) = (flag(r)?, flag(r)?);
    let mut columns = || positions(r).map(|p| p.into_iter().collect::<BTreeSet<usize>>());
    let (insert, update, references) = (columns()?, columns()?, columns()?);
    let actions = Actions { select, insert, delete, update, references };
    Ok(PrivilegeDesc { grantor, grantee, table, actions, grantable: flag(r)? })
}

// Unit tests: what is not a description is refused (RULES.md R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    /// Bytes of `fill`, then `tail`.
    fn bytes(fill: impl FnOnce(&mut Vec<u8>), tail: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        fill(&mut out);
        out.extend_from_slice(tail);
        out
    }

    /// The code of reading `bytes` with `read`, refused.
    fn refused<T>(bytes: &[u8], read: impl FnOnce(&mut Reader<'_>) -> Result<T>) -> Option<Code> {
        read(&mut Reader::new(bytes, "description")).err().map(|e| e.code)
    }

    /// Verifies: [DESIGN] RULES.md D25, R-ERR-1: a description whose tag
    /// names no type, default, flag, constraint, referential action or
    /// check option is SQLCODE -916, never misread.
    #[test]
    fn unknown_tags_are_corrupt() {
        let corrupt = Some(Code::Corrupt);
        assert_eq!(refused(&[9], data_type), corrupt, "a type");
        assert_eq!(refused(&[2], flag), corrupt, "a flag");
        assert_eq!(refused(&[9], kind), corrupt, "a kind of constraint");
        let column_with = |tag: u8| {
            bytes(
                |out| {
                    out.put_str("C");
                    put_type(out, DataType::Exact { precision: 9, scale: 0 });
                    put_flag(out, false);
                },
                &[tag, 0],
            )
        };
        assert_eq!(refused(&column_with(9), column), corrupt, "a default");
        assert!(column(&mut Reader::new(&column_with(0), "column")).is_ok(), "no default, of no domain");
        let foreign_key = |action: u8| {
            bytes(
                |out| {
                    out.put_u8(2);
                    put_positions(out, [0].iter());
                    put_name(out, &QualifiedName { auth: "A".into(), ident: "T".into() });
                    put_positions(out, [0].iter());
                },
                &[0, action, 0],
            )
        };
        assert_eq!(refused(&foreign_key(9), kind), corrupt, "an action");
        assert!(kind(&mut Reader::new(&foreign_key(3), "kind")).is_ok());
        let view_with = |option: u8| {
            bytes(
                |out| {
                    put_name(out, &QualifiedName { auth: "A".into(), ident: "V".into() });
                    out.put_u32(0);
                    out.put_str("SELECT 1");
                },
                &[option],
            )
        };
        assert_eq!(refused(&view_with(9), view), corrupt, "a check option");
    }

    /// Verifies: [DESIGN] RULES.md D25: a table's kind reads back as written,
    /// persistent or each kind of temporary with its ON COMMIT; a kind of no
    /// table is SQLCODE -916.
    #[test]
    fn a_tables_kind_reads_back() {
        let kinds = [TemporaryKind::Global, TemporaryKind::CreatedLocal, TemporaryKind::Declared];
        let temporary = kinds.into_iter().flat_map(|kind| [true, false].map(|delete_rows| Some(Temporary { kind, delete_rows })));
        for of in std::iter::once(None).chain(temporary) {
            let desc = TableDesc {
                name: QualifiedName { auth: "A".into(), ident: "T".into() },
                columns: Vec::new(),
                constraints: Vec::new(),
                engine: "rowstore".into(),
                options: Vec::new(),
                generation: 7,
                temporary: of,
            };
            let mut out = Vec::new();
            put_table(&mut out, &desc);
            let read = table(&mut Reader::new(&out, "table")).unwrap();
            assert_eq!((read.temporary, read.generation), (of, 7));
            let last = if of.is_some() { out.len() - 2 } else { out.len() - 1 };
            let mut bad = out.clone();
            bad[last] = 9;
            assert_eq!(refused(&bad, table), Some(Code::Corrupt), "{of:?}");
        }
    }
}
