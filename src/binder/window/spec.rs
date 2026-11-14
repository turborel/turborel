//! [ISO/IEC 9075-2:2023 §7.15] Windows: the definitions of a `<window
//! clause>` described, each with those before it in scope [SR16], and a
//! window's partitioning, ordering and frame bound in the select list's
//! scope, over the rows of the table expression or its groups.

use super::super::composite::{grouping_operand, integer, ordering_operand};
use super::super::{Binder, Scope, rule};
use crate::ast::{
    Exclusion as Excluded, Frame as Written, FrameBound, FrameUnits, GroupingColumn, SetArg, ValueExpression, ValueOrder, WindowDefinition,
    WindowSpecification,
};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::LISTAGG;
use crate::lexer::kw;
use crate::logical::{Bound, Exclusion, Expr, Frame, Units};
use crate::params::ParameterType;
use crate::value::{ArithOp, DataType, SortOrder};

/// [ISO/IEC 9075-2:2023 §7.15 SR6] A grouped query's window referencing a
/// column not grouped by.
const NOT_GROUPED: &str =
    "[ISO/IEC 9075-2:2023 §7.15 SR6] in a grouped query, a window references grouping columns alone, outside set functions";

/// [ISO/IEC 9075-2:2023 §7.15 GR1b i] A window structure descriptor: its
/// partitioning, ordering and framing, those of an existing window taken.
#[derive(Debug, Clone)]
pub(in crate::binder) struct Described {
    /// Its window partitioning columns.
    pub(in crate::binder) partition: Vec<GroupingColumn>,
    /// Its window ordering.
    pub(in crate::binder) order: Vec<ValueOrder>,
    /// Its window framing.
    pub(in crate::binder) frame: Option<Written>,
}

/// A window's partitioning and ordering, bound.
pub(in crate::binder) struct Keys {
    /// Its partitioning columns, each its sort key if it has a collation.
    pub(in crate::binder) partition: Vec<Expr>,
    /// Its sort keys, each its collation's sort key if it has one, and
    /// their orders.
    pub(in crate::binder) order: Vec<(Expr, SortOrder)>,
    /// The type of each sort key.
    types: Vec<DataType>,
    /// It as text, which tells two apart.
    pub(in crate::binder) text: String,
}

/// [SR2, SR12, SR16] The definitions `definitions`, each described with
/// those before it in scope, by name.
pub(super) fn described(definitions: &[WindowDefinition]) -> Result<Vec<(String, Described)>> {
    let mut named: Vec<(String, Described)> = Vec::new();
    for d in definitions {
        if named.iter().any(|(n, _)| *n == d.name) {
            return Err(SqlError::at(
                Code::SyntaxRule,
                d.at,
                format!("[ISO/IEC 9075-2:2023 §7.15 SR2] window {} is defined twice", d.name),
            ));
        }
        let described = describe(&d.specification, &named)?;
        named.push((d.name.clone(), described));
    }
    Ok(named)
}

/// [SR12, GR1b i] The window `spec` describes, the windows `named` in
/// scope: of an existing window, its partitioning, and its ordering if it
/// has one.
pub(super) fn describe(spec: &WindowSpecification, named: &[(String, Described)]) -> Result<Described> {
    let Some((name, at)) = &spec.existing else {
        return Ok(Described { partition: spec.partition.clone(), order: spec.order.clone(), frame: spec.frame.clone() });
    };
    let refused = |text: String| SqlError::at(Code::SyntaxRule, *at, text);
    let Some((_, existing)) = named.iter().find(|(n, _)| n == name) else {
        return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.15 SR12a] no window {name} is in scope")));
    };
    if !spec.partition.is_empty() {
        return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.15 SR12c] a window over the existing window {name} has no PARTITION BY")));
    }
    if !existing.order.is_empty() && !spec.order.is_empty() {
        return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.15 SR12d] the existing window {name} has an ordering, so this one has none")));
    }
    if existing.frame.is_some() {
        return Err(refused(format!("[ISO/IEC 9075-2:2023 §7.15 SR12e] the existing window {name} has a frame")));
    }
    let order = if spec.order.is_empty() { existing.order.clone() } else { spec.order.clone() };
    Ok(Described { partition: existing.partition.clone(), order, frame: spec.frame.clone() })
}

