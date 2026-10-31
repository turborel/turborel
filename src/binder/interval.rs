//! [ISO/IEC 9075:1992 §6.14, §6.15, §8.11] Intervals as a statement binds
//! them: arithmetic of datetimes and intervals and its types, the
//! difference of two datetimes, `<time zone>`, and OVERLAPS.

use std::sync::Arc;

use super::datetime::shown;
use super::{Binder, Scope, rule};
use crate::ast::{RowValue, ValueExpression};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Cond, Expr};
use crate::value::datetime::{Datetime, Field, MAX_DISPLACEMENT, MIN_DISPLACEMENT};
use crate::value::interval::{MAX_LEADING_PRECISION, Qualifier, difference, minutes};
use crate::value::{ArithOp, CompOp, DataType, Value};

/// The qualifier of `t`, if an interval type.
fn qualifier(t: &DataType) -> Option<Qualifier> {
    if let DataType::Interval(q) = t { Some(*q) } else { None }
}

/// [ISO/IEC 9075:1992 §6.14 SR3] Whether an interval of `q` may be added
/// to a datetime of `t`: its fields all the datetime's.
fn fits(t: &DataType, q: Qualifier) -> bool {
    let has = |f: Field| match t {
        DataType::Date => matches!(f, Field::Year | Field::Month | Field::Day),
        DataType::Time { .. } => matches!(f, Field::Hour | Field::Minute | Field::Second),
        DataType::Timestamp { .. } => true,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => false,
    };
    q.fields().all(has)
}

/// A datetime operand not of an interval its fields hold.
fn not_added(t: &DataType, q: Qualifier) -> SqlError {
    rule(format!("[ISO/IEC 9075:1992 §6.14 SR3] INTERVAL {q} is not added to or subtracted from {t}"))
}

/// [ISO/IEC 9075:1992 §6.15 GR6] (A - B) Q.
struct Difference {
    qualifier: Qualifier,
}

impl ScalarFunction for Difference {
    fn name(&self) -> &'static str {
        "-"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Interval(self.qualifier))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (Some(Value::Datetime(a)), Some(Value::Datetime(b))) = (args.first(), args.get(1)) else { return Ok(Value::Null) };
        difference(a, b, self.qualifier)
    }
}

/// [ISO/IEC 9075:1992 §6.14 GR4] A datetime AT a time zone: its instant,
/// of the displacement given, or of the session's, `local`.
struct AtZone {
    local: i16,
}

impl ScalarFunction for AtZone {
    fn name(&self) -> &'static str {
        "AT TIME ZONE"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        Ok(args.first().copied().unwrap_or(DataType::Date))
    }
    /// [GR2] null of a null displacement; [GR4b] 22009 of one out of
    /// -12:59 to +13:00.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let zone = match args.get(1) {
            None => Some(self.local),
            Some(Value::Interval(i)) => minutes(i),
            Some(_) => return Ok(Value::Null),
        };
        let Some(Value::Datetime(d)) = args.first() else { return Ok(Value::Null) };
        match zone.filter(|z| (MIN_DISPLACEMENT..=MAX_DISPLACEMENT).contains(z)) {
            Some(zone) => Ok(Value::Datetime(Datetime { zone, zoned: true, ..*d })),
            None => fail_with(
                Code::Data,
                SqlState::INVALID_TIME_ZONE_DISPLACEMENT,
                "[ISO/IEC 9075:1992 §6.14 GR4b] a time zone displacement is not from -12:59 to +13:00",
            ),
        }
    }
}

