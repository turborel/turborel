//! \[DESIGN\] The catalog's descriptions in bytes (RULES.md D25): what a
//! checkpoint's image records of the definitions as they are, so a
//! database opens from them, not from every definition ever made
//! (`db/snapshot.rs`). Fields are written through `bytes.rs`, values as
//! in `value/codec.rs`.
//!
//! ```text
//! name:       auth (str), ident (str)
//! positions:  count u32, each u32
//! type:       a tag u8 of its kind, then its parameters (catalog/codec/types.rs)
//! default:    0 none | 1 value | 2 the user | 3 a datetime value function,
//!             0 date, 1 time or 2 timestamp u8, precision u32
//! column:     name (str), type, not null u8, default, domain (0, or 1, a
//!             name and its default), collation (0, or 1 and a name),
//!             generated (0 none; 1 identity, always u8, a sequence
//!             generator; 2 generation expression, text (str), names (str))
//! table:      name, columns (count u32, each a column), constraints (count
//!             u32, each a constraint), engine (str), options (count u32,
//!             each str and value), generation u32, temporary u8 (0
//!             persistent; 1 global, 2 created local or 3 declared, then
//!             on commit delete rows u8)
//! constraint: name, kind, deferrable u8, initially deferred u8, domain (0,
//!             or 1 and the name of the domain it is a copy of)
//! kind:       0 not null, position u32 | 1 unique, positions, primary u8,
//!             nulls distinct u8
//!             | 2 foreign key, positions, table name, positions, match u8
//!             (0 simple, 1 partial, 2 full), on update u8, on delete u8 (0
//!             cascade, 1 set null, 2 set default, 3 no action) | 3 check,
//!             text (str), names (str), reads (count u32, names), horizon
//! horizon:    count u32, each a name and its degree u32
//! domain:     name, type, default, constraints (count u32, each a
//!             constraint), usage (count u32, each grantor (str), grantee
//!             (0 PUBLIC, or 1 and str), grantable u8)
//! collation, sequence generator and user-defined type:
//!             catalog/codec/objects.rs; trigger: catalog/codec/triggers.rs;
//!             SQL-invoked routine: catalog/codec/routines.rs; the column
//!             is written in catalog/codec/columns.rs
//! view:       name, columns, query (str), names (str), check option u8 (0
//!             none, 1 cascaded, 2 local), grouped u8, updates (0, or 1, a
//!             name and positions), reads (count u32, names), horizon,
//!             underlying (0, or 1, leaves (count u32, names), and for each
//!             column (count u32) the columns it is drawn from (count u32,
//!             each a name and str))
//! index:      name (str), table name, positions, method (str), unique u8,
//!             options
//! spatial reference systems and geometry columns:
//!             catalog/codec/spatial.rs
//! privilege:  grantor (str), grantee (0 PUBLIC, or 1 and str), table name,
//!             select u8, delete u8, insert positions, update positions,
//!             references positions, grantable u8
//! ```

use std::collections::BTreeSet;

use super::{
    Actions, CheckLevels, ConstraintAttributes, ConstraintDesc, ConstraintKind, DomainDesc, Horizon, MatchType, PeriodDesc, PrivilegeDesc,
    QualifiedName, ReferentialAction, TableDesc, Temporary, TemporaryKind, TypedActions, Underlying, UsageDesc, ViewDesc,
};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::codec::{put_value, value};

mod columns;
mod foreign;
mod indexes;
mod objects;
mod roles;
mod routines;
mod spatial;
mod structured;
mod triggers;
use crate::value::types::tags::{data_type, put_type};
use columns::{collation_name, column, put_collation_name, put_column};
pub(crate) use columns::{default, put_default};
pub(crate) use indexes::{index, put_index};
pub(crate) use objects::{collation, put_collation, put_sequence, put_user_type, sequence, user_type};
pub(crate) use roles::{put_roles, roles};
pub(crate) use routines::{module, put_module, put_routine, routine};
pub(crate) use spatial::{put_spatial, put_spatial_change, spatial, spatial_change};
pub(crate) use structured::{cast, ordering, put_cast, put_ordering, put_structured_type, put_transforms, structured_type, transforms};
pub(crate) use triggers::{put_trigger, trigger};

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

pub(crate) fn put_name(out: &mut Vec<u8>, name: &QualifiedName) {
    out.put_str(&name.auth);
    out.put_str(&name.ident);
}

