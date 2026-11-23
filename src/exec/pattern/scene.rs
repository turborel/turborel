//! [ISO/IEC 9075-2:2023 §6.27, §6.31, §6.33, §10.9 GR1a to c, §4.23.2]
//! What a definition or a measure reads of a row pattern match. A match is
//! seen as a `Scene`: the rows of its partition, its first row, and the
//! primary variable each of its rows is mapped to. A value is read of the
//! rows mapped to its variable up to a count of the match's rows, the
//! running ones, or of them all, the final ones. Each expression a value
//! evaluates in a row is over that row, then its classifier, the name of
//! the variable the match maps it to, null if it maps it to none [§6.33
//! GR20e], so a definition sees no classifier of a row after its
//! candidate, then the match's number [§6.31].

use super::aggregate::Fed;
use crate::error::{Code, Result, SqlState, fail_with};
use crate::exec::work::Work;
use crate::exec::{HeldRows, Subqueries, broken, eval, eval_cond};
use crate::logical::{Cond, Expr, Matching, Navigated, PatternAggregate, PatternValue, Slotted, Truth};
use crate::pattern::Mapped;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::value::{Decimal, Value};

/// What every value of one recognition is read with: its matching, the
/// runner of its subqueries, the work area a DISTINCT set function keeps
/// its values in, and the space an inverse distribution function holds
/// its values in.
pub(in crate::exec) struct Reading<'a> {
    /// The variables, definitions and skip.
    pub(in crate::exec) matching: &'a Matching,
    /// The runner of subqueries and hearer of warnings.
    pub(in crate::exec) runner: &'a dyn Subqueries,
    /// The work area a set function's values are kept or sorted in.
    pub(in crate::exec) work: &'a Work,
    /// The space a set function's values are held in.
    pub(in crate::exec) space: &'a ListSpace,
}

/// [ISO/IEC 9075-2:2023 §9.41] A row pattern match, potential, retained
/// or designated, as its values read it.
pub(in crate::exec) struct Scene<'a> {
    /// The rows of the row pattern partition [§6.27 GR7], or of the window
    /// partition, in order.
    pub(in crate::exec) rows: &'a mut HeldRows<Row>,
    /// The place of the match's first row in the partition.
    pub(in crate::exec) start: usize,
    /// The primary variable each row from `start` on is mapped to, as far
    /// as it is known.
    pub(in crate::exec) mapped: &'a [Mapped],
    /// [§6.31] The match's number in its partition; none in a window
    /// [§6.31 SR22a].
    pub(in crate::exec) number: Option<u64>,
}

/// [§6.27 GR11b i] Of the rows `set` of a variable, in order, the one
/// `logical` from the first, or from the last if `last`; none if there
/// are not so many [GR11a].
fn picked(set: &[usize], logical: usize, last: bool) -> Option<usize> {
    if last { set.len().checked_sub(logical.saturating_add(1)).and_then(|at| set.get(at)).copied() } else { set.get(logical).copied() }
}

/// [§4.23.2] How many of the match's rows a value reads, of `mapped`:
/// all if `final_`, else the first `upto`, the rows up to the current row.
pub(in crate::exec) fn reach(final_: bool, upto: usize, mapped: &[Mapped]) -> usize {
    if final_ { mapped.len() } else { upto.min(mapped.len()) }
}

