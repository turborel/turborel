//! \[DESIGN\] The grouping sets of a `<group by clause>`, resolved and laid
//! out for one grouping of every set at once, its rows read once. Each row
//! of the FROM/WHERE result is made once for each grouping set by a
//! collection derived table of constant arrays, which give, of each set,
//! its ordinal, a GROUPING bit for each column grouped by, 1 if the set
//! leaves the column out [ISO/IEC 9075-2:2023 §7.13 SR17 m iii], and, of a
//! column grouped by under two collations, whether each is in the set. A
//! projection appends each column's value in its set, null if the set
//! leaves it out [SR17 m iii 1 A II], and each collation's of such a
//! column. The grouping is by those values, each under its collation,
//! and by the ordinal and the flags, which the ordinal decides, so two
//! sets alike are kept apart, as UNION ALL keeps their rows [SR17 n]; and
//! each empty grouping set's group is made before any row, so it is a
//! group even of no rows [GR2a].

use super::Sets;
use super::expansion::Set;
use crate::ast::{GroupingColumn, ValueExpression};
use crate::binder::collation::any_of;
use crate::binder::composite::grouping_operand;
use crate::binder::{Binder, OutColumn};
use crate::error::{At, Result};
use crate::logical::{Cond, Expr};
use crate::storage::Row;
use crate::value::collation::CollationRef;
use crate::value::{CompOp, Decimal, Value};

/// A `<grouping column reference>` resolved: its column of the FROM row,
/// its collation, and whether a `<collate clause>` gave it.
struct Reference {
    pos: usize,
    collation: Option<CollationRef>,
    explicit: bool,
}

/// A column grouped by, PC [SR17d]: its column of the FROM row, the
/// result column its value is in SL2 [SR17l], and its references.
struct Grouped {
    pos: usize,
    column: OutColumn,
    references: Vec<usize>,
}

/// The grouping sets `sets`, resolved: the references, the columns, and
/// each set's references, sorted, a set alike to one before left out if
/// `distinct` [SR17n, UNION DISTINCT].
struct Resolved {
    references: Vec<Reference>,
    columns: Vec<Grouped>,
    members: Vec<Vec<usize>>,
}

/// The integer `n` as an exact numeric value of scale 0.
fn integer(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// [SR1 to SR3] `gc`, resolved: its column of the FROM row, of a type of
/// grouping operands, and the collation it groups by.
fn reference(binder: &Binder<'_>, gc: &GroupingColumn) -> Result<(usize, OutColumn, Reference)> {
    let (pos, data_type) = binder.resolve(&gc.column)?;
    grouping_operand(&data_type, "GROUP BY").or_at(gc.column.at)?;
    let collation = binder.grouped_by(gc)?;
    let named = data_type.is_character().then(|| binder.result_collation(&ValueExpression::Column(gc.column.clone()))).flatten();
    let column = OutColumn { name: Some(gc.column.column.clone()), collation: named, data_type, not_null: false, origin: Some(pos) };
    Ok((pos, column, Reference { pos, collation, explicit: gc.collate.is_some() }))
}

/// The grouping sets `sets`, resolved, alike ones once if `distinct`.
fn resolved(binder: &Binder<'_>, sets: &[Set<'_>], distinct: bool) -> Result<Resolved> {
    let mut r = Resolved { references: Vec::new(), columns: Vec::new(), members: Vec::new() };
    for set in sets {
        let mut of = Vec::new();
        for gc in set {
            let (pos, column, found) = reference(binder, gc)?;
            let j = r.references.iter().position(|g| g.pos == pos && g.collation == found.collation).unwrap_or_else(|| {
                r.references.push(Reference { explicit: false, ..found });
                let j = r.references.len().saturating_sub(1);
                match r.columns.iter_mut().find(|c| c.pos == pos) {
                    Some(c) => c.references.push(j),
                    None => r.columns.push(Grouped { pos, column, references: vec![j] }),
                }
                j
            });
            if let Some(g) = r.references.get_mut(j) {
                g.explicit |= gc.collate.is_some();
            }
            of.push(j);
        }
        of.sort_unstable();
        of.dedup();
        if !(distinct && r.members.contains(&of)) {
            r.members.push(of);
        }
    }
    Ok(r)
}

/// Where the grouping's columns are: in the rows grouped, the first of
/// the unnested ones after the FROM row's `w`, and the first appended; and
/// among the groups' columns.
struct Places {
    w: usize,
    p: usize,
    split: Vec<usize>,
}

impl Places {
    fn ord(&self) -> usize {
        self.w
    }
    fn bit(&self, k: usize) -> usize {
        self.w.saturating_add(1).saturating_add(k)
    }
    fn flag(&self, i: usize) -> usize {
        self.bit(self.p).saturating_add(i)
    }
    fn value(&self, k: usize) -> usize {
        self.flag(self.split.len()).saturating_add(k)
    }
    fn split_key(&self, i: usize) -> usize {
        self.value(self.p).saturating_add(i)
    }
}

