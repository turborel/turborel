//! [ISO/IEC 9075-2:2023 §7.13, §6.9] The grouping of a grouped query: by
//! the grouping columns of a primitive `<group by clause>`, or by each of
//! the grouping sets of any other, ROLLUP, CUBE and GROUPING SETS taken to
//! the sets they stand for (`expansion.rs`), all grouped at once
//! (`layout.rs`); and the `<grouping operation>`, GROUPING, 1 of a column
//! a group's set leaves out, else 0.
//!
//! The Syntax Rules make the query of grouping sets the UNION of one
//! grouped query for each set, each column a set leaves out null in it,
//! in set functions' arguments too [SR17 m iii 1 A II], and GROUPING of
//! it 1. The rows of the FROM/WHERE result are read once here, not once
//! for each set: each is made once for each set, so each set's groups are
//! the groups of one grouping, and their rows the same. HAVING is applied
//! to each group, as to each set's; ALL keeps two sets alike apart, and
//! DISTINCT, whose UNION removes their rows alike, keeps one of them, and
//! removes rows alike of any two others where the grouping could leave
//! them apart.

mod expansion;
mod layout;

use std::cell::RefCell;

use self::expansion::{Expansion, Set};
use super::collation::any_of;
use super::query::distinct;
use super::subquery::outer_grouping;
use super::{Binder, Grouping, OutColumn, Scope, rule};
use crate::ast::{GroupBy, ValueExpression};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Unnest, map_expr_columns};
use crate::storage::Row;
use crate::value::{ArithOp, DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The most column references a GROUPING may have: its value is
/// at most 2 to their number, less 1 [§6.9 SR10c], in the
/// `MAX_EXACT_PRECISION` digits of its type (`grouping_type`).
pub const MAX_GROUPING_ARGUMENTS: usize = 59;

/// [ISO/IEC 9075-2:2023 §6.9 SR10c] What GROUPING of all but the last of
/// its column references is multiplied by before the last's is added.
const GROUPING_WEIGHT: i128 = 2;

/// [ISO/IEC 9075-2:2023 §6.9 SR10b] The type of GROUPING: exact numeric of
/// scale 0, of the implementation-defined precision (IV081) BIGINT has.
fn grouping_type() -> DataType {
    DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// The integer `n` as an exact numeric value of scale 0.
fn integer(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

/// [SR17] The grouping sets of a grouped query, laid out (`layout.rs`).
pub(super) struct Sets {
    /// The FROM row's column of each column grouped by, PC [SR17d].
    pcs: Vec<usize>,
    /// Of each, its value of a group, over the grouping's rows.
    values: Vec<Expr>,
    /// Of each, whether a set leaves it out, so its value may be null.
    left_out: Vec<bool>,
    /// The first GROUPING bit among the grouping's columns, one for each
    /// of `pcs`.
    bits: usize,
    /// The arrays the FROM row is made once for each set by, and the names
    /// of their columns.
    collections: Vec<Expr>,
    names: Vec<String>,
    /// The projection after them: their columns, then each PC's value in
    /// its set, then each collation's of a PC grouped by under more than one.
    appended: Vec<Expr>,
    /// [GR2a] The groups of the empty grouping sets.
    seeds: Vec<Row>,
    /// The FROM row's column of each PC, and its value's column, which set
    /// functions' arguments read [SR17 m iii 1 A II].
    masked: Vec<(usize, usize)>,
    /// Of GROUP BY DISTINCT, if its rows alike are to be removed: the
    /// result columns of the PCs' values, in SL2 [SR17l, SR17n].
    sl2: Option<Vec<OutColumn>>,
}

impl Sets {
    /// The value of a group of the column grouped by `pos` of the FROM row.
    pub(super) fn value_of(&self, pos: usize) -> Option<&Expr> {
        self.pcs.iter().position(|&c| c == pos).and_then(|k| self.values.get(k))
    }

    /// The group column of the GROUPING bit of the column grouped by `pos`.
    fn bit_of(&self, pos: usize) -> Option<usize> {
        self.pcs.iter().position(|&c| c == pos).map(|k| self.bits.saturating_add(k))
    }

    /// Whether a set leaves out the column grouped by `pos`, so its value
    /// may be null.
    pub(super) fn left_out(&self, pos: usize) -> bool {
        self.pcs.iter().position(|&c| c == pos).and_then(|k| self.left_out.get(k)).is_some_and(|&l| l)
    }
}

/// The grouping of a query bound by `binder`, grouped by `group_by` if
/// written, else one group [X3.135-1986 §5.23 GR1, §5.25 GR1].
pub(super) fn grouping(binder: &Binder<'_>, group_by: Option<&GroupBy>) -> Result<Grouping> {
    let expansion = match group_by {
        Some(g) => expansion::expand(g)?,
        None => Expansion::Primitive(Vec::new()),
    };
    match expansion {
        Expansion::Primitive(columns) => primitive(binder, &columns),
        Expansion::Sets(sets) => {
            let distinct = group_by.is_some_and(|g| g.distinct);
            let (columns, collations, sets) = layout::laid_out(binder, &sets, distinct)?;
            Ok(Grouping { columns, collations, aggs: RefCell::new(Vec::new()), sets: Some(sets) })
        }
    }
}

/// [SR10, SR11] The grouping of a primitive `<group by clause>` of
/// `columns`.
fn primitive(binder: &Binder<'_>, columns: &Set<'_>) -> Result<Grouping> {
    let mut at = Vec::new();
    let mut collations = Vec::new();
    for g in columns {
        // [X3.135-1986 §5.22 SR2] each grouping column references a column of T,
        // [ISO/IEC 9075-2:2023 §9.12 SR2a] an operand of a grouping operation,
        let (pos, t) = binder.resolve(&g.column)?;
        super::composite::grouping_operand(&t, "GROUP BY").or_at(g.column.at)?;
        at.push(pos);
        // [ISO/IEC 9075-2:2023 §9.15] grouped by its collation.
        collations.push(binder.grouped_by(g)?);
    }
    Ok(Grouping { columns: at, collations: any_of(collations), aggs: RefCell::new(Vec::new()), sets: None })
}

/// [ISO/IEC 9075-2:2023 §7.13 SR17n, §9.12 SR2a] Of GROUP BY DISTINCT of
/// grouping sets, each result column `columns` an operand of the grouping
/// operation its UNION DISTINCT is.
pub(super) fn checked(g: &Grouping, group_by: Option<&GroupBy>, columns: &[OutColumn]) -> Result<()> {
    if g.sets.is_none() || !group_by.is_some_and(|b| b.distinct) {
        return Ok(());
    }
    columns.iter().try_for_each(|c| super::composite::grouping_operand(&c.data_type, "GROUP BY DISTINCT"))
}

/// [X3.135-1986 §5.22, §5.23, §5.25 GR1-GR2] `plan` grouped by `g`, the
/// groups HAVING accepts kept, and the select list `exprs`, of `columns`,
/// applied. Without GROUP BY, or of GROUP BY (), the aggregation gives one
/// group even of no rows: the argument of a non-grouped query's set
/// functions [ISO/IEC 9075:1992 §7.9 GR1a i], the single group HAVING
/// alone makes [ISO/IEC 9075:1992 §7.8 GR1], or T as its only group
/// [ISO/IEC 9075-2:2023 §7.13 GR2a]. A grouped table of 0 groups gives no
/// row, whatever the select list [ISO/IEC 9075:1992 §7.9 GR1b i]: GROUP BY
/// a column of no rows makes none, and HAVING may remove every group, the
/// single one included.
pub(super) fn grouped(
    binder: &Binder<'_>,
    plan: LogicalPlan,
    (g, having): (Grouping, Option<Cond>),
    (exprs, selected): (Vec<Expr>, &[OutColumn]),
) -> LogicalPlan {
    let Grouping { columns: groups, collations, aggs, sets } = g;
    let aggs = aggs.into_inner();
    let Some(sets) = sets else {
        let plan = LogicalPlan::Aggregate { groups, collations, aggs, seeds: Vec::new(), input: Box::new(plan) };
        return LogicalPlan::Project { exprs, input: Box::new(having_of(plan, having)) };
    };
    let Sets { values, bits, collections, names, appended, seeds, masked, sl2, .. } = sets;
    let spread = vec![None; collections.len()];
    let made = LogicalPlan::Unnest(Box::new(Unnest { collections, spread, ordinality: false, outer: None, names, input: plan }));
    let rows = LogicalPlan::Project { exprs: appended, input: Box::new(made) };
    // [SR17 m iii 1 A II] a set function's argument reads each column
    // grouped by as its set has it.
    let value = |c: usize| masked.iter().find(|(pos, _)| *pos == c).map_or(c, |(_, v)| *v);
    let aggs = aggs.into_iter().map(|a| AggCall { arg: a.arg.map(|e| map_expr_columns(e, &value)), ..a }).collect();
    let groups_kept = having_of(LogicalPlan::Aggregate { groups, collations, aggs, seeds, input: Box::new(rows) }, having);
    match sl2 {
        Some(pcs) => union_distinct(binder, groups_kept, (pcs, values, bits), (exprs, selected)),
        None => LogicalPlan::Project { exprs, input: Box::new(groups_kept) },
    }
}

/// [SR17l, SR17n] The select list `exprs`, of `selected`, of the groups
/// `groups_kept`, its rows alike removed as UNION DISTINCT removes them of
/// SL2: the value of each PC, of the result columns `pcs`, its GROUPING bit,
/// the first of which is the group column `bits`, then the select list.
fn union_distinct(
    binder: &Binder<'_>,
    groups_kept: LogicalPlan,
    (pcs, values, bits): (Vec<OutColumn>, Vec<Expr>, usize),
    (exprs, selected): (Vec<Expr>, &[OutColumn]),
) -> LogicalPlan {
    let p = pcs.len();
    let bit = OutColumn { name: None, collation: None, data_type: grouping_type(), not_null: true, origin: None };
    let mut columns = pcs;
    columns.extend(std::iter::repeat_n(bit, p));
    columns.extend(selected.iter().cloned());
    let mut list = values;
    list.extend((0..p).map(|k| Expr::Column(bits.saturating_add(k))));
    let start = list.len();
    list.extend(exprs);
    let projected = LogicalPlan::Project { exprs: list, input: Box::new(groups_kept) };
    let once = distinct(projected, (binder.db, binder.session_collation), &columns);
    LogicalPlan::Project { exprs: (start..columns.len()).map(Expr::Column).collect(), input: Box::new(once) }
}

/// `plan`, its groups those `having` accepts, if written.
fn having_of(plan: LogicalPlan, having: Option<Cond>) -> LogicalPlan {
    match having {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(plan) },
        None => plan,
    }
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.9] GROUPING of the column references `args`
    /// where `scope` says: of the query an enclosing one is, if each is an
    /// outer reference [SR3, SR6b]; else, in a group of this query
    /// [SR7], each a grouping column of it [SR10a], 1 if the group's set
    /// leaves it out, else 0 [§7.13 SR17 m iii], of a primitive GROUP BY
    /// always 0 [§7.13 SR11], twice that of all but the last, plus the
    /// last's [SR10c].
    pub(super) fn grouping_operation(&self, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if args.len() > MAX_GROUPING_ARGUMENTS {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[ISO/IEC 9075-2:2023 §6.9 SR10b, SR10c] GROUPING of more than {MAX_GROUPING_ARGUMENTS} column references exceeds the {MAX_EXACT_PRECISION} digits of its implementation-defined precision"
                ),
            ));
        }
        if let Some(outer) = self.outer {
            let outward = args.iter().map(|a| self.is_outer_argument(a)).collect::<Result<Vec<_>>>()?;
            if outward.iter().all(|&o| o) {
                return outer_grouping(outer, args);
            }
        }
        let g = match scope {
            Scope::Groups(g, _) => g,
            Scope::Rows(why) => return Err(rule(why)),
            Scope::Select => return Err(not_grouping("its query has no grouping column")),
        };
        let mut value: Option<Expr> = None;
        for a in args {
            let bit = self.grouping_bit(a, g)?;
            value = Some(match value {
                None => bit,
                Some(v) => {
                    let doubled = Expr::Arith(Box::new(v), ArithOp::Mul, Box::new(integer(GROUPING_WEIGHT)));
                    Expr::Arith(Box::new(doubled), ArithOp::Add, Box::new(bit))
                }
            });
        }
        Ok((value.unwrap_or_else(|| integer(0)), grouping_type()))
    }

    /// [ISO/IEC 9075-2:2023 §6.9 SR10a, §7.13 SR11, SR17 m iii] GROUPING of
    /// the column reference `a` alone, of a group of `g`.
    fn grouping_bit(&self, a: &ValueExpression, g: &Grouping) -> Result<Expr> {
        let ValueExpression::Column(cs) = a else {
            return Err(SqlError::new(Code::Api, "[DESIGN] GROUPING is read with column references"));
        };
        let pos = match self.local(cs)? {
            Some((pos, _, _)) => pos,
            // An outer reference, among references of this query.
            None => match self.outer {
                Some(_) => return Err(not_grouping(&cs.column)).or_at(cs.at),
                None => self.resolve(cs)?.0,
            },
        };
        let not = || not_grouping(&self.names.get(pos).cloned().unwrap_or_default());
        let bit = match &g.sets {
            Some(sets) => sets.bit_of(pos).map(Expr::Column),
            None => g.columns.contains(&pos).then(|| integer(0)),
        };
        bit.ok_or_else(not).or_at(cs.at)
    }
}

/// [ISO/IEC 9075-2:2023 §6.9 SR10a] The error of an argument of GROUPING,
/// `found`, that is no grouping column of its query.
fn not_grouping(found: &str) -> SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §6.9 SR10a] each argument of GROUPING references a grouping column of its query (found {found})"))
}