pub(crate) fn qualified(r: &mut Reader<'_>) -> Result<QualifiedName> {
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

/// A horizon: the columns seen of each table, then whether the subject
/// routines are fixed, and if so their specific names.
pub(super) fn put_horizon(out: &mut Vec<u8>, horizon: &Horizon) {
    put_all(out, horizon.widths.iter(), |out, (table, degree)| {
        put_name(out, table);
        out.put_u32(count(*degree));
    });
    put_flag(out, horizon.routines.is_some());
    if let Some(routines) = &horizon.routines {
        put_all(out, routines.iter(), put_name);
    }
}

/// What `put_horizon` wrote.
pub(super) fn horizon(r: &mut Reader<'_>) -> Result<Horizon> {
    let widths = get_all(r, |r| Ok((qualified(r)?, position(r)?)))?;
    let routines = if flag(r)? { Some(get_all(r, qualified)?) } else { None };
    Ok(Horizon { widths, routines })
}

/// The tag of `action`, and the action of a tag.
const ACTIONS: [ReferentialAction; 5] = [
    ReferentialAction::Cascade,
    ReferentialAction::SetNull,
    ReferentialAction::SetDefault,
    ReferentialAction::NoAction,
    ReferentialAction::Restrict,
];
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
        ConstraintKind::Unique { columns, primary, nulls_distinct, period } => {
            out.put_u8(1);
            put_positions(out, columns.iter());
            put_flag(out, *primary);
            put_flag(out, *nulls_distinct);
            triggers::put_optional(out, period.as_deref());
        }
        ConstraintKind::ForeignKey(fk) => {
            out.put_u8(2);
            foreign::put_foreign_key(out, fk);
        }
        ConstraintKind::Check { text, names, reads, horizon } => {
            out.put_u8(3);
            out.put_str(text);
            out.put_str(names);
            put_all(out, reads.iter(), put_name);
            put_horizon(out, horizon);
        }
    }
}

