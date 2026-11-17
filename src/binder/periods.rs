//! [ISO/IEC 9075-2:2023 §8.20] The `<period predicate>`s as a query binds
//! them. A `<period reference>` names a period of a table the query, or a
//! query it is in, reads, and stands for its start and end columns
//! [GR4a i 2, GR4b i 2]; PERIOD ( start , end ) writes one, of two
//! datetimes of one type, not of TIME [SR1]; after CONTAINS, any other
//! value is a point in time, not of TIME [SR2]. Every bound is cast to
//! TIMESTAMP WITH TIME ZONE of the greatest precision, IL055 [GR3, GR4],
//! and `functions/periods.rs` judges them.

use super::expression::dynamic_of;
use super::structured::call;
use super::{Binder, Scope, rule};
use crate::ast::{ColumnSpecification, PeriodOperand, PeriodOperator, TableName, ValueExpression};
use crate::error::Result;
use crate::functions::PeriodTest;
use crate::logical::{Cond, Expr};
use crate::value::DataType;
use crate::value::datetime::MAX_TIME_PRECISION;

/// [ISO/IEC 9075-2:2023 §8.20 GR3] The type every bound is cast to:
/// TIMESTAMP(n) WITH TIME ZONE, n the greatest `<timestamp precision>`
/// (IL055).
const COMPARED: DataType = DataType::Timestamp { precision: MAX_TIME_PRECISION, zoned: true };

/// Whether `t` may bound a period or be a point in time: DATE or a
/// timestamp type, not TIME [§8.20 SR1a, SR2].
fn instant(t: &DataType) -> bool {
    matches!(t, DataType::Date | DataType::Timestamp { .. })
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §8.20] The `<period predicate>` `op` of
    /// `left` and `right`, where `scope` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an operand is not a period, or after CONTAINS a
    /// point in time, or a bound is of no datetime type it may be.
    pub(super) fn period_predicate(
        &self,
        op: PeriodOperator,
        (left, right): (&PeriodOperand, &PeriodOperand),
        scope: Scope<'_>,
    ) -> Result<Cond> {
        let not_period = || rule("[ISO/IEC 9075-2:2023 §8.20] a <period predicand> is PERIOD ( start , end ) or names a period");
        let (s1, e1, w1) = self.period_operand(left, scope)?.ok_or_else(not_period)?;
        let mut args = vec![s1, e1];
        let w2 = match (self.period_operand(right, scope)?, right, op == PeriodOperator::Contains) {
            (Some((s2, e2, w2)), _, _) => {
                args.extend([s2, e2]);
                w2
            }
            // [GR4a iii 1] after CONTAINS, a point in time.
            (None, PeriodOperand::Value(v), true) => {
                args.push(self.point_in_time(v, scope)?);
                false
            }
            (None, PeriodOperand::Value(_), false) | (None, PeriodOperand::Written(..), _) => return Err(not_period()),
        };
        Ok(Cond::Value(call(PeriodTest { op, written: [w1, w2] }, args)))
    }

    /// The period `o` is, bound: its start, its end, and whether it is
    /// written; none if it is a value naming no period.
    fn period_operand(&self, o: &PeriodOperand, scope: Scope<'_>) -> Result<Option<(Expr, Expr, bool)>> {
        let cs = match o {
            PeriodOperand::Written(start, end) => return self.written_period(start, end, scope).map(Some),
            PeriodOperand::Value(v) => {
                // A <period reference> is a <basic identifier chain>.
                let ValueExpression::Column(cs) = &**v else { return Ok(None) };
                cs
            }
        };
        let Some((start, end)) = self.period_named(cs)? else { return Ok(None) };
        let (s, ts) = self.column_expr(&start, scope)?;
        let (e, te) = self.column_expr(&end, scope)?;
        Ok(Some((self.datetime_cast(s, &ts, COMPARED)?, self.datetime_cast(e, &te, COMPARED)?, false)))
    }

    /// [ISO/IEC 9075-2:2023 §8.20 SR2, GR4a iv 1] The point in time `v`, a
    /// `?` of TIMESTAMP WITH TIME ZONE, cast to it.
    fn point_in_time(&self, v: &ValueExpression, scope: Scope<'_>) -> Result<Expr> {
        let (x, t) = self.typed_by(v, Some(COMPARED), scope)?;
        if !instant(&t) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.20 SR2] a point in time is DATE or a timestamp, not {t}")));
        }
        self.datetime_cast(x, &t, COMPARED)
    }

    /// [ISO/IEC 9075-2:2023 §8.20 SR1] PERIOD ( `start` , `end` ): of one
    /// datetime type, not TIME, a `?` taking the other's.
    fn written_period(&self, start: &ValueExpression, end: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, Expr, bool)> {
        let ((s, ts), (e, te)) = if dynamic_of(start).is_some() {
            let (e, te) = self.value_expression(end, scope)?;
            (self.typed_by(start, Some(te.clone()), scope)?, (e, te))
        } else {
            let (s, ts) = self.value_expression(start, scope)?;
            let taken = dynamic_of(end).map(|_| ts.clone());
            ((s, ts), self.typed_by(end, taken, scope)?)
        };
        if !instant(&ts) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.20 SR1a] a period's start is DATE or a timestamp, not {ts}")));
        }
        if ts != te {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.20 SR1b] a period's start and end are of one type, not {ts} and {te}")));
        }
        Ok((self.datetime_cast(s, &ts, COMPARED)?, self.datetime_cast(e, &te, COMPARED)?, true))
    }

    /// [ISO/IEC 9075-2:2023 §8.20 GR4a i 2] The start and end columns of
    /// the period `cs` names, a `<period reference>`, as column references
    /// of the table that has it: of this query, else of one it is in.
    /// None if it names no period, so it is a column.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if two tables of a query have a period of the name.
    fn period_named(&self, cs: &ColumnSpecification) -> Result<Option<(ColumnSpecification, ColumnSpecification)>> {
        let mut found =
            self.items.iter().filter(|it| cs.qualifier.as_ref().is_none_or(|q| it.designated_by(q, self.schema))).filter_map(|it| {
                let p = it.desc.periods.iter().find(|p| p.name == cs.column)?;
                let qualifier = match &it.correlation {
                    Some(c) => TableName { auth: None, ident: c.clone(), at: cs.at },
                    None => TableName { auth: Some(it.table.auth.clone()), ident: it.table.ident.clone(), at: cs.at },
                };
                let column = |name: &str| ColumnSpecification { qualifier: Some(qualifier.clone()), column: name.to_owned(), ..cs.clone() };
                Some((column(&p.start), column(&p.end)))
            });
        let Some(first) = found.next() else {
            return match self.outer {
                Some(outer) => outer.binder().period_named(cs),
                None => Ok(None),
            };
        };
        if found.next().is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.20] period {} is of more than one table", cs.column)).or_at(cs.at));
        }
        Ok(Some(first))
    }
}
