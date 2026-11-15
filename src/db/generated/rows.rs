//! How each row a statement writes gets the values of its table's identity
//! column and generated columns [ISO/IEC 9075-2:2023 §15.11 GR3a, GR6a;
//! §15.14 GR7a, GR9; §11.11 GR2], and the rules an INSERT keeps for them
//! [§14.11 SR12, SR13].

use std::sync::Arc;

use crate::ast::{InsertSource, Overriding, QueryExpression, RowElement, ValuesRow};
use crate::binder::generations;
use crate::catalog::{SequenceDesc, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::{Subqueries, eval};
use crate::logical::Expr;
use crate::storage::Row;
use crate::value::{DataType, assign};

/// What a statement gives each row it writes of a table: its identity
/// column's next value, if it is unassigned in each, and each generated
/// column's value.
#[derive(Debug, Clone, Default)]
pub(in crate::db) struct Generating {
    /// The identity column, its position, type and sequence generator, if
    /// each row takes the generator's next value \[§15.11 GR3a\].
    draw: Option<(usize, DataType, Arc<SequenceDesc>)>,
    /// Each generated column, its position, type and generation expression
    /// bound over the row \[§15.11 GR6a\].
    exprs: Vec<(usize, DataType, Expr)>,
}

impl Generating {
    /// Of table `desc`: its generated columns; and its identity column's
    /// next value if `draw`, the column unassigned in every row.
    ///
    /// # Errors
    ///
    /// A generation expression binds no more.
    pub(in crate::db) fn of(st: &State, desc: &TableDesc, draw: bool) -> Result<Generating> {
        let typed = |at: usize| desc.columns.get(at).map(|c| c.data_type.clone());
        let exprs = generations(st, desc)?.into_iter().filter_map(|(at, e)| Some((at, typed(at)?, e))).collect();
        let identity =
            desc.columns.iter().enumerate().find_map(|(at, c)| Some((at, c.data_type.clone(), Arc::new(c.identity()?.sequence.clone()))));
        Ok(Generating { draw: identity.filter(|_| draw), exprs })
    }

    /// [ISO/IEC 9075-2:2023 §11.11 GR2] Of the column at `at` of `desc`
    /// alone, a column added: its next value of an identity column, its
    /// generation expression's of a generated column; nothing of another.
    ///
    /// # Errors
    ///
    /// As `of`.
    pub(in crate::db) fn of_column(st: &State, desc: &TableDesc, at: usize) -> Result<Generating> {
        let all = Generating::of(st, desc, true)?;
        Ok(Generating { draw: all.draw.filter(|(k, ..)| *k == at), exprs: all.exprs.into_iter().filter(|(k, ..)| *k == at).collect() })
    }

    /// The positions of the columns it gives each row values of.
    pub(in crate::db) fn columns(&self) -> impl Iterator<Item = usize> + '_ {
        self.draw.iter().map(|(at, ..)| *at).chain(self.exprs.iter().map(|(at, ..)| *at))
    }

    /// Whether it gives each row its identity column's next value.
    pub(in crate::db) fn draws(&self) -> bool {
        self.draw.is_some()
    }

    /// `row` given its identity column's next value, if each row takes it,
    /// then each generated column's value, by store assignment [§9.2], its
    /// generation expression's subqueries run by `subqueries`: it has none.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200H if the identity column's generator is exhausted; an
    /// exception a generation expression raises, or its value's assignment.
    pub(in crate::db) fn give(&self, st: &State, row: &mut Row, subqueries: &dyn Subqueries) -> Result<()> {
        if let Some((at, t, seq)) = &self.draw {
            let next = st.next_value(seq).map_err(|e| of_identity(e, seq))?;
            put(row, *at, assign(next, t, "ISO/IEC 9075-2:2023 §15.11 GR3a iii")?)?;
        }
        for (at, t, e) in &self.exprs {
            let value = eval(e, row, subqueries)?;
            put(row, *at, assign(value, t, "ISO/IEC 9075-2:2023 §15.11 GR6a, §15.14 GR9")?)?;
        }
        Ok(())
    }
}

/// `value` in the column at `at` of `row`.
fn put(row: &mut Row, at: usize, value: crate::value::Value) -> Result<()> {
    let slot = row.get_mut(at).ok_or_else(|| crate::db::api(format!("[DESIGN] a row has no column {at}")))?;
    *slot = value;
    Ok(())
}

/// `e`, raised drawing from the generator `seq` of an identity column:
/// said to be of the identity column of its table, whose name it has.
fn of_identity(e: SqlError, seq: &SequenceDesc) -> SqlError {
    if e.state != Some(SqlState::SEQUENCE_GENERATOR_LIMIT_EXCEEDED) {
        return e;
    }
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §9.35 GR3a] the sequence generator of the identity column of {} has issued every value of its cycle, and is of NO CYCLE",
        seq.name
    );
    SqlError { msg, ..e }
}

/// Whether an object column of an INSERT is given values, and where the
/// first is written, if the INSERT says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Given {
    /// By DEFAULT VALUES, or DEFAULT in every row of its VALUES
    /// [ISO/IEC 9075-2:2023 §14.11 SR11b]: it is defaulted.
    Defaulted,
    /// By a query expression other than VALUES.
    Queried,
    /// In a row of its VALUES, written there.
    At(usize),
}

impl Given {
    /// Where a refusal of a value given points, if anywhere.
    fn at(self) -> Option<usize> {
        match self {
            Given::At(at) => Some(at),
            Given::Defaulted | Given::Queried => None,
        }
    }
}