fn kind(r: &mut Reader<'_>) -> Result<ConstraintKind> {
    Ok(match r.u8()? {
        0 => ConstraintKind::NotNull(position(r)?),
        1 => {
            let (columns, primary, nulls_distinct) = (positions(r)?, flag(r)?, flag(r)?);
            ConstraintKind::Unique { columns, primary, nulls_distinct, period: triggers::optional(r)? }
        }
        2 => ConstraintKind::ForeignKey(foreign::foreign_key(r)?),
        3 => ConstraintKind::Check { text: r.str()?, names: r.str()?, reads: get_all(r, qualified)?, horizon: horizon(r)? },
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
    structured::put_typed(out, table.typed.as_ref());
    // [ISO/IEC 9075-2:2023 §4.16.1] each period descriptor.
    put_all(out, table.periods.iter(), |out, p| {
        out.put_str(&p.name);
        out.put_str(&p.start);
        out.put_str(&p.end);
        put_all(out, p.constraint.iter(), put_name);
    });
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
    let typed = structured::typed(r)?;
    let periods =
        get_all(r, |r| Ok(PeriodDesc { name: r.str()?, start: r.str()?, end: r.str()?, constraint: get_all(r, qualified)?.pop() }))?;
    Ok(TableDesc { name, columns, constraints, engine, options, generation, temporary, typed, periods })
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
    put_type(out, &domain.data_type.clone());
    put_default(out, domain.default.as_ref());
    put_all(out, domain.constraints.iter(), put_constraint);
    put_usage(out, &domain.usage);
    put_collation_name(out, domain.collation.as_ref());
}

/// The domain `put_domain` wrote.
pub(crate) fn domain(r: &mut Reader<'_>) -> Result<DomainDesc> {
    let (name, data_type, default) = (qualified(r)?, data_type(r)?, default(r)?);
    let constraints = get_all(r, constraint)?;
    let usage = usage(r)?;
    Ok(DomainDesc { name, data_type, default, constraints, usage, collation: collation_name(r)? })
}

/// Each USAGE grant, its grantor, grantee (0, or 1 then the name; none is
/// PUBLIC) and grant option.
fn put_usage(out: &mut Vec<u8>, usage: &[UsageDesc]) {
    put_all(out, usage.iter(), |out, u| {
        out.put_str(&u.grantor);
        put_flag(out, u.grantee.is_some());
        if let Some(grantee) = &u.grantee {
            out.put_str(grantee);
        }
        put_flag(out, u.grantable);
    });
}

/// The grants `put_usage` wrote.
fn usage(r: &mut Reader<'_>) -> Result<Vec<UsageDesc>> {
    get_all(r, |r| {
        let grantor = r.str()?;
        let grantee = if flag(r)? { Some(r.str()?) } else { None };
        Ok(UsageDesc { grantor, grantee, grantable: flag(r)? })
    })
}

/// `view` in bytes.
pub(crate) fn put_view(out: &mut Vec<u8>, view: &ViewDesc) {
    put_name(out, &view.name);
    put_all(out, view.columns.iter(), put_column);
    out.put_str(&view.query);
    out.put_str(&view.names);
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
    structured::put_typed(out, view.typed.as_ref());
}

/// The view `put_view` wrote.
pub(crate) fn view(r: &mut Reader<'_>) -> Result<ViewDesc> {
    let (name, columns, query, names) = (qualified(r)?, get_all(r, column)?, r.str()?, r.str()?);
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
    let typed = structured::typed(r)?;
    Ok(ViewDesc { name, columns, query, names, check_option, grouped, updates, reads, horizon, typed, underlying })
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
    put_flag(out, a.trigger);
    put_flag(out, a.typed.hierarchy);
    put_all(out, a.typed.methods.iter(), put_name);
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
    let (trigger, hierarchy) = (flag(r)?, flag(r)?);
    let methods = get_all(r, qualified)?.into_iter().collect();
    let actions = Actions { select, insert, delete, update, references, trigger, typed: TypedActions { hierarchy, methods } };
    Ok(PrivilegeDesc { grantor, grantee, table, actions, grantable: flag(r)? })
}

// Unit tests: what is not a description is refused (RULES.md R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ColumnDesc, Generated, Generation, Identity, SequenceDesc};
    use crate::error::Code;
    use crate::value::{DataType, Spelled};

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
                    put_type(out, &DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric });
                    put_flag(out, false);
                },
                &[tag, 0, 0, 0],
            )
        };
        assert_eq!(refused(&column_with(9), column), corrupt, "a default");
        assert!(column(&mut Reader::new(&column_with(0), "column")).is_ok(), "no default, of no domain, collation or generation");
        let foreign_key = |action: u8| {
            bytes(
                |out| {
                    out.put_u8(2);
                    put_positions(out, [0].iter());
                    put_name(out, &QualifiedName { auth: "A".into(), ident: "T".into() });
                    put_positions(out, [0].iter());
                },
                &[0, action, 0, 0, 0],
            )
        };
        assert_eq!(refused(&foreign_key(9), kind), corrupt, "an action");
        assert!(kind(&mut Reader::new(&foreign_key(4), "kind")).is_ok(), "RESTRICT");
        let mut alone = foreign_key(3);
        alone.truncate(alone.len().saturating_sub(2));
        triggers::put_optional(&mut alone, Some("P"));
        triggers::put_optional(&mut alone, None);
        assert_eq!(refused(&alone, kind), corrupt, "a foreign key of one period");
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

    /// Verifies: [DESIGN] RULES.md D25, D63: how a column's values are
    /// generated reads back as written, of an identity column with its
    /// sequence generator, of a generated column, and of a system-generated
    /// and a derived self-referencing column; another tag is SQLCODE -916.
    #[test]
    fn a_columns_generation_reads_back() {
        let t = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
        let sequence = SequenceDesc {
            name: QualifiedName { auth: "A".into(), ident: "T".into() },
            data_type: t.clone(),
            start: 1,
            minimum: 1,
            maximum: 99,
            increment: 2,
            cycle: true,
            epoch: 7,
            origin: 5,
            usage: Vec::new(),
        };
        let system = Generated::SystemReference(sequence.clone());
        let derived = Generated::DerivedReference(vec!["X".into(), "Y".into()]);
        let identity = Generated::Identity(Identity { always: true, sequence });
        let horizon = Horizon::default().invoking(vec![QualifiedName { auth: "A".into(), ident: "F_1".into() }]);
        let expression = Generated::Expression(Generation { text: "K + 1".into(), names: "A".into(), horizon, asynchronous: false });
        for generated in [None, Some(identity), Some(expression), Some(system), Some(derived)] {
            let c = ColumnDesc {
                name: "C".into(),
                data_type: t.clone(),
                not_null: true,
                default: None,
                domain: None,
                collation: None,
                generated,
            };
            let mut out = Vec::new();
            put_column(&mut out, &c);
            assert_eq!(column(&mut Reader::new(&out, "column")).unwrap().generated, c.generated);
            let mut bad = out.clone();
            if let Some(tag) = bad.iter_mut().rev().find(|_| c.generated.is_none()) {
                *tag = 9;
                assert_eq!(refused(&bad, column), Some(Code::Corrupt));
            }
        }
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
                periods: Vec::new(),
                typed: None,
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
