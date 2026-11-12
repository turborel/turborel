//! Sequence generators [ISO/IEC 9075-2:2023 §4.29, §11.72 to §11.74]:
//! CREATE, ALTER and DROP SEQUENCE, in a transaction as the other schema
//! statements are (RULES.md D33), and the values NEXT VALUE FOR draws.
//!
//! A descriptor is a definition, made and undone with its transaction. Its
//! current base value is not \[§4.29.1\]: it is a counter shared by every
//! snapshot (`sequences/counters.rs`, RULES.md D62), and a value given out
//! is logged apart from the transaction that asked, so it is never given
//! again, even if that transaction rolls back.

use std::collections::VecDeque;
use std::sync::Arc;

mod counters;
mod usage;

pub(crate) use counters::{Counters, Settled};

use super::ddl::Dropped;
use super::definitions::Defined;
use super::{State, api, rule};
use crate::ast::{AlterSequence, SequenceDefinition, SequenceOptions, SignedNumber};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, SequenceDesc};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::log::record::{Change, Record};
use crate::txn::{LockMode, NO_TRANSACTION};
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value, pow10};

/// [ISO/IEC 9075-2:2023 §11.72 SR6] The type of a sequence generator that
/// names none (ID157): NUMERIC of the greatest precision, scale 0.
pub(crate) const DEFAULT_TYPE: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// \[DESIGN\] RULES.md D62: the epochs of the counters of the sequence
/// generators a statement defines, or restarts, in order: each given anew
/// as a session's statement makes it, else as its record logged them.
#[derive(Debug, Default)]
pub(super) struct Epochs {
    /// Those its record logged, still to be taken; none of a statement.
    logged: Option<VecDeque<u64>>,
    /// Those it took.
    given: Vec<u64>,
}

impl Epochs {
    /// The epochs of a session's statement, each given anew.
    pub(super) fn fresh() -> Epochs {
        Epochs::default()
    }

    /// The epochs `logged`, as a record of the statement holds them.
    pub(super) fn logged(logged: &[u64]) -> Epochs {
        Epochs { logged: Some(logged.iter().copied().collect()), given: Vec::new() }
    }

    /// Whether they are given anew, as a session's statement is made.
    fn are_fresh(&self) -> bool {
        self.logged.is_none()
    }

    /// The next epoch, noted given in `counters`.
    fn next(&mut self, counters: &Counters) -> Result<u64> {
        let epoch = match &mut self.logged {
            None => counters.epoch()?,
            Some(logged) => {
                let epoch = logged.pop_front();
                epoch.ok_or_else(|| api("[DESIGN] RULES.md D62: a definition logged holds fewer epochs than it gives"))?
            }
        };
        counters.advance_past(epoch)?;
        self.given.push(epoch);
        Ok(epoch)
    }

    /// The epochs taken, in order, to log.
    pub(super) fn given(self) -> Vec<u64> {
        self.given
    }
}