/// [ISO/IEC 9075-2:2023 §9.23 SR8c, SR8d] That the aggregate function
/// `aggregate` of a window, `described`, is no hypothetical set function,
/// and of a window of an ordering or a frame is no LISTAGG or inverse
/// distribution function and is DISTINCT only if COUNT. COUNT DISTINCT
/// over such a window is T627, not claimed.
pub(super) fn aggregate_rules(aggregate: &ValueExpression, described: &Described) -> Result<()> {
    let ValueExpression::SetFunction(sf) = aggregate else { return Ok(()) };
    if matches!(sf.arg, SetArg::Hypothetical(_)) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.23 SR8c] {} WITHIN GROUP, a hypothetical set function, is over no window",
            sf.name
        )));
    }
    if described.order.is_empty() && described.frame.is_none() {
        return Ok(());
    }
    if sf.name == LISTAGG || matches!(sf.arg, SetArg::Fraction(_)) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.23 SR8d i] {} is over a window of no ordering and no frame", sf.name)));
    }
    if !matches!(sf.arg, SetArg::Distinct(_)) {
        return Ok(());
    }
    if sf.name == kw::COUNT {
        return Err(SqlError::new(
            Code::Unsupported,
            "[ISO/IEC 9075-2:2023 §9.23 SR8d ii] COUNT(DISTINCT ...) over a window of an ordering or a frame is Feature T627, which turborel does not support",
        ));
    }
    Err(rule(format!("[ISO/IEC 9075-2:2023 §9.23 SR8d ii] {}(DISTINCT ...) is over a window of no ordering and no frame", sf.name)))
}

/// [SR10] That the bounds `start` and `end` of a frame are a pair it may
/// have.
fn pair_rules(start: &FrameBound, end: &FrameBound) -> Result<()> {
    let refused = match (start, end) {
        (FrameBound::UnboundedFollowing, _) => "SR10a] a frame's first bound is not UNBOUNDED FOLLOWING",
        (_, FrameBound::UnboundedPreceding) => "SR10b] a frame's last bound is not UNBOUNDED PRECEDING",
        (FrameBound::CurrentRow, FrameBound::Preceding(_)) => "SR10c] a frame from CURRENT ROW does not end PRECEDING",
        (FrameBound::Following(_), FrameBound::Preceding(_) | FrameBound::CurrentRow) => {
            "SR10d] a frame from a row FOLLOWING ends FOLLOWING"
        }
        (FrameBound::UnboundedPreceding | FrameBound::Preceding(_) | FrameBound::CurrentRow | FrameBound::Following(_), _) => {
            return Ok(());
        }
    };
    Err(rule(format!("[ISO/IEC 9075-2:2023 §7.15 {refused}")))
}