/// The type `t` WITH TIME ZONE.
fn zoned(t: DataType) -> DataType {
    match t {
        DataType::Time { precision, .. } => DataType::Time { precision, zoned: true },
        DataType::Timestamp { precision, .. } => DataType::Timestamp { precision, zoned: true },
        DataType::Date
        | DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Bit { .. }
        | DataType::BitVarying { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => t,
    }
}

/// [ISO/IEC 9075:1992 §16.5 SR1, §6.14 SR4] Whether `t` is INTERVAL HOUR
/// TO MINUTE, a time zone displacement's type.
pub fn is_displacement(t: &DataType) -> bool {
    qualifier(t).is_some_and(|q| q.start == Field::Hour && q.end == Field::Minute)
}

/// The value of the time zone displacement `v`, an interval HOUR TO
/// MINUTE, in minutes, if it is one from -12:59 to +13:00 [§16.5 GR1b].
pub fn displacement_of(v: &Value) -> Option<i16> {
    let Value::Interval(i) = v else { return None };
    minutes(i).filter(|z| (MIN_DISPLACEMENT..=MAX_DISPLACEMENT).contains(z))
}

/// An operand of datetime arithmetic that is no interval.
fn not_interval(t: &DataType) -> SqlError {
    rule(format!("[ISO/IEC 9075:1992 §6.14, §6.15] a datetime is added to or subtracted from an interval, not {t}"))
}

/// [ISO/IEC 9075:1992 §6.15 GR4f, GR5e] `q` of the most leading digits,
/// so the result of an operation loses none.
fn widest(q: Qualifier) -> Qualifier {
    Qualifier { leading: u8::try_from(MAX_LEADING_PRECISION).unwrap_or(9), ..q }
}

/// `CASE WHEN when THEN then ELSE otherwise END`.
fn case(when: Cond, then: Expr, otherwise: Expr) -> Expr {
    Expr::Case(vec![(when, then)], Box::new(otherwise))
}

/// `a op b`.
fn compare(a: &Expr, op: CompOp, b: &Expr) -> Cond {
    Cond::Compare(a.clone(), op, b.clone())
}

/// `a AND b`.
fn and(a: Cond, b: Cond) -> Cond {
    Cond::And(Box::new(a), Box::new(b))
}

/// `a OR b`.
fn or(a: Cond, b: Cond) -> Cond {
    Cond::Or(Box::new(a), Box::new(b))
}

/// [ISO/IEC 9075:1992 §8.11 GR3, GR5] The start and the end of the period
/// from `d` to `e`: S = E and T = D if D is null or E < D; else S = D
/// and T = E.
fn ordered(d: &Expr, e: &Expr) -> (Expr, Expr) {
    let reversed = || or(Cond::IsNull(d.clone(), false), compare(e, CompOp::Lt, d));
    (case(reversed(), e.clone(), d.clone()), case(reversed(), d.clone(), e.clone()))
}

/// [ISO/IEC 9075:1992 §8.11 GR6] Whether the periods from `s1` to `t1`
/// and from `s2` to `t2` overlap.
fn overlap((s1, t1): (&Expr, &Expr), (s2, t2): (&Expr, &Expr)) -> Cond {
    let after = |sa: &Expr, sb: &Expr, ta: &Expr, tb: &Expr| {
        and(compare(sa, CompOp::Gt, sb), Cond::Not(Box::new(and(compare(sa, CompOp::Ge, tb), compare(ta, CompOp::Ge, tb)))))
    };
    let same = and(compare(s1, CompOp::Eq, s2), or(compare(t1, CompOp::Ne, t2), compare(t1, CompOp::Eq, t2)));
    or(or(after(s1, s2, t1, t2), after(s2, s1, t2, t1)), same)
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §6.14, §6.15] `a` `op` `b`, of types `ta` and
    /// `tb`, one a datetime or an interval: a datetime plus or minus an
    /// interval of its fields, or an interval plus a datetime, a datetime
    /// \[§6.14 SR3\]; two intervals of one class added or subtracted, of the
    /// fields of both \[§6.15 SR3c, SR4\]; an interval times or divided by a
    /// number, or a number times one, an interval.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for any other.
    pub(super) fn temporal(&self, (a, ta): (Expr, DataType), op: ArithOp, (b, tb): (Expr, DataType)) -> Result<(Expr, DataType)> {
        let additive = matches!(op, ArithOp::Add | ArithOp::Sub);
        let arith = |x: Expr, y: Expr| Expr::Arith(Box::new(x), op, Box::new(y));
        if ta.is_datetime() && additive {
            let q = qualifier(&tb).ok_or_else(|| not_interval(&tb))?;
            return if fits(&ta, q) { Ok((arith(shown(a, ta, self.now), b), ta)) } else { Err(not_added(&ta, q)) };
        }
        if tb.is_datetime() && op == ArithOp::Add {
            let q = qualifier(&ta).ok_or_else(|| not_interval(&ta))?;
            return if fits(&tb, q) { Ok((arith(a, shown(b, tb, self.now)), tb)) } else { Err(not_added(&tb, q)) };
        }
        let refused = || rule(format!("[ISO/IEC 9075:1992 §6.14, §6.15] {ta} {} {tb} is no datetime or interval arithmetic", op.symbol()));
        let number = |t: &DataType| t.is_numeric();
        let t = match (qualifier(&ta), qualifier(&tb)) {
            (Some(x), Some(y)) => {
                if !additive || x.year_month() != y.year_month() {
                    return Err(refused());
                }
                let leading = u32::from(x.leading.max(y.leading)).saturating_add(1).min(MAX_LEADING_PRECISION);
                x.joined(&y, leading)
            }
            (Some(x), None) => {
                if !matches!(op, ArithOp::Mul | ArithOp::Div) || !number(&tb) {
                    return Err(refused());
                }
                widest(x)
            }
            (None, Some(y)) => {
                if op != ArithOp::Mul || !number(&ta) {
                    return Err(refused());
                }
                widest(y)
            }
            (None, None) => return Err(refused()),
        };
        Ok((arith(a, b), DataType::Interval(t)))
    }

    /// [ISO/IEC 9075:1992 §6.15] (`minuend` - `subtrahend`) `q`: datetimes
    /// of the same fields \[SR5\], their difference an interval of `q`
    /// \[GR6\]; of years and months, of datetimes that have them.
    pub(super) fn difference(
        &self,
        (minuend, subtrahend): (&ValueExpression, &ValueExpression),
        q: Qualifier,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (a, ta) = self.value_expression(minuend, scope)?;
        let (b, tb) = self.value_expression(subtrahend, scope)?;
        if !ta.is_datetime() || !ta.comparable(&tb) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.15 SR5] the difference of {ta} and {tb}, not datetimes of the same fields")));
        }
        if q.year_month() && matches!(ta, DataType::Time { .. }) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.15 GR6] a time has no years or months: INTERVAL {q}")));
        }
        let args = vec![shown(a, ta, self.now), shown(b, tb, self.now)];
        Ok((Expr::Call(ScalarRef(Arc::new(Difference { qualifier: q })), args), DataType::Interval(q)))
    }

    /// [ISO/IEC 9075:1992 §6.14] `source` AT TIME ZONE `zone`, or AT LOCAL
    /// for none: a time or timestamp \[SR5a\], the zone INTERVAL HOUR TO
    /// MINUTE \[SR4\]; its instant, WITH TIME ZONE, of that displacement.
    pub(super) fn at_zone(&self, source: &ValueExpression, zone: Option<&ValueExpression>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(source, scope)?;
        if !matches!(t, DataType::Time { .. } | DataType::Timestamp { .. }) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.14 SR1, SR5a] {t} has no time zone")));
        }
        let mut args = vec![shown(e, t, self.now)];
        if let Some(z) = zone {
            let (ze, zt) = self.value_expression(z, scope)?;
            if !is_displacement(&zt) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §6.14 SR4] a time zone is INTERVAL HOUR TO MINUTE, not {zt}")));
            }
            args.push(ze);
        }
        Ok((Expr::Call(ScalarRef(Arc::new(AtZone { local: self.now.zone() })), args), zoned(t)))
    }

    /// [ISO/IEC 9075:1992 §8.11] `left` OVERLAPS `right`: rows of two
    /// \[SR1\], each a datetime, the two comparable \[SR2\], then a datetime
    /// comparable with it or an interval of its fields \[SR3\].
    pub(super) fn overlaps(&self, left: &RowValue, right: &RowValue, scope: Scope<'_>) -> Result<Cond> {
        let (d1, e1, type1) = self.period(left, scope)?;
        let (d2, e2, type2) = self.period(right, scope)?;
        if !type1.comparable(&type2) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §8.11 SR2] {type1} and {type2} are not comparable")));
        }
        let ((s1, t1), (s2, t2)) = (ordered(&d1, &e1), ordered(&d2, &e2));
        Ok(overlap((&s1, &t1), (&s2, &t2)))
    }

    /// [ISO/IEC 9075:1992 §8.11 SR1-SR3, GR2, GR4] A period of OVERLAPS:
    /// its start, its end, the start plus an interval if one is given, and
    /// its type.
    fn period(&self, row: &RowValue, scope: Scope<'_>) -> Result<(Expr, Expr, DataType)> {
        let [first, second] = row.as_slice() else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §8.11 SR1] a period of OVERLAPS has two values, not {}", row.len())));
        };
        let (d, td) = self.value_expression(first, scope)?;
        if !td.is_datetime() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §8.11 SR2] a period begins with a datetime, not {td}")));
        }
        let (x, tx) = self.value_expression(second, scope)?;
        if tx.is_datetime() {
            if !td.comparable(&tx) {
                return Err(rule(format!("[ISO/IEC 9075:1992 §8.11 SR3b] {td} and {tx} are not comparable")));
            }
            return Ok((d, x, td));
        }
        let q = qualifier(&tx)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §8.11 SR3] a period ends with a datetime or an interval, not {tx}")))?;
        if !fits(&td, q) {
            return Err(not_added(&td, q));
        }
        let end = Expr::Arith(Box::new(shown(d.clone(), td, self.now)), ArithOp::Add, Box::new(x));
        Ok((d, end, td))
    }
}