/// The options a definition or alteration gives, its values taken.
struct Values {
    increment: i128,
    maximum: i128,
    minimum: i128,
    cycle: bool,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.72] CREATE SEQUENCE `d`, its name in schema
    /// `schema` if it names none, its epoch from `epochs`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema is not there (SQLSTATE 3F000), it has a
    /// sequence generator of the name \[SR3\], its type is no exact numeric
    /// of scale 0 \[SR6\], or its options break §9.36's Syntax Rules.
    pub(super) fn create_sequence(&mut self, schema: &str, d: &SequenceDefinition, epochs: &mut Epochs) -> Result<Defined> {
        let qn = qualify(&d.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.72 SR3] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(d.name.at));
        }
        if self.catalog.sequence(&qn).is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.72 SR3] schema {} has a sequence generator {} already", qn.auth, qn.ident))
                .or_at(d.name.at));
        }
        let data_type = match d.data_type {
            None => DEFAULT_TYPE,
            Some((t @ DataType::Exact { scale: 0, .. }, _)) => t,
            Some((t, at)) => {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.72 SR6] a sequence generator is of an exact numeric type of scale 0, not {t}"
                ))
                .or_at(at));
            }
        };
        let o = &d.options;
        let increment = o.increment.map_or(Ok(1), |n| integer(n, data_type))?;
        let values = bounds(o, increment, data_type, None).or_at(d.name.at)?;
        let start = match o.start {
            Some(n) => integer(n, data_type)?,
            None if increment > 0 => values.minimum,
            None => values.maximum,
        };
        within(start, &values, "§9.36 SR12", "start").or_at(o.start.map_or(d.name.at, |n| n.at))?;
        let epoch = epochs.next(&self.sequences)?;
        self.put_sequence(SequenceDesc { data_type, start, epoch, origin: start, ..described(qn.clone(), &values) });
        Ok(Defined { locks: vec![(QualifiedName::of_schema(&qn.auth), LockMode::Shared)], ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.72] CREATE SEQUENCE `d` of the `<schema
    /// definition>` of schema `schema`, its epoch from `epochs`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its name is of another schema \[SR2\]; as
    /// `create_sequence`.
    pub(super) fn schema_sequence(&mut self, schema: &str, d: &SequenceDefinition, epochs: &mut Epochs) -> Result<()> {
        if let Some(other) = d.name.auth.as_deref().filter(|a| *a != schema) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.72 SR2] sequence generator {} of schema {schema} is named in schema {other}",
                d.name.ident
            );
            return Err(rule(msg).or_at(d.name.at));
        }
        self.create_sequence(schema, d, epochs).map(|_| ())
    }

    /// [ISO/IEC 9075-2:2023 §11.73] ALTER SEQUENCE `a`, its name in schema
    /// `schema` if it names none; restarted, its new epoch from `epochs`.
    /// Not restarted, its current base value is kept, which a session's
    /// statement checks is within its new bounds \[§9.37 SR12\]: a record
    /// made again, checked as it was made, is not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such sequence generator \[SR2\], or its
    /// options break §9.37's Syntax Rules.
    pub(super) fn alter_sequence(&mut self, schema: &str, a: &AlterSequence, epochs: &mut Epochs) -> Result<Defined> {
        let qn = qualify(&a.name, schema);
        let seq = self.sequence_named(&qn, "§11.73 SR2").or_at(a.name.at)?;
        let o = &a.options;
        let increment = o.increment.map_or(Ok(seq.increment), |n| integer(n, seq.data_type))?;
        let values = bounds(o, increment, seq.data_type, Some(&seq)).or_at(a.name.at)?;
        let restarted = match o.restart {
            None => None,
            Some(None) => Some((seq.start, a.name.at)),
            Some(Some(n)) => Some((integer(n, seq.data_type)?, n.at)),
        };
        let (epoch, origin) = if let Some((value, at)) = restarted {
            within(value, &values, "§9.37 SR12", "restart").or_at(at)?;
            (epochs.next(&self.sequences)?, value)
        } else {
            if epochs.are_fresh() {
                self.keeps_base(&seq, &values).or_at(a.name.at)?;
            }
            (seq.epoch, seq.origin)
        };
        self.put_sequence(SequenceDesc {
            data_type: seq.data_type,
            start: seq.start,
            epoch,
            origin,
            usage: seq.usage,
            ..described(qn, &values)
        });
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §9.37 SR12] That `seq`'s current base value is
    /// within the bounds `values` gives it. \[DESIGN\] Or that it has none
    /// left in its cycle, exhausted, as it is then, beyond its bounds.
    fn keeps_base(&self, seq: &SequenceDesc, values: &Values) -> Result<()> {
        let base = self.sequences.base(seq)?;
        if counters::exhausted(base, seq) {
            return Ok(());
        }
        within(base, values, "§9.37 SR12", "current base")
    }

    /// [ISO/IEC 9075-2:2023 §11.74] Drops sequence generator `qn`, and USAGE
    /// on it \[GR1\]. Nothing names one that RESTRICT refuses \[SR3\]: no
    /// view, constraint or default holds NEXT VALUE FOR [§6.14 SR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such sequence generator \[SR2\].
    pub(super) fn drop_sequence(&mut self, qn: &QualifiedName) -> Result<Dropped> {
        self.sequence_named(qn, "§11.74 SR2")?;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_sequence(qn);
        self.catalog = Arc::from(catalog);
        Ok(Dropped::default())
    }

    /// [ISO/IEC 9075-2:2023 §11.2 GR3] The sequence generators of schema
    /// `auth`, which is being dropped, each dropped.
    pub(super) fn drop_schema_sequences(&mut self, auth: &str) -> Result<Dropped> {
        for qn in self.schema_sequences(auth) {
            self.drop_sequence(&qn)?;
        }
        Ok(Dropped::default())
    }

    /// The sequence generators of schema `auth`.
    pub(super) fn schema_sequences(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.sequences().into_iter().filter(|s| s.name.auth == auth).map(|s| s.name.clone()).collect()
    }

    /// The descriptor of sequence generator `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none, as the Syntax Rule `cited` says.
    pub(crate) fn sequence_named(&self, qn: &QualifiedName, cited: &str) -> Result<SequenceDesc> {
        let found = self.catalog.sequence(qn).cloned();
        found.ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 {cited}] there is no sequence generator {qn}")))
    }

    /// \[DESIGN\] RULES.md D62: as a database opens, with no transaction
    /// open, drops each counter none of its sequence generators draws from,
    /// one dropped, restarted, or made by a transaction that did not commit.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked holding the counters.
    pub(super) fn prune_counters(&self) -> Result<()> {
        self.sequences.prune(&|name, epoch| self.catalog.sequence(name).is_some_and(|s| s.epoch == epoch))
    }

    /// `desc`, recorded.
    fn put_sequence(&mut self, desc: SequenceDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_sequence(desc);
        self.catalog = Arc::from(catalog);
    }

    /// [ISO/IEC 9075-2:2023 §9.35] The next value of `seq`: once a record
    /// of where its counter goes on after a crash is synced, if the
    /// database keeps a log, logged apart from any transaction.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200H if its cycle is exhausted and it is of NO CYCLE
    /// \[GR3a\]; the database's if it takes no change, a standby's or one
    /// whose log failed; the log's.
    pub(crate) fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        let reserve = |resume: i128| -> Result<()> {
            let Some(log) = &self.log else { return Ok(()) };
            self.tx.taking_changes()?;
            let change = Change::Sequence { name: seq.name.clone(), epoch: seq.epoch, resume };
            let lsn = log.append(&Record { tx: NO_TRANSACTION, change }.encode())?;
            self.tx.sync(&**log, lsn)
        };
        let logged: Option<&dyn Fn(i128) -> Result<()>> = if self.log.is_some() { Some(&reserve) } else { None };
        Ok(Value::Exact(Decimal::new(self.sequences.next(seq, logged)?, 0)))
    }
}