impl Binder<'_> {
    /// The partitioning and ordering of `described`, bound in `scope`.
    pub(in crate::binder) fn window_keys(&self, described: &Described, scope: Scope<'_>) -> Result<Keys> {
        let scope = match scope {
            Scope::Groups(g, _) => Scope::Groups(g, NOT_GROUPED),
            Scope::Rows(_) | Scope::Select => scope,
        };
        let mut partition = Vec::new();
        for g in &described.partition {
            partition.push(self.partitioning_column(g, scope).or_at(g.column.at)?);
        }
        let (mut order, mut types) = (Vec::new(), Vec::new());
        for o in &described.order {
            let (e, t) = self.value_expression(&o.key, scope)?;
            // [§10.10 SR2] an operand of an ordering operation, ordered by
            // its collation [§9.15].
            ordering_operand(&t, "a <window order clause>")?;
            let collation = if t.is_character() { self.collation_of(&[self.derived(&o.key)?])? } else { None };
            order
                .push((collation.map_or_else(|| e.clone(), |c| Expr::sort_key(e.clone(), &c)), SortOrder::of(o.descending, o.nulls_first)));
            types.push(t);
        }
        let text = format!("{partition:?} {order:?}");
        Ok(Keys { partition, order, types, text })
    }

    /// [SR4, SR5] The window partitioning column `g`, bound in `scope`: a
    /// column of the table expression's result, an operand of a grouping
    /// operation, its sort key if it has a collation.
    fn partitioning_column(&self, g: &GroupingColumn, scope: Scope<'_>) -> Result<Expr> {
        let column = ValueExpression::Column(g.column.clone());
        if !self.is_local(&column)? {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.15 SR4] the window partitioning column {} is no column of the table expression",
                g.column.column
            )));
        }
        let (e, t) = self.value_expression(&column, scope)?;
        grouping_operand(&t, "a <window partition clause>")?;
        if g.collate.is_some() && !t.is_character() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.15 SR5a] a window partitioning column of {t} has no COLLATE")));
        }
        if !t.is_character() {
            return Ok(e);
        }
        let collation = self.chosen(g.collate.as_ref(), self.derived(&column)?)?;
        Ok(collation.map_or_else(|| e.clone(), |c| Expr::sort_key(e.clone(), &c)))
    }

    /// The frame `written`, of a window of `keys`, bound in `scope`; of
    /// none, the frame implied [GR5a].
    pub(in crate::binder) fn window_frame(&self, written: Option<&Written>, keys: &Keys, scope: Scope<'_>) -> Result<Frame> {
        let Some(f) = written else { return Ok(Frame::implied(!keys.order.is_empty())) };
        // [SR11] a start alone, BETWEEN it AND CURRENT ROW.
        let end = f.end.as_ref().unwrap_or(&FrameBound::CurrentRow);
        pair_rules(&f.start, end).or_at(f.at)?;
        let units = match f.units {
            FrameUnits::Rows => Units::Rows,
            FrameUnits::Range => Units::Range,
            FrameUnits::Groups => Units::Groups,
        };
        if units == Units::Groups && keys.order.is_empty() {
            return Err(SqlError::at(Code::SyntaxRule, f.at, "[ISO/IEC 9075-2:2023 §7.15 SR15c i] a window of GROUPS has an ordering"));
        }
        let start = self.frame_bound(&f.start, (units, keys), scope)?;
        let end = self.frame_bound(end, (units, keys), scope)?;
        let exclusion = match f.exclusion {
            Excluded::NoOthers => Exclusion::NoOthers,
            Excluded::CurrentRow => Exclusion::CurrentRow,
            Excluded::Group => Exclusion::Group,
            Excluded::Ties => Exclusion::Ties,
        };
        Ok(Frame { units, start, end, exclusion })
    }

    /// The bound `b` of a frame of `units` over a window of `keys`.
    fn frame_bound(&self, b: &FrameBound, (units, keys): (Units, &Keys), scope: Scope<'_>) -> Result<Bound> {
        Ok(match b {
            FrameBound::UnboundedPreceding => Bound::UnboundedPreceding,
            FrameBound::Preceding(v) => Bound::Preceding(self.frame_offset(v, (units, keys), scope)?),
            FrameBound::CurrentRow => Bound::CurrentRow,
            FrameBound::Following(v) => Bound::Following(self.frame_offset(v, (units, keys), scope)?),
            FrameBound::UnboundedFollowing => Bound::UnboundedFollowing,
        })
    }

    /// [SR15] The offset `v` of a bound of a frame of `units` over a window
    /// of `keys`: of ROWS and GROUPS, exact numeric of scale 0; of RANGE,
    /// of a window of one sort key, numeric, datetime or interval, a
    /// number of a numeric key, else an interval added to it.
    fn frame_offset(&self, v: &ValueExpression, (units, keys): (Units, &Keys), _scope: Scope<'_>) -> Result<Expr> {
        let none = Scope::Rows("[ISO/IEC 9075-2:2023 §7.15] a frame's offset is an <unsigned value specification>");
        let key = match units {
            Units::Rows | Units::Groups => {
                let (e, t) = self.typed(v, ParameterType::Of(integer()), none)?;
                if !matches!(t, DataType::Exact { scale: 0, .. }) {
                    let rule_of = if units == Units::Rows { "SR15b" } else { "SR15c ii" };
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §7.15 {rule_of}] the offset of a frame of ROWS or GROUPS is exact numeric of scale 0, not {t}"
                    )));
                }
                return Ok(e);
            }
            Units::Range => match keys.types.as_slice() {
                [key] => key,
                [] | [_, _, ..] => {
                    return Err(rule(
                        "[ISO/IEC 9075-2:2023 §7.15 SR15a ii] a frame of RANGE with an offset is of a window of one sort key",
                    ));
                }
            },
        };
        let (e, t) = if key.is_numeric() || key.is_interval() {
            self.typed(v, ParameterType::Of(key.clone()), none)?
        } else {
            self.value_expression(v, none)?
        };
        let fits = if key.is_numeric() {
            t.is_numeric()
        } else if key.is_datetime() || key.is_interval() {
            t.is_interval() && self.temporal((Expr::Column(0), key.clone()), ArithOp::Add, (Expr::Column(1), t.clone()), false).is_ok()
        } else {
            false
        };
        if !fits {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.15 SR15a iii] a frame of RANGE is of a numeric, datetime or interval sort key, its offset a number of a number, else an interval added to it: not {t} of {key}"
            )));
        }
        Ok(e)
    }
}