/// The grouping of the grouping sets `sets` of a query bound by `binder`,
/// DISTINCT if `distinct`: the columns of the rows grouped it groups by,
/// what each groups by, and the sets.
pub(super) fn laid_out(binder: &Binder<'_>, sets: &[Set<'_>], distinct: bool) -> Result<(Vec<usize>, Vec<Option<CollationRef>>, Sets)> {
    let r = resolved(binder, sets, distinct)?;
    // The references of a column grouped by under more than one collation.
    let split: Vec<usize> = r.columns.iter().filter(|c| c.references.len() > 1).flat_map(|c| c.references.iter().copied()).collect();
    let at = Places { w: binder.names.len(), p: r.columns.len(), split };
    let key = |j: usize| match at.split.iter().position(|&s| s == j) {
        Some(i) => at.split_key(i),
        None => r.columns.iter().position(|c| c.references.contains(&j)).map_or(0, |k| at.value(k)),
    };
    let mut groups: Vec<usize> = (0..r.references.len()).map(key).collect();
    groups.push(at.ord());
    groups.extend((0..at.p).map(|k| at.bit(k)));
    groups.extend((0..at.split.len()).map(|i| at.flag(i)));
    let mut collations: Vec<Option<CollationRef>> = r.references.iter().map(|g| g.collation.clone()).collect();
    collations.resize(groups.len(), None);
    Ok((groups, any_of(collations), sets_of(&r, &at, distinct)))
}

/// The grouping sets `r`, laid out `at`.
fn sets_of(r: &Resolved, at: &Places, distinct: bool) -> Sets {
    let refs = r.references.len();
    let bits = refs.saturating_add(1);
    let in_set = |s: &Vec<usize>, c: &Grouped| c.references.iter().any(|j| s.contains(j));
    let values = r.columns.iter().map(|c| value_of(c, &at.split, bits.saturating_add(at.p))).collect();
    let left_out = r.columns.iter().map(|c| r.members.iter().any(|s| !in_set(s, c))).collect();
    let mut collections = vec![Expr::Const(Value::Array((0..r.members.len()).map(integer).collect()))];
    let mut names = vec!["GROUPING SET".to_owned()];
    for c in &r.columns {
        collections.push(Expr::Const(Value::Array(r.members.iter().map(|s| integer(usize::from(!in_set(s, c)))).collect())));
        names.push(format!("GROUPING({})", c.column.name.as_deref().unwrap_or_default()));
    }
    for &j in &at.split {
        collections.push(Expr::Const(Value::Array(r.members.iter().map(|s| Value::Bool(s.contains(&j))).collect())));
        names.push(format!("GROUPING SET HAS {j}"));
    }
    let mut appended: Vec<Expr> = (0..at.value(0)).map(Expr::Column).collect();
    // [SR17 m iii 1 A II] a column's value, null in a set leaving it out.
    for (k, c) in r.columns.iter().enumerate() {
        let kept = Cond::Compare(Expr::Column(at.bit(k)), CompOp::Eq, Expr::Const(integer(0)));
        appended.push(Expr::Case(vec![(kept, Expr::Column(c.pos))], Box::new(Expr::Const(Value::Null))));
    }
    for (i, &j) in at.split.iter().enumerate() {
        let pos = r.references.get(j).map_or(0, |g| g.pos);
        appended.push(Expr::Case(vec![(Cond::Value(Expr::Column(at.flag(i))), Expr::Column(pos))], Box::new(Expr::Const(Value::Null))));
    }
    let seeds = r.members.iter().enumerate().filter(|(_, s)| s.is_empty()).map(|(n, _)| seed(n, refs, at)).collect();
    let masked = r.columns.iter().enumerate().map(|(k, c)| (c.pos, at.value(k))).collect();
    let pcs = r.columns.iter().map(|c| c.pos).collect();
    let sl2 = (distinct && needs_distinct(r)).then(|| r.columns.iter().map(|c| c.column.clone()).collect());
    Sets { pcs, values, left_out, bits, collections, names, appended, seeds, masked, sl2 }
}

/// The value of the column grouped by `c`, of a group: its reference's
/// group column, or, if it is grouped by under more than one collation
/// (`split`), that of the first of them its set has, the first of whose
/// flags is the group column `flags`.
fn value_of(c: &Grouped, split: &[usize], flags: usize) -> Expr {
    let [only] = c.references.as_slice() else {
        let arm = |&j: &usize| {
            let flag = flags.saturating_add(split.iter().position(|&s| s == j).unwrap_or(0));
            (Cond::Value(Expr::Column(flag)), Expr::Column(j))
        };
        return Expr::Case(c.references.iter().map(arm).collect(), Box::new(Expr::Const(Value::Null)));
    };
    Expr::Column(*only)
}

/// [GR2a] The group of the empty grouping set of ordinal `n`: each
/// reference's value null, the ordinal, each bit 1, each flag false.
fn seed(n: usize, refs: usize, at: &Places) -> Row {
    let mut row = vec![Value::Null; refs];
    row.push(integer(n));
    row.extend((0..at.p).map(|_| integer(1)));
    row.extend(at.split.iter().map(|_| Value::Bool(false)));
    row
}

/// [SR17n] Whether GROUP BY DISTINCT's UNION DISTINCT can find two rows
/// alike that the grouping keeps apart: of two sets of the same columns,
/// grouped by different collations, or of a column grouped by a collation
/// a `<collate clause>` gave, not the one its values are compared by. Any
/// other two rows differ in a GROUPING bit or in a grouping value.
fn needs_distinct(r: &Resolved) -> bool {
    if r.references.iter().any(|g| g.explicit) {
        return true;
    }
    let columns = |s: &Vec<usize>| {
        let mut of: Vec<usize> = s.iter().filter_map(|&j| r.references.get(j).map(|g| g.pos)).collect();
        of.sort_unstable();
        of.dedup();
        of
    };
    let all: Vec<Vec<usize>> = r.members.iter().map(columns).collect();
    all.iter().enumerate().any(|(i, a)| all.iter().skip(i.saturating_add(1)).any(|b| a == b))
}
