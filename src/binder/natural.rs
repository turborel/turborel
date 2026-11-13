//! [ISO/IEC 9075:1992 §7.5 SR6, SR9, GR1d] The corresponding join columns of
//! a NATURAL or USING join: their equalities and the common columns COALESCE
//! gives. See `from.rs`.

use super::case::set_type;
use super::collation::of_join_columns;
use super::from::{From, FromItem, Tree, column_name, columns};
use super::rule;
use super::views::Source;
use crate::ast::{JoinKind, JoinedTable};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Cond, Expr, and_all};
use crate::value::collation::CollationRef;

/// The condition and common columns of a NATURAL join (`using` none) or a
/// USING join `j` of `left` and `right`, the first starting at `start` and
/// the common columns at `end` [ISO/IEC 9075:1992 §7.5 SR6, SR9, GR1d].
#[expect(clippy::type_complexity, reason = "the condition and the item of the common columns, made together")]
pub(super) fn natural(
    (j, db, session): (&JoinedTable, &State, Option<&CollationRef>),
    using: Option<&[String]>,
    (left, right): (&Tree, &Tree),
    (start, end): (usize, usize),
    from: &mut From,
) -> Result<(Option<Cond>, Option<(usize, Vec<Expr>)>)> {
    let (lcols, rcols) = (columns(left, &from.items), columns(right, &from.items));
    let called = |cols: &[(usize, usize)], name: &str| {
        cols.iter().filter(|&&(i, c)| column_name(&from.items, i, c) == name).copied().collect::<Vec<_>>()
    };
    let names: Vec<String> = match using {
        Some(list) => list.to_vec(),
        None => lcols.iter().map(|&(i, c)| column_name(&from.items, i, c).to_owned()).filter(|n| !called(&rcols, n).is_empty()).collect(),
    };
    let mut equal = Vec::new();
    let mut exprs = Vec::new();
    let mut descs = Vec::new();
    let mut joined = Vec::new();
    for name in &names {
        let (in_left, in_right) = (called(&lcols, name), called(&rcols, name));
        let ([first], [second]) = (in_left.as_slice(), in_right.as_slice()) else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §7.5 SR6a, SR6b] {name} is not the name of exactly one column of each operand")));
        };
        let (cond, coalesced, desc, positions) = corresponding((j.kind, db, session), (&from.items, *first, *second), start)?;
        from.used.extend(positions);
        joined.extend([*first, *second]);
        equal.push(cond);
        exprs.push(coalesced);
        descs.push(desc);
    }
    for (item, column) in joined {
        from.items.get_mut(item).into_iter().for_each(|it| it.common.push(column));
    }
    if descs.is_empty() {
        return Ok((None, None));
    }
    let name = QualifiedName { auth: String::new(), ident: format!("#{}", from.items.len()) };
    let desc = TableDesc { columns: descs, ..super::views::unconstrained(&name) };
    from.items.push(FromItem {
        correlation: Some(name.ident.clone()),
        table: name,
        at: j.at,
        desc,
        offset: end,
        source: Source::Common,
        nullable: false,
        common: Vec::new(),
        drawn: None,
    });
    Ok((and_all(equal), Some((from.items.len().saturating_sub(1), exprs))))
}

/// [ISO/IEC 9075:1992 §7.5 SR6c, SR6d, SR9] A pair of corresponding join
/// columns of a join of `kind`, each an item and a column of it, the join
/// starting at `start`: their equality and COALESCE over the join's
/// columns, its common column's descriptor, and their places in the FROM
/// row.
fn corresponding(
    (kind, db, session): (JoinKind, &State, Option<&CollationRef>),
    (items, (left_item, left_column), (right_item, right_column)): (&[FromItem], (usize, usize), (usize, usize)),
    start: usize,
) -> Result<(Cond, Expr, ColumnDesc, [usize; 2])> {
    let lost = || SqlError::new(Code::Api, "[DESIGN] a join operand lost a column");
    let (first, second) = (items.get(left_item).ok_or_else(lost)?, items.get(right_item).ok_or_else(lost)?);
    let (c1, c2) = (first.desc.columns.get(left_column).ok_or_else(lost)?, second.desc.columns.get(right_column).ok_or_else(lost)?);
    // [SR6c] comparable.
    let common = set_type(&[c1.data_type.clone(), c2.data_type.clone()], "§7.5 SR6c")?;
    let at1 = first.offset.saturating_add(first.position(left_column));
    let at2 = second.offset.saturating_add(second.position(right_column));
    let (local1, local2) = (at1.saturating_sub(start), at2.saturating_sub(start));
    let cast = |e: Expr, own| super::case::brought(e, own, common.clone());
    // [SR6d] COALESCE (TA.C, TB.C).
    let coalesced = Expr::Case(
        vec![(Cond::IsNull(Expr::Column(local1), true), cast(Expr::Column(local1), &c1.data_type.clone()))],
        Box::new(cast(Expr::Column(local2), &c2.data_type.clone())),
    );
    // [SR9] possibly nullable only if an outer join may null it on the side it is.
    let (null1, null2) = (!c1.not_null || first.nullable, !c2.not_null || second.nullable);
    let nullable = (matches!(kind, JoinKind::Right | JoinKind::Full | JoinKind::Union) && null1)
        || (matches!(kind, JoinKind::Left | JoinKind::Full | JoinKind::Union) && null2);
    let shared = c1.collation.clone().filter(|_| c1.collation == c2.collation);
    let desc = ColumnDesc {
        name: c1.name.clone(),
        data_type: common.clone(),
        not_null: !nullable,
        default: None,
        domain: None,
        collation: shared,
        generated: None,
    };
    // [ISO/IEC 9075-2:2023 §9.15] by their collation.
    let collation = of_join_columns(db, session, c1.collation.as_ref(), c2.collation.as_ref())?;
    let compared =
        |e: Expr| collation.as_ref().filter(|_| common.is_character()).map_or_else(|| e.clone(), |c| Expr::sort_key(e.clone(), c));
    let equal = Cond::Compare(compared(Expr::Column(local1)), crate::value::CompOp::Eq, compared(Expr::Column(local2)));
    Ok((equal, coalesced, desc, [at1, at2]))
}