/// The descriptor of `name` of `values`, the rest to be given.
fn described(name: QualifiedName, values: &Values) -> SequenceDesc {
    SequenceDesc {
        name,
        data_type: DEFAULT_TYPE,
        start: 0,
        minimum: values.minimum,
        maximum: values.maximum,
        increment: values.increment,
        cycle: values.cycle,
        epoch: 0,
        origin: 0,
        usage: Vec::new(),
    }
}

/// [ISO/IEC 9075-2:2023 §9.36 SR4 to SR11, SR13; §9.37 SR5 to SR8, SR10,
/// SR11] The increment `increment`, bounds and cycle option `o` gives a
/// generator of type `t`, else `before`'s, else the defaults: of NO
/// MAXVALUE, the type's greatest value ascending, -1 descending (ID112);
/// of NO MINVALUE, 1 ascending, the type's least value descending (ID113).
fn bounds(o: &SequenceOptions, increment: i128, t: DataType, before: Option<&SequenceDesc>) -> Result<Values> {
    if increment == 0 {
        return Err(rule("[ISO/IEC 9075-2:2023 §9.36 SR5, §9.37 SR5] the increment of a sequence generator is not 0".to_owned()));
    }
    let greatest = greatest_of(t);
    let (high, low) = if increment > 0 { (greatest, 1) } else { (-1, greatest.saturating_neg()) };
    let bound = |given: Option<Option<SignedNumber>>, kept: Option<i128>, default: i128| match given {
        Some(Some(n)) => integer(n, t),
        Some(None) => Ok(default),
        None => Ok(kept.unwrap_or(default)),
    };
    let maximum = bound(o.maximum, before.map(|s| s.maximum), high)?;
    let minimum = bound(o.minimum, before.map(|s| s.minimum), low)?;
    if maximum <= minimum {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.36 SR11, §9.37 SR11] a sequence generator's maximum value is greater than its minimum, and {maximum} is not greater than {minimum}"
        )));
    }
    let cycle = o.cycle.or(before.map(|s| s.cycle)).unwrap_or(false);
    Ok(Values { increment, maximum, minimum, cycle })
}

/// [ISO/IEC 9075-2:2023 §9.36 SR12, §9.37 SR12] That `value`, the value
/// `what`, is within `values`' bounds, as the Syntax Rule `cited` says.
fn within(value: i128, values: &Values, cited: &str, what: &str) -> Result<()> {
    if (values.minimum..=values.maximum).contains(&value) {
        return Ok(());
    }
    Err(rule(format!(
        "[ISO/IEC 9075-2:2023 {cited}] a sequence generator's {what} value is within its bounds, and {value} is not within {} to {}",
        values.minimum, values.maximum
    )))
}

/// The greatest value of `t`, an exact numeric type of scale 0.
fn greatest_of(t: DataType) -> i128 {
    let precision = if let DataType::Exact { precision, .. } = t { precision } else { MAX_EXACT_PRECISION };
    pow10(precision).map_or(i128::MAX, |p| p.saturating_sub(1))
}

/// [ISO/IEC 9075-2:2023 §9.36 SR10, §9.37 SR10] The value of `n`, which is
/// exactly representable with the precision and scale of `t`.
///
/// # Errors
///
/// SQLCODE -103 if it is not.
fn integer(n: SignedNumber, t: DataType) -> Result<i128> {
    let whole = n.value.and_then(|d| {
        let factor = pow10(d.scale)?;
        (d.units.checked_rem(factor)? == 0).then(|| d.units.checked_div(factor)).flatten()
    });
    let greatest = greatest_of(t);
    match whole.filter(|v| v.checked_abs().is_some_and(|a| a <= greatest)) {
        Some(v) => Ok(v),
        None => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.36 SR10, §9.37 SR10] a value of a sequence generator is exactly representable in {t}, as {} is not",
            n.value.map_or_else(|| "this approximate number".to_owned(), |d| d.to_string())
        ))
        .or_at(n.at)),
    }
}

// Unit tests: the defaults, bounds and representable values §9.36 and
// §9.37 give (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