impl Reading<'_> {
    /// [ISO/IEC 9075-2:2023 §7.9 SR16, SR18] Whether the primary variable
    /// `primary` is a component of the variable `variable`: of the
    /// universal row pattern variable, the last, every one.
    pub(in crate::exec) fn covers(&self, variable: usize, primary: usize) -> bool {
        if variable.saturating_add(1) == self.matching.variables.len() {
            return true;
        }
        self.matching.variables.get(variable).is_some_and(|v| v.components.contains(&primary))
    }

    /// [ISO/IEC 9075-2:2023 §6.27 GR5, GR6, §10.9 GR1a] The places, from
    /// the match's first row, of those of its first `upto` rows `mapped`
    /// maps to a component of `variable`, in order.
    pub(in crate::exec) fn members(&self, mapped: &[Mapped], variable: usize, upto: usize) -> Vec<usize> {
        mapped.iter().take(upto).enumerate().filter(|(_, m)| self.covers(variable, m.variable)).map(|(j, _)| j).collect()
    }

    /// [ISO/IEC 9075-2:2023 §6.33 GR20] The classifier of the `place`th row
    /// of the partition in `scene`: the name of the primary variable the
    /// match maps it to, null if it maps it to none.
    fn classifier(&self, scene: &Scene<'_>, place: usize) -> Value {
        let mapped = place.checked_sub(scene.start).and_then(|j| scene.mapped.get(j));
        mapped.and_then(|m| self.matching.variables.get(m.variable)).map_or(Value::Null, |v| Value::Char(v.name.clone().into()))
    }

    /// The `place`th row of the partition in `scene`, then its classifier,
    /// then the match's number, null in a window: the row a value's
    /// expressions are evaluated over.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the row.
    pub(in crate::exec) fn extended(&self, scene: &mut Scene<'_>, place: usize) -> Result<Row> {
        let classifier = self.classifier(scene, place);
        let mut row = scene.rows.get(place)?.ok_or_else(|| broken("a row pattern's row past its partition's last"))?;
        row.push(classifier);
        row.push(scene.number.map_or(Value::Null, |n| Value::Exact(Decimal::new(i128::from(n), 0))));
        Ok(row)
    }

    /// [ISO/IEC 9075-2:2023 §6.27 GR2] The value of the offset `e`.
    fn offset(&self, e: &Expr) -> Result<usize> {
        let v = eval(e, &Vec::new(), self.runner)?;
        let what = if v.is_null() {
            "null"
        } else {
            let Value::Exact(d) = v else { return Err(broken("a row pattern navigation's offset is not an exact number")) };
            if d.scale != 0 {
                return Err(broken("a row pattern navigation's offset is not of scale 0"));
            }
            if d.units >= 0 {
                return Ok(usize::try_from(d.units).unwrap_or(usize::MAX));
            }
            "negative"
        };
        fail_with(
            Code::Data,
            SqlState::INVALID_ARGUMENT_FOR_ROW_PATTERN_NAVIGATION,
            format!("[ISO/IEC 9075-2:2023 §6.27 GR2] a row pattern navigation's offset is {what}"),
        )
    }

    /// [ISO/IEC 9075-2:2023 §6.27 GR11] The navigation `n` of the rows
    /// `set` of its variable, places from the match's first row: the row
    /// its logical offset picks of them, moved by its physical offset
    /// within the partition, its expression evaluated there; null if there
    /// is no such row.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2202J if an offset is negative or null [GR2]; the
    /// expression's.
    pub(in crate::exec) fn navigated(&self, scene: &mut Scene<'_>, n: &Navigated, set: &[usize]) -> Result<Value> {
        let (logical, physical) = (self.offset(&n.logical)?, self.offset(&n.physical)?);
        let Some(j) = picked(set, logical, n.last) else { return Ok(Value::Null) };
        let at = scene.start.saturating_add(j);
        let target = if n.next { at.checked_add(physical).filter(|&t| t < scene.rows.len()) } else { at.checked_sub(physical) };
        match target {
            None => Ok(Value::Null),
            Some(t) => {
                let row = self.extended(scene, t)?;
                eval(&n.expr, &row, self.runner)
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.9 GR1a to c] The aggregate `a` over the
    /// rows `set` of its variable, places from the match's first row.
    ///
    /// # Errors
    ///
    /// The set function's; its argument's or filter's.
    pub(in crate::exec) fn aggregated(&self, scene: &mut Scene<'_>, a: &PatternAggregate, set: &[usize]) -> Result<Value> {
        let mut fed = Fed::new(&a.call, self);
        for &j in set {
            fed.feed(&a.call, self, scene, scene.start.saturating_add(j))?;
        }
        fed.finish(&a.call, self, scene)
    }

    /// [ISO/IEC 9075-2:2023 §6.31 GR20] MATCH_NUMBER of `scene`.
    pub(in crate::exec) fn number(scene: &Scene<'_>) -> Result<Value> {
        let n = scene.number.ok_or_else(|| broken("MATCH_NUMBER of a window's match"))?;
        Ok(Value::Exact(Decimal::new(i128::from(n), 0)))
    }

    /// The value `v` reads of `scene`, its current row the `upto`th of the
    /// match, each read afresh.
    fn value(&self, scene: &mut Scene<'_>, v: &PatternValue, upto: usize) -> Result<Value> {
        match v {
            PatternValue::Navigated(n) => {
                let set = self.members(scene.mapped, n.variable, reach(n.final_, upto, scene.mapped));
                self.navigated(scene, n, &set)
            }
            PatternValue::MatchNumber => Reading::number(scene),
            PatternValue::Aggregate(a) => {
                let set = self.members(scene.mapped, a.variable, reach(a.final_, upto, scene.mapped));
                self.aggregated(scene, a, &set)
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.41 GR7] Whether the definition `define`,
    /// none for TRUE, is true of the candidate row of the potential match
    /// `scene`, its last row, with everything read running [§6.27 SR5b].
    ///
    /// # Errors
    ///
    /// Its values' and condition's evaluations'.
    pub(in crate::exec) fn holds(&self, scene: &mut Scene<'_>, define: Option<&Slotted<Cond>>) -> Result<bool> {
        let Some(d) = define else { return Ok(true) };
        let upto = scene.mapped.len();
        let slots = d.values.iter().map(|v| self.value(scene, v, upto)).collect::<Result<Row>>()?;
        Ok(eval_cond(&d.expr, &slots, self.runner)? == Truth::True)
    }

    /// [ISO/IEC 9075-2:2023 §7.7 GR11a ii, §6.10 GR1e ii] The values of
    /// `measures` of `scene`, its current row the `upto`th of the match,
    /// each read afresh.
    ///
    /// # Errors
    ///
    /// Their values' and expressions' evaluations'.
    pub(in crate::exec) fn measured(&self, scene: &mut Scene<'_>, measures: &Slotted<Vec<Expr>>, upto: usize) -> Result<Row> {
        let slots = measures.values.iter().map(|v| self.value(scene, v, upto)).collect::<Result<Row>>()?;
        measures.expr.iter().map(|e| eval(e, &slots, self.runner)).collect()
    }

    /// [ISO/IEC 9075-2:2023 §7.7 GR8b iii, §7.15 GR6d iii 2 E III] The
    /// place, from the match's first row, of the row of `mapped` that
    /// SKIP TO FIRST, or LAST if `last`, of `variable` goes to, `rules`
    /// citing the rules of no such row and of the first row.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2202K if no row is mapped to it; 2202L if it is the
    /// match's first.
    pub(in crate::exec) fn skipped_to(&self, mapped: &[Mapped], (variable, last): (usize, bool), rules: (&str, &str)) -> Result<usize> {
        let set = self.members(mapped, variable, mapped.len());
        let found = if last { set.last() } else { set.first() };
        let Some(&j) = found else {
            return fail_with(
                Code::Data,
                SqlState::SKIP_TO_NON_EXISTENT_ROW,
                format!("{} no row of the match is mapped to the row pattern variable skipped to", rules.0),
            );
        };
        if j == 0 {
            return fail_with(
                Code::Data,
                SqlState::SKIP_TO_FIRST_ROW_OF_MATCH,
                format!("{} the row skipped to is the first row of the match", rules.1),
            );
        }
        Ok(j)
    }
}