/// Where the values an INSERT gives come from: an `<insert columns and
/// source>` [ISO/IEC 9075-2:2023 §14.11], or the one row of a MERGE's
/// `<merge insert value list>` [§14.12].
#[derive(Debug, Clone, Copy)]
pub(in crate::db) enum Inserted<'a> {
    /// An INSERT's source.
    Source(&'a InsertSource),
    /// A MERGE's row of values.
    Row(&'a ValuesRow),
}

impl Inserted<'_> {
    /// The rule giving a generated column only DEFAULT: §14.11 SR12 of an
    /// INSERT, §14.12 SR18f of a MERGE.
    fn generated_rule(self) -> &'static str {
        match self {
            Inserted::Source(_) => "§14.11 SR12",
            Inserted::Row(_) => "§14.12 SR18f",
        }
    }

    /// The rules of the `<override clause>`: when it is written at all,
    /// of an identity column always generated, and of one generated by
    /// default; §14.11 SR13 of an INSERT, §14.12 SR18h of a MERGE.
    fn override_rules(self) -> [&'static str; 3] {
        match self {
            Inserted::Source(_) => ["§14.11 SR13", "§14.11 SR13b", "§14.11 SR13c"],
            Inserted::Row(_) => ["§14.12 SR18h", "§14.12 SR18h ii", "§14.12 SR18h iii"],
        }
    }
}

/// [ISO/IEC 9075-2:2023 §14.11 SR11b, §14.12 SR18b] Whether the object
/// column `k` of an INSERT from `source` is given values, and where.
fn given_at(source: Inserted<'_>, k: usize) -> Given {
    let at = |r: &ValuesRow| match r.elements.get(k) {
        Some(RowElement::Default(_)) => None,
        Some(RowElement::Value(_, at) | RowElement::Null(at)) => Some(Given::At(*at)),
        None => Some(Given::At(r.at)),
    };
    let query = match source {
        Inserted::Row(row) => return at(row).unwrap_or(Given::Defaulted),
        Inserted::Source(InsertSource::DefaultValues) => return Given::Defaulted,
        Inserted::Source(InsertSource::Query(query)) => &**query,
    };
    let QueryExpression::Values(rows) = query else { return Given::Queried };
    rows.iter().find_map(at).unwrap_or(Given::Defaulted)
}

/// Error `e`, placed `at` if that says where.
fn placed(e: SqlError, at: Option<usize>) -> SqlError {
    match at {
        Some(at) => e.or_at(at),
        None => e,
    }
}

/// [ISO/IEC 9075-2:2023 §14.11 SR12, SR13, §14.12 SR18f, SR18h] That an INSERT, or a MERGE's, into base table
/// `desc`, its object columns `object` by their positions, from `source`,
/// with the `<override clause>` `overriding`, gives no generated column a
/// value, and gives an identity column one as its override says. Whether
/// the identity column, if the table has one, takes its generator's next
/// value in every row: it is no object column, or its values are given
/// by DEFAULT VALUES, or OVERRIDING USER VALUE ignores them \[GR7e\].
///
/// # Errors
///
/// SQLCODE -103 for a rule the INSERT breaks.
pub(in crate::db) fn check_override(
    desc: &TableDesc,
    object: &[usize],
    (source, overriding): (Inserted<'_>, Option<Overriding>),
) -> Result<bool> {
    let mut identity = None;
    for (k, column) in object.iter().filter_map(|&c| desc.columns.get(c)).enumerate() {
        let given = given_at(source, k);
        if column.generation().is_some() && given != Given::Defaulted {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 {}] column {} is a generated column, so it is given only DEFAULT",
                source.generated_rule(),
                column.name
            );
            return Err(placed(rule(msg), given.at()));
        }
        if let Some(id) = column.identity() {
            identity = Some((column.name.as_str(), id.always, given));
        }
    }
    if let Some((msg, at)) = override_refused(identity, overriding, source.override_rules()) {
        return Err(placed(rule(msg), at));
    }
    Ok(identity.is_none() || overriding == Some(Overriding::User) || matches!(source, Inserted::Source(InsertSource::DefaultValues)))
}

/// The identity column an INSERT inserts into, its name, whether its
/// values are always generated, and whether it is given values, and
/// where.
type IdentityGiven<'a> = Option<(&'a str, bool, Given)>;

/// [ISO/IEC 9075-2:2023 §14.11 SR13] Why the `<override clause>`
/// `overriding` may not be written, or must be, of an INSERT into the
/// identity column `identity`, and where, if it says.
fn override_refused(
    identity: IdentityGiven<'_>,
    overriding: Option<Overriding>,
    [rule, always, by_default]: [&str; 3],
) -> Option<(String, Option<usize>)> {
    let cited = "[ISO/IEC 9075-2:2023";
    match (identity, overriding) {
        (None, Some(_)) => Some((format!("{cited} {rule}] an <override clause> is written only to insert into an identity column"), None)),
        (Some((name, true, given @ (Given::Queried | Given::At(_)))), None) => Some((
            format!(
                "{cited} {always}] identity column {name} is GENERATED ALWAYS, so a value is given it only with OVERRIDING SYSTEM VALUE or OVERRIDING USER VALUE"
            ),
            given.at(),
        )),
        (Some((name, true, Given::Defaulted)), Some(_)) => {
            Some((format!("{cited} {rule}] identity column {name} is given no value, so no <override clause> is written"), None))
        }
        (Some((name, false, _)), Some(Overriding::System)) => Some((
            format!(
                "{cited} {by_default}] identity column {name} is GENERATED BY DEFAULT, so its <override clause> is OVERRIDING USER VALUE"
            ),
            None,
        )),
        (None, None)
        | (Some((_, true, Given::Queried | Given::At(_))), Some(_))
        | (Some((_, true, Given::Defaulted)), None)
        | (Some((_, false, _)), None | Some(Overriding::User)) => None,
    }
}

/// A Syntax Rule violation.
fn rule(message: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, message)
}
